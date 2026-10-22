//! Retention (docs/design.md §4.6), in the family's pattern: a background
//! job that first runs a minute after start and then once a day, deleting
//! in small batches so ingest can write in between.
//!
//! Each run, per project (settings → retention):
//! 1. event details (payloads, attachments, feedback in `store.sqlite`)
//!    older than `retention_event_days` (30) go; the events keep their small
//!    index rows, so counts, users and the event list stay;
//! 2. issues last seen more than `retention_issue_days` (90) ago go, with
//!    their events and details;
//! 3. hourly counts older than 30 days go (the pages show at most 14 days);
//!
//! then the store's rows of deleted projects go, new and regressed issues
//! that have been so for a week become ongoing, and `store.sqlite` gives
//! freed pages back (incremental vacuum).
//!
//! Debug files ([`prune_debug_files`]) go when they were neither uploaded
//! nor used for `retention_dif_days` (180), unless one of the project's
//! `retention_dif_releases` (5) newest releases used them; with them their
//! objects and SymCaches on disk, chunks of uploads that never finished,
//! and the files of deleted projects.
//!
//! Every minute the job also ends ignoring that ran out by time (an issue
//! ignored for an hour comes back even if no event arrives).
//!
//! Everything runs on the writer's connection ([`Writer::run`]).

use std::collections::HashSet;
use std::time::Duration;

use rusqlite::{Connection, params};

use crate::db::triage;
use crate::difs::Difs;
use crate::writer::{Writer, WriterError};

const FIRST_RUN_DELAY: Duration = Duration::from_secs(60);
const RUN_EVERY: Duration = Duration::from_secs(24 * 3600);
const TICK: Duration = Duration::from_secs(60);
/// Payloads removed per write.
const DETAIL_BATCH: i64 = 2_000;
/// Issues removed per write.
const ISSUE_BATCH: i64 = 200;
/// Pause between batches so ingest can write.
const BATCH_PAUSE: Duration = Duration::from_millis(25);
/// Hourly counts are kept this long.
pub const HOURS_KEPT_DAYS: i64 = 30;
/// Pages `store.sqlite` gives back per run.
const VACUUM_PAGES: i64 = 10_000;

/// What one run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub details_deleted: usize,
    pub issues_deleted: usize,
    pub hours_deleted: usize,
    pub settled: usize,
}

/// Chunks of uploads that never assembled are kept this long.
const CHUNKS_KEPT: Duration = Duration::from_secs(24 * 3600);

/// Starts the job (at server start).
pub fn spawn(writer: Writer, difs: Difs) {
    tokio::spawn(async move {
        let mut next_run = tokio::time::Instant::now() + FIRST_RUN_DELAY;
        let mut ticker = tokio::time::interval(TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let now = crate::issues::now();
            let n = now.clone();
            let expired = writer
                .run(move |c| {
                    let tx = c.transaction()?;
                    let n = triage::expire_ignored(&tx, &n)?;
                    tx.commit()?;
                    Ok(n)
                })
                .await;
            match expired {
                Ok(n) if n > 0 => tracing::info!(issues = n, "ignoring ran out"),
                Ok(_) => {}
                Err(err) => tracing::warn!(%err, "can't end expired ignoring"),
            }
            if tokio::time::Instant::now() >= next_run {
                next_run = tokio::time::Instant::now() + RUN_EVERY;
                if let Err(err) = run_once(&writer, &now).await {
                    tracing::error!(%err, "retention run failed");
                }
                match prune_debug_files(&writer, &difs, &now).await {
                    Ok(n) if n > 0 => tracing::info!(files = n, "debug files removed"),
                    Ok(_) => {}
                    Err(err) => tracing::error!(%err, "debug file retention failed"),
                }
            }
        }
    });
}

/// `(details cutoff, issues cutoff)` for a project at `now`.
fn cutoffs(c: &Connection, now: &str, r: triage::Retention) -> rusqlite::Result<(String, String)> {
    c.query_row(
        "SELECT datetime(?1, ?2), datetime(?1, ?3)",
        params![
            now,
            format!("-{} days", r.event_days.max(1)),
            format!("-{} days", r.issue_days.max(1))
        ],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
}

fn hours_cutoff(c: &Connection, now: &str) -> rusqlite::Result<String> {
    c.query_row(
        "SELECT strftime('%Y-%m-%d %H:00:00', ?1, ?2)",
        params![now, format!("-{HOURS_KEPT_DAYS} days")],
        |r| r.get(0),
    )
}

fn vacuum(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch("PRAGMA optimize;")?;
    let mode: i64 = c.query_row("PRAGMA store.auto_vacuum", [], |r| r.get(0))?;
    if mode == 2 {
        // The pragma returns no rows; reading it runs it.
        let mut stmt = c.prepare(&format!("PRAGMA store.incremental_vacuum({VACUUM_PAGES})"))?;
        let mut rows = stmt.query([])?;
        while rows.next()?.is_some() {}
    }
    Ok(())
}

/// One run through the writer, in batches with pauses (`now` is
/// `YYYY-MM-DD HH:MM:SS` UTC).
pub async fn run_once(writer: &Writer, now: &str) -> Result<Report, WriterError> {
    let started = std::time::Instant::now();
    let mut report = Report::default();
    let projects = writer.run(|c| triage::all_retention(c)).await?;
    for (project_id, retention) in projects {
        let n = now.to_string();
        let (details_cutoff, issues_cutoff) =
            writer.run(move |c| cutoffs(c, &n, retention)).await?;
        loop {
            let cutoff = details_cutoff.clone();
            let deleted = writer
                .run(move |c| {
                    let tx = c.transaction()?;
                    let n = triage::prune_details(&tx, project_id, &cutoff, DETAIL_BATCH)?;
                    tx.commit()?;
                    Ok(n)
                })
                .await?;
            report.details_deleted += deleted;
            if (deleted as i64) < DETAIL_BATCH {
                break;
            }
            tokio::time::sleep(BATCH_PAUSE).await;
        }
        let mut issues_removed = 0;
        loop {
            let cutoff = issues_cutoff.clone();
            let deleted = writer
                .run(move |c| {
                    let tx = c.transaction()?;
                    let n = triage::prune_issues(&tx, project_id, &cutoff, ISSUE_BATCH)?;
                    tx.commit()?;
                    Ok(n)
                })
                .await?;
            issues_removed += deleted;
            if (deleted as i64) < ISSUE_BATCH {
                break;
            }
            tokio::time::sleep(BATCH_PAUSE).await;
        }
        report.issues_deleted += issues_removed;
        let n = now.to_string();
        report.hours_deleted += writer
            .run(move |c| {
                let cutoff = hours_cutoff(c, &n)?;
                triage::prune_hours(c, project_id, &cutoff)
            })
            .await?;
        if issues_removed > 0 {
            writer.notify(project_id);
        }
    }
    loop {
        let removed = writer
            .run(|c| triage::prune_orphans(c, DETAIL_BATCH))
            .await?;
        report.details_deleted += removed;
        if (removed as i64) < DETAIL_BATCH {
            break;
        }
        tokio::time::sleep(BATCH_PAUSE).await;
    }
    let n = now.to_string();
    report.settled = writer.run(move |c| triage::settle_substatus(c, &n)).await?;
    writer.run(|c| vacuum(c)).await?;
    tracing::info!(
        details = report.details_deleted,
        issues = report.issues_deleted,
        hours = report.hours_deleted,
        settled = report.settled,
        elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "retention run finished"
    );
    Ok(report)
}

/// Removes the debug files retention says go (rows through the writer,
/// then their directories), the files of deleted projects, directories
/// without a row (an upload cut short) and chunks older than a day.
/// Returns how many debug files went.
pub async fn prune_debug_files(
    writer: &Writer,
    difs: &Difs,
    now: &str,
) -> Result<usize, WriterError> {
    let n = now.to_string();
    let (expired, projects) = writer
        .run(move |c| {
            let tx = c.transaction()?;
            let mut out = Vec::new();
            for (project_id, r) in crate::db::difs::all_retention(&tx)? {
                for (id, debug_id) in crate::db::difs::expired(&tx, project_id, r, &n)? {
                    crate::db::difs::delete(&tx, project_id, id)?;
                    out.push((project_id, debug_id));
                }
            }
            let projects: HashSet<i64> = tx
                .prepare("SELECT id FROM projects")?
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            tx.commit()?;
            Ok((out, projects))
        })
        .await?;
    for (project_id, debug_id) in &expired {
        if let Some(id) = crate::difs::parse_debug_id(debug_id) {
            difs.remove(*project_id, &id);
        }
    }
    let difs = difs.clone();
    let _ = tokio::task::spawn_blocking(move || {
        for project_id in difs.stray_projects(&projects) {
            difs.forget_project(project_id);
        }
        for project_id in &projects {
            difs.remove_unlisted(*project_id, CHUNKS_KEPT);
        }
        crate::difs::files::prune_chunks(&difs.chunk_dir(), CHUNKS_KEPT);
    })
    .await;
    Ok(expired.len())
}

/// The same steps on one connection (with `store.sqlite` attached as
/// `store`) without pauses, for tests and tooling.
pub fn run_blocking(c: &mut Connection, now: &str) -> rusqlite::Result<Report> {
    let mut report = Report::default();
    for (project_id, retention) in triage::all_retention(c)? {
        let (details_cutoff, issues_cutoff) = cutoffs(c, now, retention)?;
        let tx = c.transaction()?;
        loop {
            let n = triage::prune_details(&tx, project_id, &details_cutoff, DETAIL_BATCH)?;
            report.details_deleted += n;
            if (n as i64) < DETAIL_BATCH {
                break;
            }
        }
        loop {
            let n = triage::prune_issues(&tx, project_id, &issues_cutoff, ISSUE_BATCH)?;
            report.issues_deleted += n;
            if (n as i64) < ISSUE_BATCH {
                break;
            }
        }
        let cutoff = hours_cutoff(&tx, now)?;
        report.hours_deleted += triage::prune_hours(&tx, project_id, &cutoff)?;
        tx.commit()?;
    }
    report.details_deleted += triage::prune_orphans(c, i64::MAX)?;
    report.settled = triage::settle_substatus(c, now)?;
    vacuum(c)?;
    Ok(report)
}
