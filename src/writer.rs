//! The one writer (docs/design.md §4.1): ingest hands it jobs through a
//! bounded channel, and it writes them in batches, each in one IMMEDIATE
//! transaction, every 100 ms or every 500 jobs, whichever comes first.
//! When the channel is full, ingest answers 429 instead of waiting.
//!
//! The writer has its own connection to `kestrel.sqlite` with `store.sqlite`
//! attached as `store`, so an event's index row and its payload are written
//! in the same transaction. Each job runs in a savepoint: one bad job (a
//! project deleted meanwhile, say) doesn't take the batch with it.
//!
//! The issue pages' actions (resolve, merge, delete …) and the retention job
//! run on the same connection through [`Writer::run`], after the jobs queued
//! before them, so they never race ingest. After a batch with events, and
//! after an action, the writer tells the live issue lists which project
//! changed ([`Writer::subscribe`]), and hands what may alert (new issues,
//! regressions, first crashes, issues with events) to the alerts, without
//! waiting for them ([`crate::alerts::Sink`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use rusqlite::{Connection, TransactionBehavior, params};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::alerts::{Sink, Trigger};
use crate::db::issues::{self, EventRecord, Outcome, Written};

/// A session update or aggregate (release health, later).
#[derive(Debug, Clone)]
pub struct SessionItem {
    pub project_id: i64,
    pub received_at: String,
    pub release: String,
    pub environment: String,
    /// JSON, without IP address or user agent, with the device id hashed.
    pub data: String,
}

/// A user report or feedback item.
#[derive(Debug, Clone)]
pub struct FeedbackItem {
    pub project_id: i64,
    pub event_id: Option<String>,
    pub received_at: String,
    pub data: String,
}

/// An attachment of an event; kept only when that event is stored.
#[derive(Debug, Clone)]
pub struct AttachmentItem {
    pub project_id: i64,
    pub event_id: String,
    pub received_at: String,
    pub filename: String,
    pub content_type: Option<String>,
    pub attachment_type: String,
    pub size: i64,
    /// zstd-compressed.
    pub data: Vec<u8>,
}

/// Debug files an event was symbolicated with: their `last_used_at` (and
/// the release that used them).
#[derive(Debug, Clone)]
pub struct DifUse {
    pub project_id: i64,
    pub debug_ids: Vec<String>,
    pub release: Option<String>,
    pub at: String,
}

#[derive(Debug)]
pub enum Job {
    Event(Box<EventRecord>),
    Session(SessionItem),
    Feedback(FeedbackItem),
    Attachment(Box<AttachmentItem>),
    DifUse(DifUse),
    /// A pageview or custom event from k.js (docs/design.md §5).
    Web(Box<crate::analytics::WebEvent>),
    /// One request's app signals (docs/design.md §5): one job, so a batch of
    /// 100 takes one place in the queue.
    App(Vec<crate::analytics::app::AppSignal>),
}

/// Work for the writer's connection besides ingest jobs.
type Exec = Box<dyn FnOnce(&mut Connection) + Send>;

enum Msg {
    Job(Job),
    Flush(oneshot::Sender<()>),
    Exec(Exec),
}

/// Why [`Writer::run`] failed.
#[derive(Debug)]
pub enum WriterError {
    /// The writer has stopped (or the work panicked).
    Stopped,
    Db(rusqlite::Error),
}

impl std::fmt::Display for WriterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriterError::Stopped => f.write_str("the writer has stopped"),
            WriterError::Db(e) => write!(f, "database: {e}"),
        }
    }
}

impl std::error::Error for WriterError {}

/// How the writer batches.
#[derive(Debug, Clone, Copy)]
pub struct WriterSettings {
    /// Jobs waiting at most; more makes ingest answer 429.
    pub queue_capacity: usize,
    pub batch_size: usize,
    pub batch_interval: Duration,
}

impl Default for WriterSettings {
    fn default() -> Self {
        WriterSettings {
            queue_capacity: 2000,
            batch_size: 500,
            batch_interval: Duration::from_millis(100),
        }
    }
}

/// Counters, for tests and logs.
#[derive(Debug, Default)]
pub struct Stats {
    pub events: AtomicU64,
    pub new_issues: AtomicU64,
    pub duplicates: AtomicU64,
    pub other_items: AtomicU64,
    pub failed: AtomicU64,
    pub batches: AtomicU64,
}

/// The queue is full: answer 429.
#[derive(Debug, PartialEq, Eq)]
pub struct QueueFull;

/// The handle ingest submits jobs through.
#[derive(Clone)]
pub struct Writer {
    tx: mpsc::Sender<Msg>,
    stats: Arc<Stats>,
    changes: broadcast::Sender<i64>,
    web: broadcast::Sender<WebChange>,
}

/// What changed in a project's analytics (web or app): new raw events
/// (current visitors and users) or new rollups (the dashboards' numbers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebChange {
    Events(i64),
    Rollup(i64),
}

impl Writer {
    /// Opens the writer's connection and starts its task. `alerts` gets
    /// what may alert after each committed batch.
    pub fn start(
        main: &Path,
        store: &Path,
        settings: WriterSettings,
        alerts: Option<Sink>,
    ) -> Result<Writer, crate::StartError> {
        let conn = open(main, store)?;
        let (tx, rx) = mpsc::channel(settings.queue_capacity.max(1));
        let stats = Arc::new(Stats::default());
        let (changes, _) = broadcast::channel(64);
        let (web, _) = broadcast::channel(64);
        let paths = (main.to_path_buf(), store.to_path_buf());
        tokio::spawn(run(
            rx,
            conn,
            paths,
            settings,
            stats.clone(),
            (changes.clone(), web.clone()),
            alerts,
        ));
        Ok(Writer {
            tx,
            stats,
            changes,
            web,
        })
    }

    /// Queues `jobs`, all or none.
    pub fn submit(&self, jobs: Vec<Job>) -> Result<(), QueueFull> {
        if jobs.is_empty() {
            return Ok(());
        }
        let permits = self
            .tx
            .try_reserve_many(jobs.len())
            .map_err(|_| QueueFull)?;
        for (permit, job) in permits.zip(jobs) {
            permit.send(Msg::Job(job));
        }
        Ok(())
    }

    /// Queues `jobs`, all or none, waiting while the queue is full (for the
    /// symbolication workers, whose SDK already has its answer).
    pub async fn submit_wait(&self, jobs: Vec<Job>) -> Result<(), WriterError> {
        if jobs.is_empty() {
            return Ok(());
        }
        let permits = self
            .tx
            .reserve_many(jobs.len())
            .await
            .map_err(|_| WriterError::Stopped)?;
        for (permit, job) in permits.zip(jobs) {
            permit.send(Msg::Job(job));
        }
        Ok(())
    }

    /// Waits until everything queued before this call is written.
    pub async fn flush(&self) {
        let (tx, rx) = oneshot::channel();
        if self.tx.send(Msg::Flush(tx)).await.is_ok() {
            let _ = rx.await;
        }
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Runs `f` on the writer's connection (with `store.sqlite` attached as
    /// `store`) after the jobs queued before it, and returns its result.
    /// `f` makes its own transaction.
    pub async fn run<T, F>(&self, f: F) -> Result<T, WriterError>
    where
        F: FnOnce(&mut Connection) -> rusqlite::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let exec: Exec = Box::new(move |c| {
            let _ = tx.send(f(c));
        });
        self.tx
            .send(Msg::Exec(exec))
            .await
            .map_err(|_| WriterError::Stopped)?;
        rx.await
            .map_err(|_| WriterError::Stopped)?
            .map_err(WriterError::Db)
    }

    /// Project ids whose issues changed (events written, an action taken).
    pub fn subscribe(&self) -> broadcast::Receiver<i64> {
        self.changes.subscribe()
    }

    /// Tells the live lists that `project_id`'s issues changed.
    pub fn notify(&self, project_id: i64) {
        let _ = self.changes.send(project_id);
    }

    /// Changes to projects' web analytics (the Web tab's live updates).
    pub fn subscribe_web(&self) -> broadcast::Receiver<WebChange> {
        self.web.subscribe()
    }

    /// Tells the Web tabs that something changed.
    pub fn notify_web(&self, change: WebChange) {
        let _ = self.web.send(change);
    }
}

fn open(main: &Path, store: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(main)?;
    conn.execute_batch(vole_kit::db::CONNECTION_PRAGMAS)?;
    conn.execute("ATTACH DATABASE ?1 AS store", [store.to_string_lossy()])?;
    conn.execute_batch(
        "PRAGMA store.synchronous = NORMAL;
         PRAGMA store.journal_size_limit = 67108864;",
    )?;
    Ok(conn)
}

async fn run(
    mut rx: mpsc::Receiver<Msg>,
    conn: Connection,
    paths: (PathBuf, PathBuf),
    settings: WriterSettings,
    stats: Arc<Stats>,
    (changes, web): (broadcast::Sender<i64>, broadcast::Sender<WebChange>),
    alerts: Option<Sink>,
) {
    let mut conn = Some(conn);
    while let Some(first) = rx.recv().await {
        let mut batch = vec![first];
        let deadline = tokio::time::Instant::now() + settings.batch_interval;
        while batch.len() < settings.batch_size
            && !matches!(batch.last(), Some(Msg::Flush(_) | Msg::Exec(_)))
        {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Some(msg)) => batch.push(msg),
                Ok(None) | Err(_) => break,
            }
        }
        let c = match conn.take() {
            Some(c) => c,
            None => match open(&paths.0, &paths.1) {
                Ok(c) => c,
                Err(err) => {
                    tracing::error!(%err, "the writer can't open the database; dropping a batch");
                    stats
                        .failed
                        .fetch_add(batch.len() as u64, Ordering::Relaxed);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            },
        };
        let stats2 = stats.clone();
        match tokio::task::spawn_blocking(move || {
            let mut c = c;
            let (acks, projects, triggers) = write_batch(&mut c, batch, &stats2);
            (c, acks, projects, triggers)
        })
        .await
        {
            Ok((c, acks, (projects, web_projects), triggers)) => {
                conn = Some(c);
                for project in projects {
                    let _ = changes.send(project);
                }
                for project in web_projects {
                    let _ = web.send(WebChange::Events(project));
                }
                if let Some(alerts) = &alerts {
                    alerts.send(triggers);
                }
                for ack in acks {
                    let _ = ack.send(());
                }
            }
            Err(err) => tracing::error!(%err, "the writer's batch panicked"),
        }
    }
}

/// What a written batch hands on: the flush acks, the projects that got
/// events and those that got web events, and what may alert.
type BatchOut = (Vec<oneshot::Sender<()>>, (Vec<i64>, Vec<i64>), Vec<Trigger>);

/// Writes a batch's jobs, then runs its other work.
fn write_batch(c: &mut Connection, batch: Vec<Msg>, stats: &Stats) -> BatchOut {
    let mut acks = Vec::new();
    let mut execs = Vec::new();
    let mut jobs = Vec::with_capacity(batch.len());
    for msg in batch {
        match msg {
            Msg::Job(job) => jobs.push(job),
            Msg::Flush(ack) => acks.push(ack),
            Msg::Exec(f) => execs.push(f),
        }
    }
    let mut projects: Vec<i64> = Vec::new();
    let mut web_projects: Vec<i64> = Vec::new();
    for job in &jobs {
        match job {
            Job::Event(e) if !projects.contains(&e.project_id) => projects.push(e.project_id),
            Job::Web(e) if !web_projects.contains(&e.project_id) => {
                web_projects.push(e.project_id);
            }
            Job::App(signals) => {
                for s in signals {
                    if !web_projects.contains(&s.project_id) {
                        web_projects.push(s.project_id);
                    }
                }
            }
            _ => {}
        }
    }
    let triggers = if jobs.is_empty() {
        Vec::new()
    } else {
        write_all(c, &mut jobs, stats)
    };
    for f in execs {
        f(c);
    }
    (acks, (projects, web_projects), triggers)
}

/// Writes the jobs (trying once more if the transaction fails). Returns
/// what may alert, from the transaction that committed.
fn write_all(c: &mut Connection, jobs: &mut [Job], stats: &Stats) -> Vec<Trigger> {
    // Events first: attachments need their event's row.
    jobs.sort_by_key(|j| match j {
        Job::Event(_) => 0,
        Job::Attachment(_) => 2,
        _ => 1,
    });
    let mut result = write_jobs(c, jobs, stats);
    if let Err(err) = &result {
        tracing::warn!(%err, jobs = jobs.len(), "a batch failed; trying once more");
        result = write_jobs(c, jobs, stats);
    }
    match result {
        Ok(triggers) => {
            stats.batches.fetch_add(1, Ordering::Relaxed);
            triggers
        }
        Err(err) => {
            tracing::error!(%err, jobs = jobs.len(), "a batch could not be written");
            stats.failed.fetch_add(jobs.len() as u64, Ordering::Relaxed);
            Vec::new()
        }
    }
}

/// What an event that was written may alert.
fn triggers_of(e: &EventRecord, w: Written, out: &mut Vec<Trigger>) {
    let (issue_id, new, regressed) = match w.outcome {
        Outcome::NewIssue(id) => (id, true, false),
        Outcome::Existing(id, regressed) => (id, false, regressed),
        Outcome::Duplicate => return,
    };
    let project_id = e.project_id;
    if new {
        out.push(Trigger::NewIssue {
            project_id,
            issue_id,
            environment: e.environment.clone(),
        });
    }
    if regressed {
        out.push(Trigger::Regression {
            project_id,
            issue_id,
            environment: e.environment.clone(),
        });
    }
    if let Some(release_id) = w.first_crash {
        out.push(Trigger::FirstCrash {
            project_id,
            issue_id,
            release_id,
            environment: e.environment.clone(),
        });
    }
    let events = Trigger::Events {
        project_id,
        issue_id,
    };
    if !out.contains(&events) {
        out.push(events);
    }
}

/// Counts of one transaction, added to [`Stats`] once it committed.
#[derive(Default)]
struct Counts {
    events: u64,
    new_issues: u64,
    duplicates: u64,
    other: u64,
    failed: u64,
}

fn write_jobs(c: &mut Connection, jobs: &[Job], stats: &Stats) -> rusqlite::Result<Vec<Trigger>> {
    let mut tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut counts = Counts::default();
    let mut triggers = Vec::new();
    for job in jobs {
        let sp = tx.savepoint()?;
        match write_job(&sp, job) {
            Ok(written) => {
                sp.commit()?;
                if let (Job::Event(e), Some(w)) = (job, written) {
                    triggers_of(e, w, &mut triggers);
                }
                match written.map(|w| w.outcome) {
                    Some(Outcome::NewIssue(_)) => {
                        counts.events += 1;
                        counts.new_issues += 1;
                    }
                    Some(Outcome::Existing(..)) => counts.events += 1,
                    Some(Outcome::Duplicate) => counts.duplicates += 1,
                    None => counts.other += 1,
                }
            }
            Err(err) => {
                tracing::warn!(%err, "skipped one ingest job");
                counts.failed += 1;
            }
        }
    }
    tx.commit()?;
    stats.events.fetch_add(counts.events, Ordering::Relaxed);
    stats
        .new_issues
        .fetch_add(counts.new_issues, Ordering::Relaxed);
    stats
        .duplicates
        .fetch_add(counts.duplicates, Ordering::Relaxed);
    stats.other_items.fetch_add(counts.other, Ordering::Relaxed);
    stats.failed.fetch_add(counts.failed, Ordering::Relaxed);
    Ok(triggers)
}

fn write_job(c: &Connection, job: &Job) -> rusqlite::Result<Option<Written>> {
    match job {
        Job::Event(e) => issues::write_event_full(c, e).map(Some),
        Job::Session(s) => {
            c.prepare_cached(
                "INSERT INTO store.session_items (project_id, received_at, release, environment, data)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?
            .execute(params![s.project_id, s.received_at, s.release, s.environment, s.data])?;
            Ok(None)
        }
        Job::Feedback(f) => {
            c.prepare_cached(
                "INSERT INTO store.feedback (project_id, event_id, received_at, data) VALUES (?1, ?2, ?3, ?4)",
            )?
            .execute(params![f.project_id, f.event_id, f.received_at, f.data])?;
            Ok(None)
        }
        Job::DifUse(u) => {
            for id in &u.debug_ids {
                crate::db::difs::mark_used(c, u.project_id, id, u.release.as_deref(), &u.at)?;
            }
            Ok(None)
        }
        Job::Web(e) => {
            crate::analytics::rollup::insert_event(c, e)?;
            Ok(None)
        }
        Job::App(signals) => {
            for s in signals {
                crate::analytics::app::insert(c, s)?;
            }
            Ok(None)
        }
        Job::Attachment(a) => {
            if issues::event_stored(c, a.project_id, &a.event_id)? {
                c.prepare_cached(
                    "INSERT INTO store.attachments (project_id, event_id, received_at, filename,
                         content_type, attachment_type, size, data)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                )?
                .execute(params![
                    a.project_id,
                    a.event_id,
                    a.received_at,
                    a.filename,
                    a.content_type,
                    a.attachment_type,
                    a.size,
                    a.data
                ])?;
            }
            Ok(None)
        }
    }
}
