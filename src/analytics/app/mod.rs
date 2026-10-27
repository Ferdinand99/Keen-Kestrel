//! App analytics (docs/design.md §5, phase 9): signals from KestrelKit (and
//! any SDK that speaks its protocol, KestrelKit's `docs/protocol.md`).
//!
//! * `crate::ingest::signals`: `POST /ingest/v1/signals`, which checks the
//!   key and hands the cleaned batch to the writer as one job;
//! * here: what a signal keeps ([`clean_batch`]): the type, the app's bundle
//!   id, version and build, the platform, OS version, device model and
//!   locale, the debug/TestFlight/simulator flags, the payload (at most 20
//!   keys, values scrubbed of email addresses, phone and card numbers,
//!   credentials and IP addresses), the time corrected for the device clock,
//!   and keyed hashes of the install and session ids ([`user_hash`]);
//! * [`rollup`]: every minute, with the web's rollup, raw signals become
//!   hourly and daily counts, daily/weekly/monthly active users, sessions and
//!   breakdowns (`source = 'app'`);
//! * [`query`]: what the App tab, the explorer, the release pages, the REST
//!   API and MCP read; [`pages`]: the App tab and the signal explorer.
//!
//! A user is `trunc64(HMAC-SHA256(project secret, "install" ‖ 0 ‖ install
//! id))`: the secret is random per project and never leaves the database, so
//! the hash can't be tied to the install id without it, and the raw install
//! id is never stored. In strict mode the key is `HMAC(secret, "day" ‖ 0 ‖
//! day)`, a new one every UTC day, so a user can't be followed across days.

pub mod pages;
pub mod query;
pub mod rollup;

use rusqlite::{Connection, params};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::clean::{self, short};
use super::day_number;
use crate::event::scrub;

/// The source of app signals (`source` columns).
pub const APP: &str = "app";

/// Largest request body, compressed or not (protocol: 256 KB, P2).
pub const MAX_BODY: usize = 256 * 1024;
/// Signals per request at most.
pub const MAX_SIGNALS: usize = 100;
/// Longest signal type (characters).
pub const MAX_TYPE: usize = 100;
/// Payload keys per signal at most.
pub const MAX_PAYLOAD_KEYS: usize = 20;
/// Longest payload key (characters).
pub const MAX_KEY: usize = 40;
/// Longest payload value and other text field (characters).
pub const MAX_VALUE: usize = 200;
/// Longest install or session id (characters).
pub const MAX_ID: usize = 64;
/// A device clock this far off (milliseconds) is corrected (P10).
pub const SKEW_TOLERANCE_MS: i64 = 5_000;
/// Signals older than this (seconds, 48 hours) are dropped, whatever the
/// raw retention. A signal for a day before yesterday makes the rollup
/// recompute that whole day for the project; with the clock taken from the
/// client, a few signals a minute for the last 30 days would keep the
/// writer busy with nothing else. Offline devices catch up within two days.
pub const MAX_BACKDATE_SECS: i64 = 48 * 3600;

/// `flags` bits.
pub const FLAG_DEBUG: i64 = 1;
pub const FLAG_TESTFLIGHT: i64 = 2;
pub const FLAG_SIMULATOR: i64 = 4;

/// One signal as the writer stores it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppSignal {
    pub project_id: i64,
    /// Unix seconds (UTC), when it happened (corrected, never in the future).
    pub ts: i64,
    /// The user hash ([`user_hash`]).
    pub user: i64,
    /// The session hash (0: none).
    pub session: i64,
    /// The signal's type, e.g. `Paywall.shown`.
    pub kind: String,
    pub bundle_id: String,
    pub version: String,
    pub build: String,
    pub platform: String,
    pub os_version: String,
    pub model: String,
    pub locale: String,
    /// [`FLAG_DEBUG`] | [`FLAG_TESTFLIGHT`] | [`FLAG_SIMULATOR`].
    pub flags: i64,
    /// The payload as a JSON object, scrubbed; `None` when empty.
    pub payload: Option<String>,
}

/// Stores one signal (the writer's job).
pub fn insert(c: &Connection, s: &AppSignal) -> rusqlite::Result<()> {
    c.prepare_cached(
        "INSERT INTO store.analytics_events (project_id, source, ts, visitor, name, os, props,
             bundle_id, app_version, build, os_version, model, locale, flags, session)
         VALUES (?1, 'app', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
    )?
    .execute(params![
        s.project_id,
        s.ts,
        s.user,
        s.kind,
        s.platform,
        s.payload,
        s.bundle_id,
        s.version,
        s.build,
        s.os_version,
        s.model,
        s.locale,
        s.flags,
        s.session,
    ])?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Dimensions
// ---------------------------------------------------------------------------

/// What app signals are broken down by (`analytics_breakdown.dim`), besides
/// the payload (`payload`, see [`rollup`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AppDim {
    /// The signal's type.
    Type,
    /// The app version (`1.4.0`).
    Version,
    /// Version and build (`1.4.0 (57)`).
    Build,
    /// `bundle@version+build`, as Sentry SDKs name releases (release pages).
    Release,
    /// iOS, iPadOS, macOS, visionOS, watchOS, tvOS (or what the SDK sent).
    Platform,
    /// Platform and OS version (`iOS 18.2`).
    Os,
    /// The hardware model (`iPhone16,2`).
    Model,
    /// The locale (`nb_NO`).
    Locale,
}

impl AppDim {
    pub const ALL: [AppDim; 8] = [
        AppDim::Type,
        AppDim::Version,
        AppDim::Build,
        AppDim::Release,
        AppDim::Platform,
        AppDim::Os,
        AppDim::Model,
        AppDim::Locale,
    ];

    /// The name in rollups, URLs, the API and MCP.
    pub fn code(self) -> &'static str {
        match self {
            AppDim::Type => "type",
            AppDim::Version => "version",
            AppDim::Build => "build",
            AppDim::Release => "release",
            AppDim::Platform => "platform",
            AppDim::Os => "os",
            AppDim::Model => "model",
            AppDim::Locale => "locale",
        }
    }

    pub fn parse(code: &str) -> Option<AppDim> {
        AppDim::ALL.into_iter().find(|d| d.code() == code)
    }

    /// The seen-set code (`analytics_seen.dim`): 110 and up, apart from the
    /// web's (0–12) and the app's day and hour codes (100–103).
    pub fn seen_code(self) -> i64 {
        110 + AppDim::ALL
            .iter()
            .position(|d| *d == self)
            .map_or(0, |i| i as i64)
    }

    /// The SQL expression of its value on a raw row.
    pub fn sql(self) -> &'static str {
        match self {
            AppDim::Type => "name",
            AppDim::Version => "app_version",
            AppDim::Build => {
                "CASE WHEN build = '' THEN app_version ELSE app_version || ' (' || build || ')' END"
            }
            AppDim::Release => {
                "CASE WHEN bundle_id = '' OR app_version = '' THEN '' WHEN build = '' THEN bundle_id || '@' || app_version ELSE bundle_id || '@' || app_version || '+' || build END"
            }
            AppDim::Platform => "os",
            AppDim::Os => "trim(os || ' ' || os_version)",
            AppDim::Model => "model",
            AppDim::Locale => "locale",
        }
    }

    /// The value of this dimension for a signal; `None` when it doesn't
    /// count here (a release needs a bundle id and a version).
    pub fn value_of(self, s: &AppSignal) -> Option<String> {
        Some(match self {
            AppDim::Type => s.kind.clone(),
            AppDim::Version => s.version.clone(),
            AppDim::Build => build_label(&s.version, &s.build),
            AppDim::Release => release_name(&s.bundle_id, &s.version, &s.build)?,
            AppDim::Platform => s.platform.clone(),
            AppDim::Os => format!("{} {}", s.platform, s.os_version)
                .trim()
                .to_string(),
            AppDim::Model => s.model.clone(),
            AppDim::Locale => s.locale.clone(),
        })
    }
}

/// `1.4.0 (57)`, or the version alone without a build.
pub fn build_label(version: &str, build: &str) -> String {
    if build.is_empty() {
        version.to_string()
    } else {
        format!("{version} ({build})")
    }
}

/// The release name Sentry's Apple SDK uses for the same app:
/// `bundle@version+build` (`bundle@version` without a build).
pub fn release_name(bundle_id: &str, version: &str, build: &str) -> Option<String> {
    if bundle_id.is_empty() || version.is_empty() {
        return None;
    }
    Some(if build.is_empty() {
        format!("{bundle_id}@{version}")
    } else {
        format!("{bundle_id}@{version}+{build}")
    })
}

// ---------------------------------------------------------------------------
// Hashes
// ---------------------------------------------------------------------------

/// HMAC-SHA256 (RFC 2104) of the concatenated `parts` under `key`.
pub fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&Sha256::digest(key)[..]);
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block.map(|b| b ^ 0x36));
    for p in parts {
        inner.update(p);
    }
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(block.map(|b| b ^ 0x5c));
    outer.update(&inner[..]);
    let mut out = [0u8; 32];
    out.copy_from_slice(&outer.finalize()[..]);
    out
}

/// The key of a strict project's hashes on `day` (a [`day_number`]).
pub fn day_key(secret: &[u8], day: i64) -> [u8; 32] {
    hmac_sha256(secret, &[b"day\0", day.to_string().as_bytes()])
}

fn hash64(key: &[u8], scope: &[u8], value: &str) -> i64 {
    let mac = hmac_sha256(key, &[scope, b"\0", value.as_bytes()]);
    let mut first = [0u8; 8];
    first.copy_from_slice(&mac[..8]);
    i64::from_be_bytes(first)
}

/// The keys a project's hashes use: its secret, or in strict mode the
/// secret's key of the day.
pub struct Keys<'a> {
    secret: &'a [u8],
    strict: bool,
    cached: Option<(i64, [u8; 32])>,
}

impl<'a> Keys<'a> {
    pub fn new(secret: &'a [u8], strict: bool) -> Keys<'a> {
        Keys {
            secret,
            strict,
            cached: None,
        }
    }

    fn hash(&mut self, day: i64, scope: &[u8], value: &str) -> i64 {
        if !self.strict {
            return hash64(self.secret, scope, value);
        }
        let key = match self.cached {
            Some((d, k)) if d == day => k,
            _ => {
                let k = day_key(self.secret, day);
                self.cached = Some((day, k));
                k
            }
        };
        hash64(&key, scope, value)
    }

    /// The user hash of an install id on `day`.
    pub fn user(&mut self, day: i64, install_id: &str) -> i64 {
        self.hash(day, b"install", install_id)
    }

    /// The session hash of a session id on `day`.
    pub fn session(&mut self, day: i64, session_id: &str) -> i64 {
        self.hash(day, b"session", session_id)
    }
}

/// The user hash of `install_id` (see the module docs).
pub fn user_hash(secret: &[u8], strict: bool, day: i64, install_id: &str) -> i64 {
    Keys::new(secret, strict).user(day, install_id)
}

// ---------------------------------------------------------------------------
// The wire format (KestrelKit's docs/protocol.md)
// ---------------------------------------------------------------------------

/// Why a batch is refused as a whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchError {
    /// Not JSON, or not `{"signals": [...]}` (400).
    Malformed(&'static str),
    /// More than [`MAX_SIGNALS`] signals (413: the SDK splits the batch).
    TooMany,
}

/// A request body that has the protocol's shape.
#[derive(Debug, Clone)]
pub struct Batch {
    /// `sent_at` in Unix milliseconds, when it is there and readable.
    pub sent_at: Option<i64>,
    pub signals: Vec<Value>,
}

/// Reads a request body (strict per batch, P5): a JSON object with a
/// `signals` array of at most [`MAX_SIGNALS`]. `sent_at` and `sdk` are
/// optional, unknown fields are ignored (P6).
pub fn parse_batch(data: &[u8]) -> Result<Batch, BatchError> {
    let Ok(Value::Object(mut body)) = serde_json::from_slice::<Value>(data) else {
        return Err(BatchError::Malformed("the body must be a JSON object"));
    };
    let Some(Value::Array(signals)) = body.remove("signals") else {
        return Err(BatchError::Malformed("the body needs a signals array"));
    };
    if signals.len() > MAX_SIGNALS {
        return Err(BatchError::TooMany);
    }
    let sent_at = body
        .get("sent_at")
        .and_then(Value::as_str)
        .and_then(parse_time_ms);
    Ok(Batch { sent_at, signals })
}

/// An RFC 3339 time (`2026-10-22T18:59:58.123Z`) in Unix milliseconds.
pub fn parse_time_ms(text: &str) -> Option<i64> {
    let t: jiff::Timestamp = text.trim().parse().ok()?;
    Some(t.as_millisecond())
}

/// A valid signal type: 1–100 characters from `[A-Za-z0-9._-]`, cut at 100.
pub fn signal_type(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty()
        || !t
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return None;
    }
    Some(t.chars().take(MAX_TYPE).collect())
}

/// What the server knows when it cleans a batch.
pub struct Ctx<'a> {
    pub project_id: i64,
    pub secret: &'a [u8],
    pub strict: bool,
    /// The server's clock, Unix milliseconds.
    pub now_ms: i64,
    /// Signals older than this (Unix seconds) are dropped
    /// ([`oldest_accepted`]).
    pub oldest: i64,
}

/// The oldest time (Unix seconds) a signal may have at `now_ms` for a project
/// that keeps raw events `raw_days` days: [`MAX_BACKDATE_SECS`] back, and
/// never further than the retention. Later times are clamped to the arrival
/// ([`clean_signal`]).
pub fn oldest_accepted(now_ms: i64, raw_days: i64) -> i64 {
    now_ms.div_euclid(1000) - (raw_days.max(1) * 86_400).min(MAX_BACKDATE_SECS)
}

/// The clock correction for a batch: `server_now − sent_at` when the two
/// are more than [`SKEW_TOLERANCE_MS`] apart, else 0 (P10).
pub fn skew_ms(sent_at: Option<i64>, now_ms: i64) -> i64 {
    match sent_at {
        Some(sent) if (now_ms - sent).abs() > SKEW_TOLERANCE_MS => now_ms - sent,
        _ => 0,
    }
}

/// A text field of an object, cleaned and cut to [`MAX_VALUE`] (P4).
fn field(obj: Option<&Map<String, Value>>, key: &str) -> String {
    short(
        obj.and_then(|o| o.get(key))
            .and_then(Value::as_str)
            .unwrap_or(""),
        MAX_VALUE,
    )
}

/// A payload value as text: strings, numbers and booleans (the SDK sends
/// strings; others are converted), else `None`.
fn payload_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// A payload as stored: at most [`MAX_PAYLOAD_KEYS`] keys (cut to 40
/// characters, without control characters) with text values cut to 200 and
/// scrubbed. Keys that name an IP address or a user agent are dropped.
/// `None` when nothing is left.
pub fn payload(raw: Option<&Value>) -> Option<String> {
    let Some(Value::Object(map)) = raw else {
        return None;
    };
    let mut out = Map::new();
    for (k, v) in map {
        if out.len() >= MAX_PAYLOAD_KEYS {
            break;
        }
        let key = short(k, MAX_KEY);
        if key.is_empty()
            || out.contains_key(&key)
            || scrub::is_ip_key(&key)
            || scrub::is_user_agent_key(&key)
        {
            continue;
        }
        let Some(value) = payload_text(v) else {
            continue;
        };
        out.insert(key, Value::from(short(&clean::scrub(&value), MAX_VALUE)));
    }
    (!out.is_empty()).then(|| Value::Object(out).to_string())
}

/// One signal as stored, or `None` when it can't be (lenient per signal,
/// P5): it isn't an object, its type isn't valid, it has no install id, or
/// it is older than the raw retention once its time is corrected.
pub fn clean_signal(v: &Value, ctx: &Ctx<'_>, skew: i64, keys: &mut Keys<'_>) -> Option<AppSignal> {
    let o = v.as_object()?;
    let kind = signal_type(o.get("type")?.as_str()?)?;
    let install: String = o
        .get("install_id")?
        .as_str()?
        .trim()
        .chars()
        .take(MAX_ID)
        .collect();
    if install.is_empty() {
        return None;
    }
    let time_ms = o
        .get("time")
        .and_then(Value::as_str)
        .and_then(parse_time_ms)
        .map_or(ctx.now_ms, |t| t.saturating_add(skew))
        .min(ctx.now_ms);
    let ts = time_ms.div_euclid(1000);
    if ts < ctx.oldest {
        return None;
    }
    let day = day_number(ts);
    let session_id: String = o
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .chars()
        .take(MAX_ID)
        .collect();
    let app = o.get("app").and_then(Value::as_object);
    let device = o.get("device").and_then(Value::as_object);
    let flags = o.get("flags").and_then(Value::as_object);
    let flag = |name: &str, bit: i64| {
        if flags.and_then(|f| f.get(name)).and_then(Value::as_bool) == Some(true) {
            bit
        } else {
            0
        }
    };
    Some(AppSignal {
        project_id: ctx.project_id,
        ts,
        user: keys.user(day, &install),
        session: if session_id.is_empty() {
            0
        } else {
            keys.session(day, &session_id)
        },
        kind,
        bundle_id: field(app, "bundle_id"),
        version: field(app, "version"),
        build: field(app, "build"),
        platform: field(device, "platform"),
        os_version: field(device, "os_version"),
        model: field(device, "model"),
        locale: field(device, "locale"),
        flags: flag("debug", FLAG_DEBUG)
            | flag("testflight", FLAG_TESTFLIGHT)
            | flag("simulator", FLAG_SIMULATOR),
        payload: payload(o.get("payload")),
    })
}

/// The signals of a batch that can be stored, and how many couldn't.
pub fn clean_batch(batch: &Batch, ctx: &Ctx<'_>) -> (Vec<AppSignal>, usize) {
    let skew = skew_ms(batch.sent_at, ctx.now_ms);
    let mut keys = Keys::new(ctx.secret, ctx.strict);
    let mut out = Vec::with_capacity(batch.signals.len());
    for v in &batch.signals {
        if let Some(s) = clean_signal(v, ctx, skew, &mut keys) {
            out.push(s);
        }
    }
    let dropped = batch.signals.len() - out.len();
    (out, dropped)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const GOLDEN: &str = r#"{"sdk":{"name":"kestrelkit","version":"0.0.0"},"sent_at":"2026-10-22T19:00:00.000Z","signals":[{"app":{"build":"57","bundle_id":"com.example.app","version":"1.4.0"},"device":{"locale":"nb_NO","model":"iPhone16,2","os_version":"18.2","platform":"iOS"},"flags":{"debug":false,"simulator":false,"testflight":false},"install_id":"8F3C2B1A-0D4E-4F5A-9B6C-7D8E9F0A1B2C","payload":{"plan":"pro"},"session_id":"1B2C3D4E-5F60-4718-8293-A4B5C6D7E8F9","time":"2026-10-22T18:59:58.123Z","type":"App.launched"}]}"#;

    fn ctx(now_ms: i64) -> Ctx<'static> {
        Ctx {
            project_id: 1,
            secret: b"0123456789abcdef0123456789abcdef",
            strict: false,
            now_ms,
            oldest: now_ms / 1000 - 30 * 86_400,
        }
    }

    #[test]
    fn hmac_matches_rfc_4231() {
        // Test case 2.
        let mac = hmac_sha256(b"Jefe", &[b"what do ya want ", b"for nothing?"]);
        assert_eq!(
            hex::encode(mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // Test case 6: a key longer than the block.
        let mac = hmac_sha256(
            &[0xaa; 131],
            &[b"Test Using Larger Than Block-Size Key - Hash Key First"],
        );
        assert_eq!(
            hex::encode(mac),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn the_golden_batch_is_read_field_by_field() {
        let now = parse_time_ms("2026-10-22T19:00:00.400Z").unwrap();
        let batch = parse_batch(GOLDEN.as_bytes()).unwrap();
        assert_eq!(batch.sent_at, parse_time_ms("2026-10-22T19:00:00.000Z"));
        let (signals, dropped) = clean_batch(&batch, &ctx(now));
        assert_eq!(dropped, 0);
        let s = &signals[0];
        assert_eq!(s.kind, "App.launched");
        assert_eq!(s.ts, 1_792_695_598, "no correction within 5 s");
        assert_eq!(
            (s.bundle_id.as_str(), s.version.as_str(), s.build.as_str()),
            ("com.example.app", "1.4.0", "57")
        );
        assert_eq!(
            (
                s.platform.as_str(),
                s.os_version.as_str(),
                s.model.as_str(),
                s.locale.as_str()
            ),
            ("iOS", "18.2", "iPhone16,2", "nb_NO")
        );
        assert_eq!(s.flags, 0);
        assert_eq!(s.payload.as_deref(), Some(r#"{"plan":"pro"}"#));
        assert_eq!(
            s.user,
            user_hash(
                b"0123456789abcdef0123456789abcdef",
                false,
                0,
                "8F3C2B1A-0D4E-4F5A-9B6C-7D8E9F0A1B2C"
            )
        );
        assert_ne!(s.session, 0);
        assert_eq!(
            AppDim::Release.value_of(s).as_deref(),
            Some("com.example.app@1.4.0+57")
        );
        assert_eq!(AppDim::Build.value_of(s).as_deref(), Some("1.4.0 (57)"));
        assert_eq!(AppDim::Os.value_of(s).as_deref(), Some("iOS 18.2"));
    }

    #[test]
    fn batches_are_strict_and_signals_lenient() {
        assert!(matches!(
            parse_batch(b"not json"),
            Err(BatchError::Malformed(_))
        ));
        assert!(matches!(parse_batch(b"[]"), Err(BatchError::Malformed(_))));
        assert!(matches!(
            parse_batch(br#"{"signals": {}}"#),
            Err(BatchError::Malformed(_))
        ));
        let many = json!({"signals": vec![json!({}); 101]}).to_string();
        assert_eq!(
            parse_batch(many.as_bytes()).unwrap_err(),
            BatchError::TooMany
        );
        // Unknown fields, no sent_at, no sdk: fine.
        let b = parse_batch(br#"{"signals": [], "batch_id": "x"}"#).unwrap();
        assert!(b.sent_at.is_none() && b.signals.is_empty());

        let now = 1_792_695_600_000;
        let ok = json!({"type": "A", "install_id": "i", "time": "2026-10-22T19:00:00.000Z"});
        let batch = Batch {
            sent_at: None,
            signals: vec![
                ok.clone(),
                json!("a string"),
                json!({"type": "Bad type!", "install_id": "i"}),
                json!({"type": "", "install_id": "i"}),
                json!({"type": "A"}),
                json!({"type": "A", "install_id": "   "}),
                json!({"type": "A", "install_id": "i", "time": "2026-08-01T00:00:00Z"}),
                json!({"type": "x".repeat(150), "install_id": "i".repeat(100), "time": "garbage", "flags": {"debug": true, "testflight": "yes", "simulator": true}, "device": {"platform": "Android"}}),
            ],
        };
        let (signals, dropped) = clean_batch(&batch, &ctx(now));
        assert_eq!((signals.len(), dropped), (2, 6));
        let long = &signals[1];
        assert_eq!(long.kind.len(), MAX_TYPE, "cut");
        assert_eq!(long.ts, now / 1000, "an unreadable time is the arrival");
        assert_eq!(long.flags, FLAG_DEBUG | FLAG_SIMULATOR);
        assert_eq!(long.platform, "Android", "unknown platforms are kept");
        assert_eq!(long.session, 0);
        assert_eq!(
            long.user,
            user_hash(ctx(now).secret, false, 0, &"i".repeat(MAX_ID))
        );
    }

    #[test]
    fn clocks_are_corrected_and_the_future_clamped() {
        let now = parse_time_ms("2026-10-22T19:00:00.000Z").unwrap();
        let one = |time: &str, sent: &str| {
            let batch = Batch {
                sent_at: parse_time_ms(sent),
                signals: vec![json!({"type": "A", "install_id": "i", "time": time})],
            };
            clean_batch(&batch, &ctx(now)).0[0].ts
        };
        // The device is 2 hours ahead: its times are moved back.
        assert_eq!(
            one("2026-10-22T20:59:00.000Z", "2026-10-22T21:00:00.000Z"),
            now / 1000 - 60
        );
        // A device 1 day behind.
        assert_eq!(
            one("2026-10-21T18:00:00.000Z", "2026-10-21T19:00:00.000Z"),
            now / 1000 - 3600
        );
        // Small differences are network time, not skew.
        assert_eq!(
            one("2026-10-22T18:59:00.000Z", "2026-10-22T18:59:57.000Z"),
            now / 1000 - 60
        );
        // Without sent_at a future time is clamped to now.
        let batch = Batch {
            sent_at: None,
            signals: vec![json!({"type": "A", "install_id": "i", "time": "2027-01-01T00:00:00Z"})],
        };
        assert_eq!(clean_batch(&batch, &ctx(now)).0[0].ts, now / 1000);
    }

    #[test]
    fn payloads_are_bounded_and_scrubbed() {
        let mut raw = Map::new();
        for i in 0..25 {
            raw.insert(format!("k{i:02}"), json!(i));
        }
        let p: Value = serde_json::from_str(&payload(Some(&Value::Object(raw))).unwrap()).unwrap();
        assert_eq!(p.as_object().unwrap().len(), MAX_PAYLOAD_KEYS);
        let p = payload(Some(&json!({
            "email": "jane.doe@example.com",
            "phone": "+47 912 34 567",
            "card": "4111 1111 1111 1111",
            "auth": "Authorization: Bearer abc.def-123",
            "note": "order 123456789",
            "pages": 12,
            "pdf": true,
            "nested": {"a": 1},
            "ip": "203.0.113.7",
            "user_agent": "Mozilla/5.0",
            "long": "v".repeat(500),
            &"k".repeat(60): "x",
        })))
        .unwrap();
        let v: Value = serde_json::from_str(&p).unwrap();
        assert_eq!(v["email"], "[Filtered]");
        assert_eq!(v["phone"], "[Filtered]");
        assert_eq!(v["card"], "[Filtered]");
        assert_eq!(v["auth"], "Authorization: Bearer [Filtered]");
        assert_eq!(v["note"], "order 123456789");
        assert_eq!(
            (v["pages"].as_str(), v["pdf"].as_str()),
            (Some("12"), Some("true"))
        );
        assert!(
            v.get("nested").is_none() && v.get("ip").is_none() && v.get("user_agent").is_none()
        );
        assert_eq!(v["long"].as_str().unwrap().chars().count(), MAX_VALUE);
        assert_eq!(v[&"k".repeat(MAX_KEY)], "x");
        assert_eq!(payload(Some(&json!({}))), None);
        assert_eq!(payload(Some(&json!("x"))), None);
        assert_eq!(payload(None), None);
    }

    #[test]
    fn scrubbing_agrees_with_kestrelkit() {
        // KestrelKit's LimitsTests: the same inputs and outputs.
        for (input, expected) in [
            ("jane.doe@example.com", "[Filtered]"),
            ("Contact jane@example.co.uk now", "Contact [Filtered] now"),
            ("+47 912 34 567", "[Filtered]"),
            ("call 0047 91234567", "call [Filtered]"),
            ("(555) 123-4567", "[Filtered]"),
            ("4111 1111 1111 1111", "[Filtered]"),
            ("card 5500-0055-5555-5559 ok", "card [Filtered] ok"),
            (
                "Authorization: Bearer abc.def-123",
                "Authorization: Bearer [Filtered]",
            ),
        ] {
            assert_eq!(clean::scrub(input), expected, "{input}");
        }
        for keep in [
            "pro",
            "1.4.0",
            "order 123456789",
            "2026-10-22",
            "12:30",
            "4111111111111112",
            "1792695598",
            "42",
            "true",
        ] {
            assert_eq!(clean::scrub(keep), keep);
        }
    }

    #[test]
    fn strict_users_change_every_day_and_others_never() {
        let secret = b"secret-secret-secret-secret-1234";
        let a = user_hash(secret, false, 100, "install");
        assert_eq!(a, user_hash(secret, false, 101, "install"));
        assert_ne!(
            a,
            user_hash(b"another project's secret.......", false, 100, "install")
        );
        let s = user_hash(secret, true, 100, "install");
        assert_ne!(s, a);
        assert_eq!(s, user_hash(secret, true, 100, "install"));
        assert_ne!(s, user_hash(secret, true, 101, "install"));
        let mut k = Keys::new(secret, false);
        assert_ne!(k.session(1, "x"), k.user(1, "x"), "scoped");
    }

    #[test]
    fn signal_types() {
        assert_eq!(
            signal_type(" Paywall.shown ").as_deref(),
            Some("Paywall.shown")
        );
        assert_eq!(
            signal_type("Settings.dark-mode_on").as_deref(),
            Some("Settings.dark-mode_on")
        );
        assert_eq!(signal_type("Paywall shown"), None);
        assert_eq!(signal_type("Økonomi"), None);
        assert_eq!(signal_type(""), None);
        for d in AppDim::ALL {
            assert_eq!(AppDim::parse(d.code()), Some(d));
            assert!(d.seen_code() >= 110);
        }
        assert_eq!(release_name("b", "1", ""), Some("b@1".into()));
        assert_eq!(release_name("", "1", "2"), None);
    }
}
