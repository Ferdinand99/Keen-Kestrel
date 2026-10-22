//! Alerts (docs/design.md §4.7): per-project rules (a new issue, a
//! regression, a spike, the first crash of a release) that send to the
//! instance's notification channels through vole-kit's providers.
//!
//! Nothing here slows ingest down. The writer hands over what it saw after
//! each committed batch ([`Trigger`]s, through a bounded queue it never
//! waits for); a task of its own evaluates the rules on the read pool and
//! puts one entry per channel in `alert_log`, which is the delivery queue
//! and the history at once. Another task sends what is due, with retries
//! (after 1, 5 and 15 minutes) and digests, and notes every result.
//!
//! * **Cooldown**: a rule alerts for an issue at most once per
//!   `cooldown_minutes`.
//! * **Digest**: with `digest_minutes`, a channel gets one message with
//!   everything that came up in that many minutes.
//! * **Spike**: events in the last hour (this hour's plus the share of the
//!   previous hour the last 60 minutes still cover, from `issue_hours`)
//!   above `max(threshold, multiplier × the hourly average of the 168 hours
//!   before)`; checked at most once a minute per issue.
//! * **Language**: each channel's own, else the instance's.

pub mod channels;
pub mod message;
pub mod pages;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::future::join_all;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc, oneshot};
use vole_kit::i18n::Lang;
use vole_kit::state::Core;

use crate::db::alerts::{self as db, LogEntry, Rule, RuleKind};

/// Something the writer saw that may alert, after its batch committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trigger {
    NewIssue {
        project_id: i64,
        issue_id: i64,
        environment: String,
    },
    Regression {
        project_id: i64,
        issue_id: i64,
        environment: String,
    },
    /// The first crash of a release.
    FirstCrash {
        project_id: i64,
        issue_id: i64,
        release_id: i64,
        environment: String,
    },
    /// The issue got events (spikes).
    Events { project_id: i64, issue_id: i64 },
}

impl Trigger {
    pub fn project_id(&self) -> i64 {
        match self {
            Trigger::NewIssue { project_id, .. }
            | Trigger::Regression { project_id, .. }
            | Trigger::FirstCrash { project_id, .. }
            | Trigger::Events { project_id, .. } => *project_id,
        }
    }
}

/// How alerts are delivered.
#[derive(Debug, Clone)]
pub struct Settings {
    /// Waits before the second, third … attempt; after the last, an alert
    /// has failed.
    pub retry_delays: Vec<Duration>,
    /// How often the sender looks for due alerts (it is also woken).
    pub tick: Duration,
    /// An issue's spike is checked at most this often.
    pub spike_check_every: Duration,
    /// Messages sent at once.
    pub concurrency: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            retry_delays: vec![
                Duration::from_secs(60),
                Duration::from_secs(5 * 60),
                Duration::from_secs(15 * 60),
            ],
            tick: Duration::from_secs(15),
            spike_check_every: Duration::from_secs(60),
            concurrency: 8,
        }
    }
}

/// Counters, for tests and logs.
#[derive(Debug, Default)]
pub struct Stats {
    /// Triggers evaluated.
    pub evaluated: AtomicU64,
    /// Log entries made (one per channel).
    pub queued: AtomicU64,
    /// Messages delivered (a digest counts once).
    pub sent: AtomicU64,
    /// Attempts that failed.
    pub failed: AtomicU64,
    /// Trigger batches lost because the queue was full.
    pub dropped: AtomicU64,
}

enum Msg {
    Triggers(Vec<Trigger>),
    Flush(oneshot::Sender<()>),
}

/// Trigger batches waiting at most; the writer drops more (with a warning)
/// rather than wait.
const QUEUE: usize = 4096;
/// Log entries sent per round.
const DUE_BATCH: i64 = 100;

/// The writer's end of the queue. Sending never waits.
#[derive(Clone)]
pub struct Sink {
    tx: mpsc::Sender<Msg>,
    inner: Arc<Inner>,
}

impl Sink {
    pub fn send(&self, triggers: Vec<Trigger>) {
        if triggers.is_empty() {
            return;
        }
        if self.tx.try_send(Msg::Triggers(triggers)).is_err() {
            self.inner.stats.dropped.fetch_add(1, Ordering::Relaxed);
            tracing::warn!("the alert queue is full; dropped a batch of alert triggers");
        }
    }
}

/// The alerts: rule evaluation and delivery.
#[derive(Clone)]
pub struct Alerts {
    inner: Arc<Inner>,
    tx: mpsc::Sender<Msg>,
}

struct Inner {
    core: Core,
    settings: Settings,
    wake: Notify,
    /// One delivery round at a time, so nothing is sent twice.
    deliver: tokio::sync::Mutex<()>,
    spike_checked: Mutex<HashMap<i64, Instant>>,
    stats: Stats,
}

impl Alerts {
    /// Starts the evaluation and delivery tasks.
    pub fn start(core: Core, settings: Settings) -> Alerts {
        let (tx, rx) = mpsc::channel(QUEUE);
        let alerts = Alerts {
            inner: Arc::new(Inner {
                core,
                settings,
                wake: Notify::new(),
                deliver: tokio::sync::Mutex::new(()),
                spike_checked: Mutex::new(HashMap::new()),
                stats: Stats::default(),
            }),
            tx,
        };
        tokio::spawn(evaluate_loop(alerts.clone(), rx));
        tokio::spawn(deliver_loop(alerts.clone()));
        alerts
    }

    /// The handle the writer reports through.
    pub fn sink(&self) -> Sink {
        Sink {
            tx: self.tx.clone(),
            inner: self.inner.clone(),
        }
    }

    pub fn stats(&self) -> &Stats {
        &self.inner.stats
    }

    /// Waits until the triggers handed over before this call are evaluated
    /// (not sent).
    pub async fn flush(&self) {
        let (tx, rx) = oneshot::channel();
        if self.tx.send(Msg::Flush(tx)).await.is_ok() {
            let _ = rx.await;
        }
    }

    /// Wakes the sender.
    pub fn wake(&self) {
        self.inner.wake.notify_one();
    }

    /// Sends everything that is due now and returns how many messages went
    /// out (the sender's task does the same on its own).
    pub async fn deliver_now(&self) -> usize {
        deliver_due(self).await
    }

    /// Sends a test message to a channel that may not be saved yet, in
    /// `language` ("" for the instance's).
    pub async fn send_test(
        &self,
        kind: &str,
        config: &Value,
        language: &str,
    ) -> Result<(), String> {
        let core = &self.inner.core;
        let lang = channel_lang(core, language).await;
        let base = core.base_url().await;
        let http = core.http.clone();
        let kind = kind.to_string();
        let config = config.clone();
        vole_kit::i18n::scope(lang, async move {
            let alert = message::test(base.as_deref());
            vole_kit::notify::send_config(&http, &kind, &config, &alert).await
        })
        .await
    }

    async fn evaluate(&self, batch: Vec<Trigger>) {
        let n = batch.len() as u64;
        let spikes_due = self.spikes_due(&batch);
        let now = crate::issues::now();
        let result = self
            .inner
            .core
            .with_conn(move |c| evaluate_batch(c, &batch, &spikes_due, &now))
            .await;
        self.inner.stats.evaluated.fetch_add(n, Ordering::Relaxed);
        match result {
            Ok(0) => {}
            Ok(queued) => {
                self.inner
                    .stats
                    .queued
                    .fetch_add(queued as u64, Ordering::Relaxed);
                self.wake();
            }
            Err(err) => tracing::warn!(%err, "alert rules could not be evaluated"),
        }
    }

    /// The issues among `batch`'s event triggers whose spike may be checked
    /// now (at most once per `spike_check_every`).
    fn spikes_due(&self, batch: &[Trigger]) -> Vec<(i64, i64)> {
        let mut checked = self
            .inner
            .spike_checked
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let every = self.inner.settings.spike_check_every;
        checked.retain(|_, t| now.duration_since(*t) < every);
        let mut out = Vec::new();
        for t in batch {
            if let Trigger::Events {
                project_id,
                issue_id,
            } = t
                && !checked.contains_key(issue_id)
            {
                checked.insert(*issue_id, now);
                out.push((*project_id, *issue_id));
            }
        }
        out
    }
}

async fn evaluate_loop(alerts: Alerts, mut rx: mpsc::Receiver<Msg>) {
    while let Some(msg) = rx.recv().await {
        match msg {
            Msg::Triggers(batch) => alerts.evaluate(batch).await,
            Msg::Flush(ack) => {
                let _ = ack.send(());
            }
        }
    }
}

async fn deliver_loop(alerts: Alerts) {
    let tick = alerts.inner.settings.tick;
    loop {
        tokio::select! {
            () = alerts.inner.wake.notified() => {}
            () = tokio::time::sleep(tick) => {}
        }
        deliver_due(&alerts).await;
    }
}

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------

/// `time` (`YYYY-MM-DD HH:MM:SS`) moved by `secs` seconds.
pub fn shift(time: &str, secs: i64) -> String {
    jiff::civil::DateTime::strptime("%Y-%m-%d %H:%M:%S", time)
        .ok()
        .and_then(|d| d.checked_add(jiff::SignedDuration::from_secs(secs)).ok())
        .map(|d| d.strftime("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| time.to_string())
}

/// The minute of the hour of `time`.
fn minute_of(time: &str) -> u32 {
    time.get(14..16).and_then(|m| m.parse().ok()).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Spikes
// ---------------------------------------------------------------------------

/// Hours the baseline averages over (7 days).
pub const BASELINE_HOURS: i64 = 168;

/// Events in the last 60 minutes, from hourly buckets: this hour's so far
/// plus the share of the previous hour the window still covers.
pub fn last_hour(current: i64, previous: i64, minute: u32) -> i64 {
    let covered = f64::from(60 - minute.min(60)) / 60.0;
    current + (previous as f64 * covered).round() as i64
}

/// The level a spike must go above: `max(threshold, multiplier × the hourly
/// average over the baseline)`.
pub fn spike_limit(threshold: i64, multiplier: f64, baseline_events: i64) -> f64 {
    let average = baseline_events as f64 / BASELINE_HOURS as f64;
    (threshold as f64).max(multiplier * average)
}

/// What a spike check found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpikeCheck {
    pub last_hour: i64,
    pub limit: f64,
    /// Events per hour on average over the 168 hours before.
    pub baseline: f64,
}

impl SpikeCheck {
    pub fn fires(&self) -> bool {
        self.last_hour as f64 > self.limit
    }
}

/// An issue's spike numbers at `now` for a rule.
pub fn check_spike(
    c: &Connection,
    issue_id: i64,
    rule: &Rule,
    now: &str,
) -> rusqlite::Result<SpikeCheck> {
    let current = crate::db::issues::hour_of(now);
    let previous = shift(&current, -3600);
    let base_start = shift(&previous, -BASELINE_HOURS * 3600);
    let (cur, prev, base): (i64, i64, i64) = c
        .prepare_cached(
            "SELECT coalesce(SUM(CASE WHEN hour = ?2 THEN events END), 0),
                    coalesce(SUM(CASE WHEN hour = ?3 THEN events END), 0),
                    coalesce(SUM(CASE WHEN hour < ?3 THEN events END), 0)
             FROM issue_hours WHERE issue_id = ?1 AND hour >= ?4",
        )?
        .query_row(params![issue_id, current, previous, base_start], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
    Ok(SpikeCheck {
        last_hour: last_hour(cur, prev, minute_of(now)),
        limit: spike_limit(rule.threshold, rule.multiplier, base),
        baseline: base as f64 / BASELINE_HOURS as f64,
    })
}

// ---------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------

/// What an alert is about, as its log entry keeps it.
fn issue_data(c: &Connection, issue_id: i64) -> rusqlite::Result<Option<(Value, String)>> {
    c.prepare_cached(
        "SELECT i.title, i.culprit, i.level, i.status, i.times_seen, i.users_seen, lr.version,
                p.id, p.name
         FROM issues i JOIN projects p ON p.id = i.project_id
         LEFT JOIN releases lr ON lr.id = i.last_release_id
         WHERE i.id = ?1",
    )?
    .query_row([issue_id], |r| {
        let status: String = r.get(3)?;
        let project_id: i64 = r.get(7)?;
        Ok((
            json!({
                "project": {"id": project_id, "name": r.get::<_, String>(8)?},
                "issue": {
                    "id": issue_id,
                    "title": r.get::<_, String>(0)?,
                    "culprit": r.get::<_, String>(1)?,
                    "level": r.get::<_, String>(2)?,
                    "events": r.get::<_, i64>(4)?,
                    "users": r.get::<_, i64>(5)?,
                },
                "release": r.get::<_, Option<String>>(6)?,
                "path": format!("/projects/{project_id}/issues/{issue_id}"),
            }),
            status,
        ))
    })
    .optional()
}

fn env_matches(rule: &Rule, environment: &str) -> bool {
    rule.environment.is_empty() || rule.environment == environment
}

/// Evaluates a batch of triggers against the projects' rules and queues
/// what fires. Returns how many log entries it made.
fn evaluate_batch(
    c: &mut Connection,
    batch: &[Trigger],
    spikes_due: &[(i64, i64)],
    now: &str,
) -> rusqlite::Result<usize> {
    let tx = c.transaction()?;
    let mut rules_of: HashMap<i64, Vec<Rule>> = HashMap::new();
    let mut queued = 0;
    let mut candidates: Vec<(Rule, i64, Value)> = Vec::new();
    let mut projects: Vec<i64> = batch.iter().map(Trigger::project_id).collect();
    projects.sort_unstable();
    projects.dedup();
    for p in projects {
        let rules = db::enabled_rules(&tx, p)?;
        if !rules.is_empty() {
            rules_of.insert(p, rules);
        }
    }
    if rules_of.is_empty() {
        return Ok(0);
    }
    for t in batch {
        let Some(rules) = rules_of.get(&t.project_id()) else {
            continue;
        };
        for rule in rules {
            let hit = match (t, rule.kind) {
                (
                    Trigger::NewIssue {
                        issue_id,
                        environment,
                        ..
                    },
                    RuleKind::NewIssue,
                ) if env_matches(rule, environment) => {
                    Some((*issue_id, json!({"environment": environment})))
                }
                (
                    Trigger::Regression {
                        issue_id,
                        environment,
                        ..
                    },
                    RuleKind::Regression,
                ) if env_matches(rule, environment) => {
                    Some((*issue_id, json!({"environment": environment})))
                }
                (
                    Trigger::FirstCrash {
                        issue_id,
                        release_id,
                        environment,
                        ..
                    },
                    RuleKind::FirstCrash,
                ) if env_matches(rule, environment) => {
                    let version: Option<String> = tx
                        .prepare_cached("SELECT version FROM releases WHERE id = ?1")?
                        .query_row([release_id], |r| r.get(0))
                        .optional()?;
                    Some((
                        *issue_id,
                        json!({"environment": environment, "crash_release": version}),
                    ))
                }
                (
                    Trigger::Events {
                        project_id,
                        issue_id,
                    },
                    RuleKind::Spike,
                ) if spikes_due.contains(&(*project_id, *issue_id)) => {
                    let check = check_spike(&tx, *issue_id, rule, now)?;
                    check.fires().then(|| {
                        (
                            *issue_id,
                            json!({"spike": {
                                "last_hour": check.last_hour,
                                "limit": check.limit.ceil() as i64,
                                "baseline": (check.baseline * 10.0).round() / 10.0,
                            }}),
                        )
                    })
                }
                _ => None,
            };
            if let Some((issue_id, extra)) = hit
                && !candidates
                    .iter()
                    .any(|(r, i, _)| r.id == rule.id && *i == issue_id)
            {
                candidates.push((rule.clone(), issue_id, extra));
            }
        }
    }
    for (rule, issue_id, extra) in candidates {
        if rule.cooldown_minutes > 0
            && let Some(last) = db::last_fired(&tx, rule.id, issue_id)?
            && last > shift(now, -rule.cooldown_minutes * 60)
        {
            continue;
        }
        let Some((mut data, status)) = issue_data(&tx, issue_id)? else {
            continue;
        };
        if rule.kind == RuleKind::Spike && status != "unresolved" {
            continue;
        }
        data["kind"] = Value::from(rule.kind.as_str());
        if let (Some(extra), Some(obj)) = (extra.as_object(), data.as_object_mut()) {
            for (k, v) in extra {
                obj.insert(k.clone(), v.clone());
            }
        }
        db::set_fired(&tx, rule.id, issue_id, now)?;
        for channel_id in &rule.channel_ids {
            let Some(channel) = db::channel(&tx, *channel_id)? else {
                continue;
            };
            if !channel.active {
                continue;
            }
            let due = match rule.digest_minutes {
                Some(minutes) => Some(
                    db::digest_due(&tx, channel.id)?.unwrap_or_else(|| shift(now, minutes * 60)),
                ),
                None => None,
            };
            db::insert_log(
                &tx,
                &db::NewEntry {
                    project_id: rule.project_id,
                    rule_id: rule.id,
                    rule_name: &rule.name,
                    channel_id: channel.id,
                    channel_name: &channel.name,
                    issue_id: Some(issue_id),
                    kind: rule.kind.as_str(),
                    data: &data,
                    now,
                    digest_due: due.as_deref(),
                },
            )?;
            queued += 1;
        }
    }
    tx.commit()?;
    Ok(queued)
}

// ---------------------------------------------------------------------------
// Delivery
// ---------------------------------------------------------------------------

/// The language a channel's messages are written in.
async fn channel_lang(core: &Core, code: &str) -> Lang {
    match Lang::from_exact_code(code) {
        Some(lang) => lang,
        None => vole_kit::i18n::site_language(core)
            .await
            .unwrap_or_default(),
    }
}

/// One message to send: the log entries it covers, the channel and the
/// alert's data (one entry, or a digest of several).
struct Outgoing {
    ids: Vec<i64>,
    attempts: i64,
    digest: bool,
    channel: Option<db::Channel>,
    entries: Vec<LogEntry>,
}

async fn deliver_due(alerts: &Alerts) -> usize {
    let inner = &alerts.inner;
    let _round = inner.deliver.lock().await;
    let now = crate::issues::now();
    let n2 = now.clone();
    let due = inner
        .core
        .with_conn(move |c| {
            let mut out: Vec<Outgoing> = Vec::new();
            let mut channels: HashMap<i64, Option<db::Channel>> = HashMap::new();
            let mut channel =
                |c: &Connection, id: Option<i64>| -> rusqlite::Result<Option<db::Channel>> {
                    let Some(id) = id else { return Ok(None) };
                    if let Some(ch) = channels.get(&id) {
                        return Ok(ch.clone());
                    }
                    let ch = db::channel(c, id)?;
                    channels.insert(id, ch.clone());
                    Ok(ch)
                };
            for e in db::due_pending(c, &n2, DUE_BATCH)? {
                out.push(Outgoing {
                    ids: vec![e.id],
                    attempts: e.attempts,
                    digest: false,
                    channel: channel(c, e.channel_id)?,
                    entries: vec![e],
                });
            }
            for channel_id in db::due_digest_channels(c, &n2)? {
                let entries = db::digest_entries(c, channel_id)?;
                if entries.is_empty() {
                    continue;
                }
                out.push(Outgoing {
                    ids: entries.iter().map(|e| e.id).collect(),
                    attempts: entries.iter().map(|e| e.attempts).max().unwrap_or(0),
                    digest: true,
                    channel: channel(c, Some(channel_id))?,
                    entries,
                });
            }
            Ok(out)
        })
        .await;
    let due = match due {
        Ok(d) => d,
        Err(err) => {
            tracing::warn!(%err, "can't read the alert queue");
            return 0;
        }
    };
    if due.is_empty() {
        return 0;
    }
    let base = inner.core.base_url().await;
    let mut sent = 0;
    for chunk in due.chunks(inner.settings.concurrency.max(1)) {
        let results = join_all(chunk.iter().map(|o| send_one(alerts, o, base.as_deref()))).await;
        let mut updates: Vec<(Vec<i64>, Result<(), String>, i64)> = Vec::new();
        for (o, result) in chunk.iter().zip(results) {
            match &result {
                Ok(()) => {
                    sent += 1;
                    inner.stats.sent.fetch_add(1, Ordering::Relaxed);
                    tracing::info!(entries = o.ids.len(), digest = o.digest, "alert sent");
                }
                Err(err) => {
                    inner.stats.failed.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(entries = o.ids.len(), %err, "alert not sent");
                }
            }
            updates.push((o.ids.clone(), result, o.attempts));
        }
        let delays = inner.settings.retry_delays.clone();
        let now = now.clone();
        let saved = inner
            .core
            .with_conn(move |c| {
                let tx = c.transaction()?;
                for (ids, result, attempts) in &updates {
                    match result {
                        Ok(()) => db::mark_sent(&tx, ids, &now)?,
                        Err(e) if e == SKIPPED_PAUSED || e == SKIPPED_DELETED => {
                            db::mark_skipped(&tx, ids, e)?;
                        }
                        Err(e) => {
                            let retry = usize::try_from(*attempts)
                                .ok()
                                .and_then(|a| delays.get(a))
                                .map(|d| shift(&now, d.as_secs() as i64));
                            db::mark_failed(&tx, ids, e, retry.as_deref())?;
                        }
                    }
                }
                tx.commit()
            })
            .await;
        if let Err(err) = saved {
            tracing::warn!(%err, "can't note what the alerts did");
        }
    }
    sent
}

const SKIPPED_PAUSED: &str = "channel paused";
const SKIPPED_DELETED: &str = "channel deleted";

async fn send_one(alerts: &Alerts, o: &Outgoing, base: Option<&str>) -> Result<(), String> {
    let Some(channel) = &o.channel else {
        return Err(SKIPPED_DELETED.into());
    };
    if !channel.active {
        return Err(SKIPPED_PAUSED.into());
    }
    let core = &alerts.inner.core;
    let lang = channel_lang(core, &channel.language).await;
    let http = core.http.clone();
    let kit = channel.kit();
    let entries: Vec<(String, Value)> = o
        .entries
        .iter()
        .map(|e| (e.kind.clone(), e.data.clone()))
        .collect();
    let digest = o.digest;
    let base = base.map(str::to_string);
    vole_kit::i18n::scope(lang, async move {
        let alert = if digest {
            message::digest(&entries, base.as_deref())
        } else {
            let (_, data) = &entries[0];
            message::alert(data, base.as_deref())
        };
        vole_kit::notify::send(&http, &kit, &alert).await
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_hour_takes_the_covered_share_of_the_previous_one() {
        assert_eq!(last_hour(10, 60, 0), 70);
        assert_eq!(last_hour(10, 60, 30), 40);
        assert_eq!(last_hour(10, 60, 59), 11);
        assert_eq!(last_hour(0, 0, 15), 0);
    }

    #[test]
    fn the_spike_limit_is_the_threshold_or_the_baseline_times_k() {
        // A quiet issue: the threshold decides.
        assert_eq!(spike_limit(100, 5.0, 168), 100.0);
        // A busy one: 20 an hour on average, five times that is 100.
        assert_eq!(spike_limit(50, 5.0, 20 * 168), 100.0);
        assert_eq!(spike_limit(50, 10.0, 20 * 168), 200.0);
        let check = SpikeCheck {
            last_hour: 100,
            limit: 100.0,
            baseline: 20.0,
        };
        assert!(!check.fires(), "above, not at");
        assert!(
            SpikeCheck {
                last_hour: 101,
                ..check
            }
            .fires()
        );
    }

    #[test]
    fn times_shift() {
        assert_eq!(shift("2026-10-22 12:00:00", -3600), "2026-10-22 11:00:00");
        assert_eq!(shift("2026-10-22 00:10:00", -1200), "2026-10-21 23:50:00");
        assert_eq!(minute_of("2026-10-22 12:34:56"), 34);
    }
}
