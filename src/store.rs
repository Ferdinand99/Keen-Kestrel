//! `store.sqlite`, the disposable store next to `kestrel.sqlite`: normalised
//! event payloads (zstd-compressed JSON), attachments, session items,
//! feedback and raw analytics events (docs/design.md §4.1, §5).
//!
//! It has its own small migration set, run by the kit's runner
//! ([`vole_kit::db::migrate_with`]) without the pre-migration backup the
//! main database gets: losing it loses event details, never issues or
//! settings. Pages are given back to the file system with incremental
//! auto-vacuum, set by the pool before the first table exists.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

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
    let mut decoder = zstd::stream::read::Decoder::new(data)?;
    // Kestrel's own frames use the default 2 MiB window.
    decoder.window_log_max(crate::ingest::body::ZSTD_WINDOW_LOG_MAX)?;
    decoder.take(limit as u64).read_to_end(&mut out)?;
    Ok(out)
}

/// Free space the data volume must keep (1 GiB) before the store takes new
/// payloads.
pub const MIN_FREE_BYTES: u64 = 1 << 30;

/// How long a measurement of the volume is reused.
const GUARD_TTL: Duration = Duration::from_secs(5);

/// Watches the data volume (docs/design.md §7): when it has less than
/// `min_free` bytes free, or `store.sqlite` (with its write-ahead log) is
/// over `max_store`, the store is "full". Ingest then still counts events,
/// but stops keeping what takes room: event payloads, attachments, feedback
/// and raw analytics. The answer is cached for a few seconds, so asking is
/// cheap.
pub struct Guard {
    dir: PathBuf,
    min_free: u64,
    max_store: Option<u64>,
    last: Mutex<Option<(Instant, bool)>>,
}

impl Guard {
    /// `min_free`: 0 turns the free-space check off; `max_store`: `None` for
    /// no limit on the store's size.
    pub fn new(dir: &Path, min_free: u64, max_store: Option<u64>) -> Self {
        Guard {
            dir: dir.to_path_buf(),
            min_free,
            max_store,
            last: Mutex::new(None),
        }
    }

    /// Whether the store takes no new payloads now.
    pub fn full(&self) -> bool {
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((at, full)) = *last
            && at.elapsed() < GUARD_TTL
        {
            return full;
        }
        let full = self.measure();
        *last = Some((Instant::now(), full));
        full
    }

    fn measure(&self) -> bool {
        if let Some(max) = self.max_store {
            let size: u64 = [STORE_FILE, "store.sqlite-wal"]
                .iter()
                .filter_map(|f| std::fs::metadata(self.dir.join(f)).ok())
                .map(|m| m.len())
                .sum();
            if size > max {
                return true;
            }
        }
        self.min_free > 0 && free_bytes(&self.dir).is_some_and(|free| free < self.min_free)
    }
}

/// Bytes available to unprivileged writers on the volume of `dir`, where the
/// platform can tell.
#[cfg(unix)]
fn free_bytes(dir: &Path) -> Option<u64> {
    let st = rustix::fs::statvfs(dir).ok()?;
    Some(st.f_bavail.saturating_mul(st.f_frsize))
}

#[cfg(not(unix))]
fn free_bytes(_: &Path) -> Option<u64> {
    None
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

    #[test]
    fn the_guard_trips_on_free_space_or_store_size() {
        let dir = std::env::temp_dir().join(format!("kk-guard-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(STORE_FILE), vec![0u8; 4096]).unwrap();
        // Nothing to trip on.
        assert!(!Guard::new(&dir, 0, None).full());
        // The store is over its limit (its WAL counts too).
        assert!(!Guard::new(&dir, 0, Some(10_000)).full());
        std::fs::write(dir.join("store.sqlite-wal"), vec![0u8; 8192]).unwrap();
        assert!(Guard::new(&dir, 0, Some(10_000)).full());
        // No volume has an exabyte free.
        if cfg!(unix) {
            assert!(Guard::new(&dir, u64::MAX, None).full());
        }
        assert!(!Guard::new(&dir, 1, None).full(), "one byte is free");
        // The answer is reused for a few seconds.
        let g = Guard::new(&dir, 0, Some(10_000));
        assert!(g.full());
        std::fs::remove_file(dir.join("store.sqlite-wal")).unwrap();
        assert!(g.full(), "cached");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
