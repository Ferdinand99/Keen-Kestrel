//! Capture mode (`KESTREL_CAPTURE_DIR`, docs/design.md §9): every ingest
//! request is written as it arrived, for recording SDK fixtures.
//!
//! Each request becomes `<millis>-<n>.json` (method, path with query and
//! headers) and `<millis>-<n>.body` (the body as sent, still compressed).
//! The files hold whatever the SDK sent, DSN keys included: use it on test
//! instances only.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::http::HeaderMap;
use serde_json::{Map, Value, json};

static SEQ: AtomicU64 = AtomicU64::new(0);

/// Writes one request to `dir`. Returns the file stem.
pub fn write(
    dir: &Path,
    method: &str,
    uri: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> std::io::Result<String> {
    std::fs::create_dir_all(dir)?;
    let millis = jiff::Timestamp::now().as_millisecond();
    let stem = format!("{millis}-{:04}", SEQ.fetch_add(1, Ordering::Relaxed));
    let mut map = Map::new();
    for name in headers.keys() {
        let values: Vec<&str> = headers
            .get_all(name)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .collect();
        map.insert(name.as_str().to_string(), Value::from(values.join(", ")));
    }
    let meta = json!({"method": method, "uri": uri, "headers": map});
    let text = serde_json::to_string_pretty(&meta).unwrap_or_default();
    std::fs::write(dir.join(format!("{stem}.json")), text + "\n")?;
    std::fs::write(dir.join(format!("{stem}.body")), body)?;
    Ok(stem)
}
