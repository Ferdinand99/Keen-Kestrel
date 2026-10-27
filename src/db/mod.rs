//! Kestrel's tables and its migration set. The pool, the runner and the core
//! tables (users, API tokens, settings) are vole-kit's.
//!
//! [`issues`] is the writer's side (one event at a time); [`triage`] holds
//! what the pages read and the lifecycle actions they take.

pub mod alerts;
pub mod difs;
pub mod groups;
pub mod issues;
pub mod projects;
pub mod releases;
pub mod triage;

use rusqlite::Connection;
use vole_kit::db::migrate::{self, MigrationError, MigrationSet};

/// Kestrel's own migrations, sorted by name. They run after the kit's core set.
const MIGRATION_FILES: &[(&str, &str)] = &[
    (
        "0001_projects.sql",
        include_str!("../../migrations/0001_projects.sql"),
    ),
    (
        "0002_errors.sql",
        include_str!("../../migrations/0002_errors.sql"),
    ),
    (
        "0003_triage.sql",
        include_str!("../../migrations/0003_triage.sql"),
    ),
    (
        "0004_debug_files.sql",
        include_str!("../../migrations/0004_debug_files.sql"),
    ),
    (
        "0005_alerts_releases.sql",
        include_str!("../../migrations/0005_alerts_releases.sql"),
    ),
    (
        "0006_analytics.sql",
        include_str!("../../migrations/0006_analytics.sql"),
    ),
    (
        "0007_app_analytics.sql",
        include_str!("../../migrations/0007_app_analytics.sql"),
    ),
    (
        "0008_token_scopes.sql",
        include_str!("../../migrations/0008_token_scopes.sql"),
    ),
    (
        "0009_project_groups.sql",
        include_str!("../../migrations/0009_project_groups.sql"),
    ),
    (
        "0010_difs_token_projects.sql",
        include_str!("../../migrations/0010_difs_token_projects.sql"),
    ),
];

fn migrations() -> Vec<(String, String)> {
    MIGRATION_FILES
        .iter()
        .map(|(name, sql)| ((*name).to_string(), (*sql).to_string()))
        .collect()
}

fn no_fixups(_: &mut Connection) -> Result<(), MigrationError> {
    Ok(())
}

/// The kit's core set first (with `api_token_scopes`, which the upload
/// tokens' `difs:write` scope needs), then Kestrel's (a fresh product:
/// nothing to adopt).
pub static MIGRATIONS: MigrationSet = MigrationSet {
    core: migrate::CORE,
    adopt_core: None,
    migrations,
    fk_off_marker: "-- kk:foreign_keys=off",
    baseline: None,
    after: no_fixups,
};
