//! Notification channels, alert rules, cooldowns and the alert log
//! (docs/design.md §4.7). The channels are the instance's; rules, cooldowns
//! and the log belong to a project.

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde_json::Value;
use vole_kit::notify::alert::NotificationChannel;

// ---------------------------------------------------------------------------
// Channels
// ---------------------------------------------------------------------------

/// A notification channel with the language its messages are written in.
#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    pub id: i64,
    pub name: String,
    pub kind: String,
    /// JSON with the provider's settings, secrets included: never shown
    /// unredacted.
    pub config: String,
    pub active: bool,
    /// A language code, or "" for the instance's language.
    pub language: String,
    pub created_at: String,
}

impl Channel {
    /// The kit's view of it, for `vole_kit::notify::send`.
    pub fn kit(&self) -> NotificationChannel {
        NotificationChannel {
            id: self.id,
            name: self.name.clone(),
            kind: self.kind.clone(),
            config: self.config.clone(),
            active: self.active,
            is_default: false,
            created_at: self.created_at.clone(),
        }
    }

    pub fn config_json(&self) -> Value {
        serde_json::from_str(&self.config).unwrap_or_else(|_| Value::Object(Default::default()))
    }
}

const CHANNEL_COLUMNS: &str = "id, name, kind, config, active, language, created_at";

fn channel_row(r: &Row<'_>) -> rusqlite::Result<Channel> {
    Ok(Channel {
        id: r.get(0)?,
        name: r.get(1)?,
        kind: r.get(2)?,
        config: r.get(3)?,
        active: r.get(4)?,
        language: r.get(5)?,
        created_at: r.get(6)?,
    })
}

/// Every channel with the number of rules that use it, by name.
pub fn channels_with_counts(c: &Connection) -> rusqlite::Result<Vec<(Channel, i64)>> {
    c.prepare(&format!(
        "SELECT {CHANNEL_COLUMNS}, (SELECT COUNT(*) FROM alert_rule_channels rc WHERE rc.channel_id = notification_channels.id)
         FROM notification_channels ORDER BY name COLLATE NOCASE, id"
    ))?
    .query_map([], |r| Ok((channel_row(r)?, r.get(7)?)))?
    .collect()
}

pub fn channels(c: &Connection) -> rusqlite::Result<Vec<Channel>> {
    c.prepare(&format!(
        "SELECT {CHANNEL_COLUMNS} FROM notification_channels ORDER BY name COLLATE NOCASE, id"
    ))?
    .query_map([], channel_row)?
    .collect()
}

pub fn channel(c: &Connection, id: i64) -> rusqlite::Result<Option<Channel>> {
    c.query_row(
        &format!("SELECT {CHANNEL_COLUMNS} FROM notification_channels WHERE id = ?1"),
        [id],
        channel_row,
    )
    .optional()
}

pub fn insert_channel(
    c: &Connection,
    name: &str,
    kind: &str,
    config: &str,
    language: &str,
) -> rusqlite::Result<i64> {
    c.execute(
        "INSERT INTO notification_channels (name, kind, config, language) VALUES (?1, ?2, ?3, ?4)",
        params![name, kind, config, language],
    )?;
    Ok(c.last_insert_rowid())
}

/// The kind of the Hedgerow app channel. Only one can be active: it reaches
/// every device registered with the server, so a second one would push the
/// same alert twice.
pub const HEDGEROW: &str = "hedgerow";

/// The name of an active `hedgerow` channel other than `except`, if there is
/// one.
pub fn other_active_hedgerow(
    c: &Connection,
    except: Option<i64>,
) -> rusqlite::Result<Option<String>> {
    c.query_row(
        "SELECT name FROM notification_channels
         WHERE kind = ?1 AND active = 1 AND id IS NOT ?2
         ORDER BY id LIMIT 1",
        params![HEDGEROW, except],
        |r| r.get(0),
    )
    .optional()
}

pub fn update_channel(
    c: &Connection,
    id: i64,
    name: &str,
    config: &str,
    active: bool,
    language: &str,
) -> rusqlite::Result<bool> {
    Ok(c.execute(
        "UPDATE notification_channels SET name = ?2, config = ?3, active = ?4, language = ?5 WHERE id = ?1",
        params![id, name, config, active, language],
    )? > 0)
}

pub fn set_channel_active(c: &Connection, id: i64, active: bool) -> rusqlite::Result<bool> {
    Ok(c.execute(
        "UPDATE notification_channels SET active = ?2 WHERE id = ?1",
        params![id, active],
    )? > 0)
}

/// Deletes a channel; alerts waiting for it are skipped.
pub fn delete_channel(c: &Connection, id: i64) -> rusqlite::Result<bool> {
    c.execute(
        "UPDATE alert_log SET status = 'skipped', error = 'channel deleted'
         WHERE channel_id = ?1 AND status IN ('pending', 'digest')",
        [id],
    )?;
    Ok(c.execute("DELETE FROM notification_channels WHERE id = ?1", [id])? > 0)
}

// ---------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------

/// What makes a rule fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleKind {
    NewIssue,
    Regression,
    Spike,
    FirstCrash,
}

impl RuleKind {
    pub const ALL: [RuleKind; 4] = [
        RuleKind::NewIssue,
        RuleKind::Regression,
        RuleKind::Spike,
        RuleKind::FirstCrash,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            RuleKind::NewIssue => "new_issue",
            RuleKind::Regression => "regression",
            RuleKind::Spike => "spike",
            RuleKind::FirstCrash => "first_crash",
        }
    }

    pub fn parse(s: &str) -> Option<RuleKind> {
        RuleKind::ALL.into_iter().find(|k| k.as_str() == s.trim())
    }
}

/// An alert rule of a project.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub kind: RuleKind,
    pub enabled: bool,
    /// "" for any environment (not used by spikes).
    pub environment: String,
    /// Spikes: at least this many events in an hour …
    pub threshold: i64,
    /// … and more than this many times the hourly average of the 7 days before.
    pub multiplier: f64,
    pub cooldown_minutes: i64,
    /// `None`: one message per alert.
    pub digest_minutes: Option<i64>,
    pub channel_ids: Vec<i64>,
    pub created_at: String,
    pub updated_at: String,
}

/// A rule's settings as a form or the API sends them.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleInput {
    pub name: String,
    pub kind: RuleKind,
    pub enabled: bool,
    pub environment: String,
    pub threshold: i64,
    pub multiplier: f64,
    pub cooldown_minutes: i64,
    pub digest_minutes: Option<i64>,
    pub channel_ids: Vec<i64>,
}

impl RuleInput {
    pub fn from_rule(r: &Rule) -> RuleInput {
        RuleInput {
            name: r.name.clone(),
            kind: r.kind,
            enabled: r.enabled,
            environment: r.environment.clone(),
            threshold: r.threshold,
            multiplier: r.multiplier,
            cooldown_minutes: r.cooldown_minutes,
            digest_minutes: r.digest_minutes,
            channel_ids: r.channel_ids.clone(),
        }
    }

    /// A new rule's defaults.
    pub fn new(kind: RuleKind) -> RuleInput {
        RuleInput {
            name: String::new(),
            kind,
            enabled: true,
            environment: String::new(),
            threshold: 100,
            multiplier: 5.0,
            cooldown_minutes: 60,
            digest_minutes: None,
            channel_ids: Vec::new(),
        }
    }
}

const RULE_COLUMNS: &str =
    "id, project_id, name, kind, enabled, environment, threshold, multiplier,
    cooldown_minutes, digest_minutes, created_at, updated_at";

fn rule_row(r: &Row<'_>) -> rusqlite::Result<Rule> {
    let kind: String = r.get(3)?;
    Ok(Rule {
        id: r.get(0)?,
        project_id: r.get(1)?,
        name: r.get(2)?,
        kind: RuleKind::parse(&kind).unwrap_or(RuleKind::NewIssue),
        enabled: r.get(4)?,
        environment: r.get(5)?,
        threshold: r.get(6)?,
        multiplier: r.get(7)?,
        cooldown_minutes: r.get(8)?,
        digest_minutes: r.get(9)?,
        channel_ids: Vec::new(),
        created_at: r.get(10)?,
        updated_at: r.get(11)?,
    })
}

fn with_channels(c: &Connection, mut rules: Vec<Rule>) -> rusqlite::Result<Vec<Rule>> {
    let mut stmt = c.prepare_cached(
        "SELECT channel_id FROM alert_rule_channels WHERE rule_id = ?1 ORDER BY channel_id",
    )?;
    for rule in &mut rules {
        rule.channel_ids = stmt
            .query_map([rule.id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
    }
    Ok(rules)
}

/// A project's rules, oldest first.
pub fn rules(c: &Connection, project_id: i64) -> rusqlite::Result<Vec<Rule>> {
    let rules = c
        .prepare_cached(&format!(
            "SELECT {RULE_COLUMNS} FROM alert_rules WHERE project_id = ?1 ORDER BY id"
        ))?
        .query_map([project_id], rule_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    with_channels(c, rules)
}

/// A project's enabled rules (for the alerts).
pub fn enabled_rules(c: &Connection, project_id: i64) -> rusqlite::Result<Vec<Rule>> {
    let rules = c
        .prepare_cached(&format!(
            "SELECT {RULE_COLUMNS} FROM alert_rules WHERE project_id = ?1 AND enabled = 1 ORDER BY id"
        ))?
        .query_map([project_id], rule_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    with_channels(c, rules)
}

pub fn rule(c: &Connection, project_id: i64, id: i64) -> rusqlite::Result<Option<Rule>> {
    let rule = c
        .query_row(
            &format!("SELECT {RULE_COLUMNS} FROM alert_rules WHERE project_id = ?1 AND id = ?2"),
            [project_id, id],
            rule_row,
        )
        .optional()?;
    Ok(match rule {
        Some(r) => with_channels(c, vec![r])?.pop(),
        None => None,
    })
}

/// The channel ids among `ids` that exist.
fn existing_channels(c: &Connection, ids: &[i64]) -> rusqlite::Result<Vec<i64>> {
    let mut stmt =
        c.prepare_cached("SELECT EXISTS (SELECT 1 FROM notification_channels WHERE id = ?1)")?;
    let mut out = Vec::new();
    for id in ids {
        if !out.contains(id) && stmt.query_row([id], |r| r.get::<_, bool>(0))? {
            out.push(*id);
        }
    }
    Ok(out)
}

fn set_channels(c: &Connection, rule_id: i64, ids: &[i64]) -> rusqlite::Result<()> {
    c.execute(
        "DELETE FROM alert_rule_channels WHERE rule_id = ?1",
        [rule_id],
    )?;
    for id in existing_channels(c, ids)? {
        c.execute(
            "INSERT INTO alert_rule_channels (rule_id, channel_id) VALUES (?1, ?2)",
            [rule_id, id],
        )?;
    }
    Ok(())
}

pub fn insert_rule(c: &Connection, project_id: i64, r: &RuleInput) -> rusqlite::Result<i64> {
    c.execute(
        "INSERT INTO alert_rules (project_id, name, kind, enabled, environment, threshold, multiplier,
             cooldown_minutes, digest_minutes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            project_id,
            r.name,
            r.kind.as_str(),
            r.enabled,
            r.environment,
            r.threshold,
            r.multiplier,
            r.cooldown_minutes,
            r.digest_minutes
        ],
    )?;
    let id = c.last_insert_rowid();
    set_channels(c, id, &r.channel_ids)?;
    Ok(id)
}

pub fn update_rule(
    c: &Connection,
    project_id: i64,
    id: i64,
    r: &RuleInput,
) -> rusqlite::Result<bool> {
    let n = c.execute(
        "UPDATE alert_rules SET name = ?3, kind = ?4, enabled = ?5, environment = ?6, threshold = ?7,
             multiplier = ?8, cooldown_minutes = ?9, digest_minutes = ?10, updated_at = datetime('now')
         WHERE project_id = ?1 AND id = ?2",
        params![
            project_id,
            id,
            r.name,
            r.kind.as_str(),
            r.enabled,
            r.environment,
            r.threshold,
            r.multiplier,
            r.cooldown_minutes,
            r.digest_minutes
        ],
    )?;
    if n == 0 {
        return Ok(false);
    }
    set_channels(c, id, &r.channel_ids)?;
    Ok(true)
}

pub fn delete_rule(c: &Connection, project_id: i64, id: i64) -> rusqlite::Result<bool> {
    c.execute(
        "UPDATE alert_log SET status = 'skipped', error = 'rule deleted'
         WHERE rule_id = ?1 AND project_id = ?2 AND status IN ('pending', 'digest')",
        [id, project_id],
    )?;
    Ok(c.execute(
        "DELETE FROM alert_rules WHERE project_id = ?1 AND id = ?2",
        [project_id, id],
    )? > 0)
}

// ---------------------------------------------------------------------------
// Cooldowns
// ---------------------------------------------------------------------------

/// When `rule_id` last alerted for `issue_id`.
pub fn last_fired(c: &Connection, rule_id: i64, issue_id: i64) -> rusqlite::Result<Option<String>> {
    c.prepare_cached("SELECT fired_at FROM alert_cooldowns WHERE rule_id = ?1 AND issue_id = ?2")?
        .query_row([rule_id, issue_id], |r| r.get(0))
        .optional()
}

pub fn set_fired(c: &Connection, rule_id: i64, issue_id: i64, at: &str) -> rusqlite::Result<()> {
    c.prepare_cached(
        "INSERT INTO alert_cooldowns (rule_id, issue_id, fired_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (rule_id, issue_id) DO UPDATE SET fired_at = excluded.fired_at",
    )?
    .execute(params![rule_id, issue_id, at])?;
    Ok(())
}

// ---------------------------------------------------------------------------
// The log
// ---------------------------------------------------------------------------

/// One alert for one channel.
#[derive(Debug, Clone, PartialEq)]
pub struct LogEntry {
    pub id: i64,
    pub project_id: i64,
    pub rule_id: Option<i64>,
    pub rule_name: String,
    pub channel_id: Option<i64>,
    pub channel_name: String,
    pub issue_id: Option<i64>,
    pub kind: String,
    pub data: Value,
    pub status: String,
    pub attempts: i64,
    pub error: Option<String>,
    pub next_attempt_at: Option<String>,
    pub due_at: Option<String>,
    pub created_at: String,
    pub sent_at: Option<String>,
}

const LOG_COLUMNS: &str = "id, project_id, rule_id, rule_name, channel_id, channel_name, issue_id,
    kind, data, status, attempts, error, next_attempt_at, due_at, created_at, sent_at";

fn log_row(r: &Row<'_>) -> rusqlite::Result<LogEntry> {
    let data: String = r.get(8)?;
    Ok(LogEntry {
        id: r.get(0)?,
        project_id: r.get(1)?,
        rule_id: r.get(2)?,
        rule_name: r.get(3)?,
        channel_id: r.get(4)?,
        channel_name: r.get(5)?,
        issue_id: r.get(6)?,
        kind: r.get(7)?,
        data: serde_json::from_str(&data).unwrap_or(Value::Null),
        status: r.get(9)?,
        attempts: r.get(10)?,
        error: r.get(11)?,
        next_attempt_at: r.get(12)?,
        due_at: r.get(13)?,
        created_at: r.get(14)?,
        sent_at: r.get(15)?,
    })
}

/// A new alert for a channel: sent at once (`pending`) or with the
/// channel's next digest (`digest`, at `due_at`).
pub struct NewEntry<'a> {
    pub project_id: i64,
    pub rule_id: i64,
    pub rule_name: &'a str,
    pub channel_id: i64,
    pub channel_name: &'a str,
    pub issue_id: Option<i64>,
    pub kind: &'a str,
    pub data: &'a Value,
    pub now: &'a str,
    /// `Some(due_at)` for a digest.
    pub digest_due: Option<&'a str>,
}

pub fn insert_log(c: &Connection, e: &NewEntry<'_>) -> rusqlite::Result<i64> {
    let (status, next, due) = match e.digest_due {
        Some(due) => ("digest", None, Some(due)),
        None => ("pending", Some(e.now), None),
    };
    c.prepare_cached(
        "INSERT INTO alert_log (project_id, rule_id, rule_name, channel_id, channel_name, issue_id,
             kind, data, status, next_attempt_at, due_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
    )?
    .execute(params![
        e.project_id,
        e.rule_id,
        e.rule_name,
        e.channel_id,
        e.channel_name,
        e.issue_id,
        e.kind,
        e.data.to_string(),
        status,
        next,
        due,
        e.now
    ])?;
    Ok(c.last_insert_rowid())
}

/// When the channel's waiting digest goes out, if one is waiting.
pub fn digest_due(c: &Connection, channel_id: i64) -> rusqlite::Result<Option<String>> {
    c.prepare_cached(
        "SELECT MIN(due_at) FROM alert_log WHERE channel_id = ?1 AND status = 'digest'",
    )?
    .query_row([channel_id], |r| r.get(0))
}

/// Alerts to send now: pending ones whose time has come, oldest first.
pub fn due_pending(c: &Connection, now: &str, limit: i64) -> rusqlite::Result<Vec<LogEntry>> {
    c.prepare_cached(&format!(
        "SELECT {LOG_COLUMNS} FROM alert_log
         WHERE status = 'pending' AND next_attempt_at <= ?1 ORDER BY id LIMIT ?2"
    ))?
    .query_map(params![now, limit], log_row)?
    .collect()
}

/// Channels whose digest is due.
pub fn due_digest_channels(c: &Connection, now: &str) -> rusqlite::Result<Vec<i64>> {
    c.prepare_cached(
        "SELECT channel_id FROM alert_log WHERE status = 'digest' AND channel_id IS NOT NULL
         GROUP BY channel_id HAVING MIN(due_at) <= ?1",
    )?
    .query_map([now], |r| r.get(0))?
    .collect()
}

/// A channel's waiting digest entries, oldest first.
pub fn digest_entries(c: &Connection, channel_id: i64) -> rusqlite::Result<Vec<LogEntry>> {
    c.prepare_cached(&format!(
        "SELECT {LOG_COLUMNS} FROM alert_log WHERE status = 'digest' AND channel_id = ?1 ORDER BY id LIMIT 500"
    ))?
    .query_map([channel_id], log_row)?
    .collect()
}

/// Marks entries sent.
pub fn mark_sent(c: &Connection, ids: &[i64], now: &str) -> rusqlite::Result<()> {
    let mut stmt = c.prepare_cached(
        "UPDATE alert_log SET status = 'sent', attempts = attempts + 1, error = NULL, sent_at = ?2,
             next_attempt_at = NULL WHERE id = ?1",
    )?;
    for id in ids {
        stmt.execute(params![id, now])?;
    }
    Ok(())
}

/// A failed attempt: tried again at `retry_at` (keeping the entries' status),
/// or failed for good without one.
pub fn mark_failed(
    c: &Connection,
    ids: &[i64],
    error: &str,
    retry_at: Option<&str>,
) -> rusqlite::Result<()> {
    let mut stmt = c.prepare_cached(
        "UPDATE alert_log SET attempts = attempts + 1, error = ?2,
             status = CASE WHEN ?3 IS NULL THEN 'failed' ELSE status END,
             next_attempt_at = CASE WHEN status = 'pending' THEN ?3 ELSE next_attempt_at END,
             due_at = CASE WHEN status = 'digest' THEN coalesce(?3, due_at) ELSE due_at END
         WHERE id = ?1",
    )?;
    for id in ids {
        stmt.execute(params![id, error, retry_at])?;
    }
    Ok(())
}

/// Entries that won't be sent (`why`: the channel was paused or deleted).
pub fn mark_skipped(c: &Connection, ids: &[i64], why: &str) -> rusqlite::Result<()> {
    let mut stmt = c.prepare_cached(
        "UPDATE alert_log SET status = 'skipped', error = ?2, next_attempt_at = NULL WHERE id = ?1",
    )?;
    for id in ids {
        stmt.execute(params![id, why])?;
    }
    Ok(())
}

/// A project's alert history, newest first.
pub fn history(
    c: &Connection,
    project_id: i64,
    limit: i64,
    offset: i64,
) -> rusqlite::Result<(Vec<LogEntry>, i64)> {
    let total: i64 = c.query_row(
        "SELECT COUNT(*) FROM alert_log WHERE project_id = ?1",
        [project_id],
        |r| r.get(0),
    )?;
    let rows = c
        .prepare_cached(&format!(
            "SELECT {LOG_COLUMNS} FROM alert_log WHERE project_id = ?1 ORDER BY id DESC LIMIT ?2 OFFSET ?3"
        ))?
        .query_map(params![project_id, limit, offset], log_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok((rows, total))
}

/// Removes history older than `cutoff` (sent, failed or skipped entries).
pub fn prune_log(c: &Connection, cutoff: &str) -> rusqlite::Result<usize> {
    c.execute(
        "DELETE FROM alert_log WHERE status IN ('sent', 'failed', 'skipped') AND created_at < ?1",
        [cutoff],
    )
}

/// Removes cooldowns older than `cutoff` (longer ago than any cooldown).
pub fn prune_cooldowns(c: &Connection, cutoff: &str) -> rusqlite::Result<usize> {
    c.execute("DELETE FROM alert_cooldowns WHERE fired_at < ?1", [cutoff])
}
