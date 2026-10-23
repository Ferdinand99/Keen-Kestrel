//! `store.sqlite`, the disposable store next to `kestrel.sqlite`: normalised
//! event payloads (zstd-compressed JSON), attachments, session items,
//! feedback and raw analytics events (docs/design.md §4.1, §5).
//!
//! It has its own small migration set, run by the kit's runner
//! ([`vole_kit::db::migrate_with`]) without the pre-migration backup the
//! main database gets: losing it loses event details, never issues or
//! settings. Pages are given back to the file system with incremental
//! auto-vacuum, set by the pool before the first table exists.

use std::path::Path;

use deadpool_sqlite::Pool;
use rusqlite::Connection;
use vole_kit::db::migrate::{MigrateOptions, MigrationError, MigrationSet};
use vole_kit::db::{PoolOptions, build_pool_with, migrate_with};

/// The store's file in the data directory.
pub const STORE_FILE: &str = "store.sqlite";

/// zstd level for payloads: fast, and JSON still shrinks five- to tenfold.
const ZSTD_LEVEL: i32 = 3;

const MIGRATION_FILES: &[(&str, &str)] = &[
    (
        "0001_store.sql",
        include_str!("../migrations/store/0001_store.sql"),
    ),
    (
        "0002_analytics.sql",
        include_str!("../migrations/store/0002_analytics.sql"),
    ),
    (
        "0003_app_signals.sql",
        include_str!("../migrations/store/0003_app_signals.sql"),
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

/// The store's migrations: no core set, nothing to adopt.
pub static MIGRATIONS: MigrationSet = MigrationSet {
    core: &[],
    adopt_core: None,
    migrations,
    fk_off_marker: "-- kk:foreign_keys=off",
    baseline: None,
    after: no_fixups,
};

/// The store's pool: incremental auto-vacuum on a new file (it only takes
/// effect before the first table exists).
pub const POOL: PoolOptions = PoolOptions {
    max_size: None,
    pragmas: "",
    new_database_pragmas: "PRAGMA auto_vacuum = INCREMENTAL;",
};

/// Opens (creating it if needed) and migrates the store in `data_dir`, in
/// WAL mode like the main database and without a backup.
pub async fn open(data_dir: &Path) -> Result<Pool, crate::StartError> {
    let pool = build_pool_with(&data_dir.join(STORE_FILE), POOL);
    let report = migrate_with(&pool, &MIGRATIONS, MigrateOptions::default()).await?;
    if !report.applied.is_empty() {
        tracing::info!(applied = ?report.applied, "store migrated");
    }
    Ok(pool)
}

/// Compresses a payload for the store.
pub fn compress(data: &[u8]) -> Vec<u8> {
    // Compressing into memory only fails when allocation fails.
    zstd::bulk::compress(data, ZSTD_LEVEL).unwrap_or_default()
}

/// Decompresses a payload from the store (at most `limit` bytes).
pub fn decompress(data: &[u8], limit: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read as _;
    let mut out = Vec::new();
    zstd::stream::read::Decoder::new(data)?
        .take(limit as u64)
        .read_to_end(&mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_store_migrates_with_incremental_auto_vacuum() {
        crate::product::install();
        let dir = std::env::temp_dir().join(format!(
            "kk-store-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let pool = open(&dir).await.unwrap();
        let conn = pool.get().await.unwrap();
        let (mode, journal, applied): (i64, String, Vec<String>) = conn
            .interact(|c| {
                let mode = c.query_row("PRAGMA auto_vacuum", [], |r| r.get(0))?;
                let journal = c.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
                let applied = c
                    .prepare("SELECT name FROM schema_migrations ORDER BY rowid")?
                    .query_map([], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                Ok::<_, rusqlite::Error>((mode, journal, applied))
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(mode, 2, "incremental");
        assert_eq!(journal, "wal");
        assert_eq!(
            applied,
            [
                "0001_store.sql",
                "0002_analytics.sql",
                "0003_app_signals.sql"
            ]
        );
        drop(conn);
        vole_kit::db::close(&pool).await;
        // Again: nothing to do, and no backup is taken.
        let pool = open(&dir).await.unwrap();
        vole_kit::db::close(&pool).await;
        assert!(!dir.join("backups").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn payloads_round_trip() {
        let json = br#"{"message":"hello","level":"error"}"#.repeat(50);
        let packed = compress(&json);
        assert!(packed.len() < json.len() / 4);
        assert_eq!(decompress(&packed, 1 << 20).unwrap(), json);
        assert_eq!(decompress(&packed, 10).unwrap().len(), 10);
    }
}
