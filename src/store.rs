//! `store.sqlite`, the disposable store next to `kestrel.sqlite`: normalised
//! event payloads (zstd-compressed JSON), attachments, session items,
//! feedback and raw analytics events (docs/design.md §4.1, §5).
//!
//! It has its own small migration set, run by the kit's runner
//! ([`apply_migrations_with`]) without the pre-migration backup the main
//! database gets: losing it loses event details, never issues or settings.
//! Pages are given back to the file system with incremental auto-vacuum.

use std::path::Path;

use deadpool_sqlite::Pool;
use rusqlite::Connection;
use vole_kit::db::migrate::{MigrationError, MigrationSet, apply_migrations_with};

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

/// Opens (creating it if needed) and migrates the store in `data_dir`.
pub async fn open(data_dir: &Path) -> Result<Pool, crate::StartError> {
    let pool = vole_kit::db::build_pool(&data_dir.join(STORE_FILE));
    let conn = pool.get().await?;
    let applied = conn
        .interact(|c| -> Result<Vec<String>, MigrationError> {
            prepare(c)?;
            apply_migrations_with(c, &MIGRATIONS)
        })
        .await
        .map_err(|e| e.to_string())??;
    if !applied.is_empty() {
        tracing::info!(?applied, "store migrated");
    }
    Ok(pool)
}

/// Incremental auto-vacuum only takes effect on a database without tables,
/// so it is set before the first migration; WAL like the main database.
pub fn prepare(conn: &Connection) -> rusqlite::Result<()> {
    let tables: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_master", [], |r| r.get(0))?;
    if tables == 0 {
        conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
    }
    conn.pragma_update(None, "journal_mode", "WAL")?;
    Ok(())
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

    #[test]
    fn the_store_migrates_with_incremental_auto_vacuum() {
        crate::product::install();
        let mut c = Connection::open_in_memory().unwrap();
        prepare(&c).unwrap();
        let applied = apply_migrations_with(&mut c, &MIGRATIONS).unwrap();
        assert_eq!(
            applied,
            [
                "0001_store.sql",
                "0002_analytics.sql",
                "0003_app_signals.sql"
            ]
        );
        let mode: i64 = c.query_row("PRAGMA auto_vacuum", [], |r| r.get(0)).unwrap();
        assert_eq!(mode, 2, "incremental");
        assert!(
            apply_migrations_with(&mut c, &MIGRATIONS)
                .unwrap()
                .is_empty()
        );
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
