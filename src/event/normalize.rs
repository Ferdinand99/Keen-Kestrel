//! Normalisation (docs/design.md §4.3): one shape for every SDK's event,
//! defaults filled in, sizes bounded, `in_app` derived, and nothing that
//! identifies a person kept: no IP address ever (`{{auto}}` is ignored),
//! credentials and card numbers scrubbed, email and username hashed unless
//! the project keeps them, and the user agent reduced to browser and OS.

use jiff::Timestamp;
use serde_json::{Map, Value, json};

use super::scrub::{self, FILTERED};

/// Breadcrumbs kept (the most recent).
pub const MAX_BREADCRUMBS: usize = 100;
/// Frames kept per stack trace (the innermost).
pub const MAX_FRAMES: usize = 250;
/// Longest string kept, in bytes; longer ones are cut.
pub const MAX_STRING: usize = 8192;
/// Events this much in the future, or older than [`MAX_AGE_SECS`], get the
/// time they were received.
const MAX_FUTURE_SECS: f64 = 60.0;
const MAX_AGE_SECS: f64 = 30.0 * 86400.0;
const MAX_TAG_KEY: usize = 32;
const MAX_TAG_VALUE: usize = 200;

pub const LEVELS: [&str; 5] = ["fatal", "error", "warning", "info", "debug"];

/// What normalisation needs to know about where the event came from.
pub struct Context<'a> {
    pub project_id: i64,
    /// Keep `user.email` and `user.username` as sent.
    pub keep_user_details: bool,
    /// The instance's key for user hashes.
    pub hash_key: &'a [u8; 32],
    pub received: Timestamp,
}

/// What normalisation found out on the way.
#[derive(Debug, Default)]
pub struct Facts {
    /// Keyed hash of the user's id, email or username.
    pub user_hash: Option<String>,
}

/// A 32-character lowercase hex event id from a UUID in any common form.
pub fn event_id(raw: &str) -> Option<String> {
    let id: String = raw
        .trim()
        .chars()
        .filter(|c| *c != '-')
        .collect::<String>()
        .to_ascii_lowercase();
    (id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())).then_some(id)
}

/// A fresh random event id.
pub fn new_event_id() -> String {
    let mut bytes = [0u8; 16];
    rand::fill(&mut bytes);
    hex::encode(bytes)
}

/// Seconds since the epoch from a number or an RFC 3339 string.
fn seconds(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => {
            if let Ok(n) = s.trim().parse::<f64>() {
                return Some(n);
            }
            let s = s.trim();
            let ts = s
                .parse::<Timestamp>()
                .ok()
                .or_else(|| format!("{s}Z").parse::<Timestamp>().ok())?;
            Some(ts.as_millisecond() as f64 / 1000.0)
        }
        _ => None,
    }
    .filter(|n| n.is_finite())
}

/// `YYYY-MM-DD HH:MM:SS` of a time in seconds.
pub fn sql_time(secs: f64) -> String {
    Timestamp::from_millisecond((secs * 1000.0) as i64)
        .unwrap_or(Timestamp::UNIX_EPOCH)
        .strftime("%Y-%m-%d %H:%M:%S")
        .to_string()
}

/// The time in seconds of `ts`.
pub fn ts_seconds(ts: Timestamp) -> f64 {
    ts.as_millisecond() as f64 / 1000.0
}

fn level(raw: Option<&Value>) -> &'static str {
    let raw = raw
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match raw.as_str() {
        "fatal" | "critical" => "fatal",
        "warning" | "warn" => "warning",
        "info" | "log" => "info",
        "debug" | "trace" => "debug",
        _ => "error",
    }
}

/// A trimmed string field, at most `max` characters; `None` when empty.
fn short_string(value: Option<&Value>, max: usize) -> Option<String> {
    let s = match value? {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    let s: String = s.chars().take(max).collect();
    (!s.is_empty()).then_some(s)
}

/// `[…]` or `{"values": […]}` → `{"values": […]}`; anything else is dropped.
fn values_form(map: &mut Map<String, Value>, key: &str) {
    let fixed = match map.remove(key) {
        Some(Value::Array(items)) => Some(json!({ "values": items })),
        Some(Value::Object(mut obj)) => match obj.remove("values") {
            Some(Value::Array(items)) => {
                obj.insert("values".into(), Value::Array(items));
                Some(Value::Object(obj))
            }
            _ => None,
        },
        _ => None,
    };
    if let Some(v) = fixed {
        map.insert(key.into(), v);
    }
}

/// `printf`-style `%s` / `%d` / `%r` (and `{}`) placeholders filled from
/// `params` in order.
fn format_template(template: &str, params: &[Value]) -> String {
    let mut out = String::new();
    let mut params = params.iter();
    let mut chars = template.chars().peekable();
    let text = |v: &Value| match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    while let Some(c) = chars.next() {
        match (c, chars.peek()) {
            ('%', Some('%')) => {
                chars.next();
                out.push('%');
            }
            ('%', Some('s' | 'd' | 'r' | 'i' | 'f')) => {
                chars.next();
                match params.next() {
                    Some(p) => out.push_str(&text(p)),
                    None => out.push_str("%s"),
                }
            }
            ('{', Some('}')) => {
                chars.next();
                match params.next() {
                    Some(p) => out.push_str(&text(p)),
                    None => out.push_str("{}"),
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// `message` (a string or the old message interface) and `logentry` become
/// one `logentry` with `formatted`, and `message`/`params` when the SDK
/// sent a template.
fn logentry(map: &mut Map<String, Value>) {
    let mut entry = match map.remove("logentry") {
        Some(Value::Object(obj)) => obj,
        Some(Value::String(s)) => Map::from_iter([("formatted".to_string(), Value::String(s))]),
        _ => Map::new(),
    };
    match map.remove("message") {
        Some(Value::String(s))
            if !entry.contains_key("formatted") && !entry.contains_key("message") =>
        {
            entry.insert("formatted".into(), Value::String(s));
        }
        Some(Value::Object(obj)) if entry.is_empty() => entry = obj,
        _ => {}
    }
    if entry.is_empty() {
        return;
    }
    let template = entry
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string);
    let has_formatted = entry
        .get("formatted")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty());
    if !has_formatted && let Some(template) = &template {
        let params: Vec<Value> = match entry.get("params") {
            Some(Value::Array(p)) => p.clone(),
            Some(Value::Object(p)) => p.values().cloned().collect(),
            _ => Vec::new(),
        };
        entry.insert(
            "formatted".into(),
            Value::String(format_template(template, &params)),
        );
    }
    // A template that equals the text isn't one.
    if template
        .as_deref()
        .is_some_and(|t| Some(t) == entry.get("formatted").and_then(Value::as_str))
    {
        entry.remove("message");
        entry.remove("params");
    }
    map.insert("logentry".into(), Value::Object(entry));
}

/// Frames as an array, the innermost [`MAX_FRAMES`] kept, `in_app` derived.
fn stacktrace(st: &mut Value, platform: &str) {
    let Some(st) = st.as_object_mut() else {
        return;
    };
    let Some(Value::Array(frames)) = st.get_mut("frames") else {
        st.remove("frames");
        return;
    };
    if frames.len() > MAX_FRAMES {
        frames.drain(..frames.len() - MAX_FRAMES);
    }
    for frame in frames.iter_mut() {
        let Some(frame) = frame.as_object_mut() else {
            continue;
        };
        let frame_platform = frame
            .get("platform")
            .and_then(Value::as_str)
            .unwrap_or(platform)
            .to_string();
        if !matches!(frame.get("in_app"), Some(Value::Bool(_))) {
            frame.insert(
                "in_app".into(),
                Value::Bool(derive_in_app(frame, &frame_platform)),
            );
        } else if matches!(frame_platform.as_str(), "native" | "rust")
            && system_function(str_field(frame, "function"))
        {
            // The Rust SDK marks some standard library and SDK frames in-app
            // (`core[…]::panicking::panic_fmt`, `sentry_panic::…`).
            frame.insert("in_app".into(), Value::Bool(false));
        }
    }
}

/// Rust standard library, runtime and Sentry SDK functions: never in-app.
const RUST_SYSTEM: &[&str] = &[
    "std::",
    "core::",
    "alloc::",
    "std[",
    "core[",
    "alloc[",
    "<std",
    "<core",
    "<alloc",
    "<&dyn ",
    "<fn(",
    "__rust",
    "rust_begin_unwind",
    "rust_panic",
    "sentry::",
    "sentry_",
    "<sentry_",
    "tokio::",
    "<tokio",
    "futures",
    "<futures",
    "backtrace::",
    "<backtrace",
    "<[",
    "<usize ",
    "<isize ",
    "<u8 ",
    "<u16 ",
    "<u32 ",
    "<u64 ",
    "<i8 ",
    "<i16 ",
    "<i32 ",
    "<i64 ",
    "<f32 ",
    "<f64 ",
    "<str ",
    "<&str ",
    "<bool ",
    "<char ",
    "<() ",
];

fn system_function(function: &str) -> bool {
    RUST_SYSTEM.iter().any(|p| function.starts_with(p))
}

fn str_field<'a>(map: &'a Map<String, Value>, key: &str) -> &'a str {
    map.get(key).and_then(Value::as_str).unwrap_or("")
}

/// `in_app` for a frame whose SDK didn't say, by platform.
pub fn derive_in_app(frame: &Map<String, Value>, platform: &str) -> bool {
    let path = {
        let abs = str_field(frame, "abs_path");
        if abs.is_empty() {
            str_field(frame, "filename")
        } else {
            abs
        }
    };
    let function = str_field(frame, "function");
    match platform {
        "python" => {
            !(path.contains("site-packages")
                || path.contains("dist-packages")
                || path.starts_with("/usr/lib/python")
                || path.starts_with("<frozen")
                || path.is_empty())
        }
        "javascript" | "node" => {
            !(path.contains("/node_modules/")
                || path.starts_with("node:")
                || path.starts_with("internal/")
                || path == "<anonymous>"
                || path.is_empty())
        }
        "cocoa" | "native" | "objc" | "swift" | "c" => {
            let package = str_field(frame, "package");
            package.contains(".app/") && !package.contains(".app/Frameworks/")
        }
        "rust" => !function.is_empty() && !system_function(function),
        _ => false,
    }
}

/// A name and a version (which may be empty).
pub type NameVersion = (String, String);

/// Browser and OS from a user agent. Enough to fill the browser and OS
/// contexts; the user agent itself is not kept.
pub fn parse_user_agent(ua: &str) -> (Option<NameVersion>, Option<NameVersion>) {
    let version_after = |marker: &str| -> Option<String> {
        let start = ua.find(marker)? + marker.len();
        let v: String = ua[start..]
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '_')
            .collect::<String>()
            .replace('_', ".");
        let major_minor: Vec<&str> = v.split('.').take(2).collect();
        Some(major_minor.join(".")).filter(|v| !v.is_empty())
    };
    let browser = [
        ("Edg/", "Edge"),
        ("OPR/", "Opera"),
        ("Firefox/", "Firefox"),
        ("Chrome/", "Chrome"),
        ("CriOS/", "Chrome Mobile iOS"),
        ("Version/", "Safari"),
    ]
    .iter()
    .find_map(|(marker, name)| version_after(marker).map(|v| (name.to_string(), v)));
    let os = if ua.contains("iPhone") || ua.contains("iPad") {
        let v = version_after("OS ").unwrap_or_default();
        Some(("iOS".to_string(), v))
    } else if ua.contains("Android") {
        Some((
            "Android".to_string(),
            version_after("Android ").unwrap_or_default(),
        ))
    } else if ua.contains("Windows") {
        Some(("Windows".to_string(), String::new()))
    } else if ua.contains("Mac OS X") || ua.contains("Macintosh") {
        Some(("macOS".to_string(), String::new()))
    } else if ua.contains("CrOS") {
        Some(("Chrome OS".to_string(), String::new()))
    } else if ua.contains("Linux") {
        Some(("Linux".to_string(), String::new()))
    } else {
        None
    };
    (browser, os)
}

/// Headers as `[name, value]` pairs or an object → an object; credentials
/// filtered; IP headers removed; the user agent removed after it filled the
/// browser and OS contexts (returned).
fn headers(value: &mut Value) -> Option<String> {
    let pairs: Vec<(String, Value)> = match value.take() {
        Value::Object(map) => map.into_iter().collect(),
        Value::Array(items) => items
            .into_iter()
            .filter_map(|item| match item {
                Value::Array(mut pair) if pair.len() == 2 => {
                    let v = pair.pop()?;
                    let k = pair.pop()?.as_str()?.to_string();
                    Some((k, v))
                }
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let mut user_agent = None;
    let mut out = Map::new();
    for (name, v) in pairs {
        if scrub::is_ip_key(&name) {
            continue;
        }
        if name.eq_ignore_ascii_case("user-agent") {
            user_agent = v.as_str().map(str::to_string);
            continue;
        }
        let v = if scrub::is_secret_header(&name) {
            Value::from(FILTERED)
        } else {
            v
        };
        out.insert(name, v);
    }
    *value = Value::Object(out);
    user_agent
}

/// The request interface: headers, cookies, env, query and URL scrubbed.
fn request(map: &mut Map<String, Value>) {
    let Some(Value::Object(req)) = map.get_mut("request") else {
        map.remove("request");
        return;
    };
    let mut user_agent = None;
    if let Some(h) = req.get_mut("headers") {
        user_agent = headers(h);
    }
    if req.contains_key("cookies") {
        req.insert("cookies".into(), Value::from(FILTERED));
    }
    if let Some(Value::Object(env)) = req.get_mut("env") {
        env.retain(|k, _| !scrub::is_ip_key(k) && !k.eq_ignore_ascii_case("HTTP_USER_AGENT"));
    }
    match req.get_mut("query_string") {
        Some(Value::String(q)) => {
            if let Some(clean) = scrub::scrub_query(q.trim_start_matches('?')) {
                *q = clean;
            }
        }
        Some(Value::Array(pairs)) => {
            for pair in pairs.iter_mut() {
                if let Value::Array(kv) = pair
                    && kv.len() == 2
                    && kv[0]
                        .as_str()
                        .is_some_and(|k| scrub::is_sensitive_key(k) || scrub::is_ip_key(k))
                {
                    kv[1] = Value::from(FILTERED);
                }
            }
        }
        _ => {}
    }
    if let Some(Value::Object(q)) = req.get_mut("query_string") {
        let mut v = Value::Object(std::mem::take(q));
        scrub::scrub_keys(&mut v);
        if let Value::Object(clean) = v {
            *q = clean;
        }
    }
    if let Some(Value::String(url)) = req.get_mut("url")
        && let Some(clean) = scrub::scrub_query_in_url(url)
    {
        *url = clean;
    }
    for key in ["data", "env", "other"] {
        if let Some(v) = req.get_mut(key) {
            scrub::scrub_keys(v);
        }
    }
    if let Some(ua) = user_agent {
        fill_from_user_agent(map, &ua);
    }
}

fn fill_from_user_agent(map: &mut Map<String, Value>, ua: &str) {
    let (browser, os) = parse_user_agent(ua);
    let contexts = map
        .entry("contexts")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(contexts) = contexts.as_object_mut() else {
        return;
    };
    if let Some((name, version)) = browser
        && !contexts.contains_key("browser")
    {
        contexts.insert(
            "browser".into(),
            json!({"name": name, "version": version, "type": "browser"}),
        );
    }
    if let Some((name, version)) = os
        && !contexts.contains_key("os")
        && !contexts.contains_key("client_os")
    {
        let mut os = json!({"name": name, "type": "os"});
        if !version.is_empty() {
            os["version"] = Value::from(version);
        }
        contexts.insert("client_os".into(), os);
    }
}

/// The user interface: never an IP address; email and username hashed
/// unless the project keeps them; other data scrubbed by key.
fn user(map: &mut Map<String, Value>, ctx: &Context<'_>, facts: &mut Facts) {
    let Some(Value::Object(user)) = map.get_mut("user") else {
        map.remove("user");
        return;
    };
    user.retain(|k, _| !scrub::is_ip_key(k));
    let text = |v: Option<&Value>| short_string(v, 512);
    let identity = text(user.get("id"))
        .map(|v| ("id", v))
        .or_else(|| text(user.get("email")).map(|v| ("email", v)))
        .or_else(|| text(user.get("username")).map(|v| ("username", v)));
    facts.user_hash =
        identity.map(|(_, v)| scrub::keyed_hash(ctx.hash_key, ctx.project_id, "user", &v));
    if !ctx.keep_user_details {
        for field in ["email", "username"] {
            if let Some(v) = text(user.get(field)) {
                let hashed = scrub::keyed_hash(ctx.hash_key, ctx.project_id, field, &v);
                user.insert(field.into(), Value::from(format!("hash:{hashed}")));
            }
        }
    }
    let mut v = Value::Object(std::mem::take(user));
    scrub::scrub_keys(&mut v);
    if let (Some(Value::Object(slot)), Value::Object(clean)) = (map.get_mut("user"), v) {
        *slot = clean;
    }
}

/// Tags as an object of short strings (from an object or `[key, value]`
/// pairs); sensitive keys filtered.
fn tags(map: &mut Map<String, Value>) {
    let pairs: Vec<(String, Value)> = match map.remove("tags") {
        Some(Value::Object(obj)) => obj.into_iter().collect(),
        Some(Value::Array(items)) => items
            .into_iter()
            .filter_map(|item| match item {
                Value::Array(mut pair) if pair.len() == 2 => {
                    let v = pair.pop()?;
                    let k = pair.pop()?.as_str()?.to_string();
                    Some((k, v))
                }
                _ => None,
            })
            .collect(),
        _ => return,
    };
    let mut out = Map::new();
    for (key, value) in pairs {
        let key = key.trim().to_string();
        if key.is_empty() || key.chars().count() > MAX_TAG_KEY || scrub::is_ip_key(&key) {
            continue;
        }
        let Some(value) = short_string(Some(&value), MAX_TAG_VALUE).or_else(|| match &value {
            Value::Bool(b) => Some(b.to_string()),
            _ => None,
        }) else {
            continue;
        };
        let value = if scrub::is_sensitive_key(&key) {
            FILTERED.to_string()
        } else {
            value
        };
        out.insert(key, Value::String(value));
    }
    if !out.is_empty() {
        map.insert("tags".into(), Value::Object(out));
    }
}

/// Cuts every string longer than [`MAX_STRING`] bytes.
fn trim_strings(value: &mut Value) {
    match value {
        Value::String(s) if s.len() > MAX_STRING => {
            let mut end = MAX_STRING;
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            s.truncate(end);
            s.push('…');
        }
        Value::Array(items) => items.iter_mut().for_each(trim_strings),
        Value::Object(map) => map.values_mut().for_each(trim_strings),
        _ => {}
    }
}

/// Every stack trace of the event: exception values, threads, and a
/// top-level one (moved into a thread).
fn stacktraces(map: &mut Map<String, Value>, platform: &str) {
    if let Some(st) = map.remove("stacktrace") {
        let has_threads = map
            .get("threads")
            .and_then(|t| t.get("values"))
            .and_then(Value::as_array)
            .is_some_and(|v| !v.is_empty());
        if !has_threads {
            map.insert(
                "threads".into(),
                json!({"values": [{"id": 0, "current": true, "crashed": false, "stacktrace": st}]}),
            );
        }
    }
    for key in ["exception", "threads"] {
        if let Some(Value::Array(values)) = map.get_mut(key).and_then(|v| v.get_mut("values")) {
            values.retain(Value::is_object);
            for value in values.iter_mut() {
                if let Some(st) = value.get_mut("stacktrace") {
                    stacktrace(st, platform);
                }
                if let Some(st) = value.get_mut("raw_stacktrace") {
                    stacktrace(st, platform);
                }
            }
        }
    }
}

/// Free-text scrubbing of the whole event except the parts that are ids,
/// addresses or code: `debug_meta`, `sdk`, the trace context and stack
/// frames (whose local variables and source lines are scrubbed).
fn scrub_free_text(map: &mut Map<String, Value>) {
    for (key, value) in map.iter_mut() {
        match key.as_str() {
            "event_id" | "timestamp" | "start_timestamp" | "debug_meta" | "sdk" | "fingerprint"
            | "platform" | "level" | "release" | "dist" | "environment" => {}
            "exception" | "threads" => {
                if let Some(Value::Array(values)) = value.get_mut("values") {
                    for v in values.iter_mut().filter_map(Value::as_object_mut) {
                        for (k, field) in v.iter_mut() {
                            match k.as_str() {
                                "stacktrace" | "raw_stacktrace" => scrub_frame_vars(field),
                                "mechanism" | "module" | "id" | "thread_id" => {}
                                _ => scrub::scrub_strings(field, false),
                            }
                        }
                    }
                }
            }
            "contexts" => {
                if let Value::Object(contexts) = value {
                    for (name, ctx) in contexts.iter_mut() {
                        if name != "trace" {
                            scrub::scrub_strings(ctx, false);
                        }
                    }
                }
            }
            _ => scrub::scrub_strings(value, true),
        }
    }
}

/// Frame variables are scrubbed by key and by text; source lines by text
/// (a card number or token written into the code).
fn scrub_frame_vars(stacktrace: &mut Value) {
    if let Some(Value::Array(frames)) = stacktrace.get_mut("frames") {
        for frame in frames.iter_mut() {
            if let Some(vars) = frame.get_mut("vars") {
                scrub::scrub_keys(vars);
                scrub::scrub_strings(vars, true);
            }
            for key in ["pre_context", "context_line", "post_context"] {
                if let Some(lines) = frame.get_mut(key) {
                    scrub::scrub_strings(lines, false);
                }
            }
        }
    }
}

/// Normalises `event` in place. Returns what the index row needs to know.
pub fn normalize(event: &mut Map<String, Value>, ctx: &Context<'_>) -> Facts {
    let mut facts = Facts::default();
    let received = ts_seconds(ctx.received);

    let ts = event
        .get("timestamp")
        .and_then(seconds)
        .filter(|t| *t <= received + MAX_FUTURE_SECS && *t >= received - MAX_AGE_SECS)
        .unwrap_or(received);
    event.insert("timestamp".into(), json!((ts * 1000.0).round() / 1000.0));
    event.insert("received".into(), json!(received));

    let lvl = level(event.get("level"));
    event.insert("level".into(), Value::from(lvl));
    let platform = short_string(event.get("platform"), 32).unwrap_or_else(|| "other".into());
    event.insert("platform".into(), Value::from(platform.clone()));

    match short_string(event.get("release"), 200) {
        Some(r) => event.insert("release".into(), Value::from(r)),
        None => event.remove("release"),
    };
    match short_string(event.get("dist"), 64) {
        Some(d) => event.insert("dist".into(), Value::from(d)),
        None => event.remove("dist"),
    };
    let env = short_string(event.get("environment"), 64)
        .filter(|e| !e.contains(['\n', '/']))
        .unwrap_or_else(|| "production".into());
    event.insert("environment".into(), Value::from(env));
    for key in ["server_name", "transaction", "culprit", "logger"] {
        match short_string(event.get(key), 200) {
            Some(v) => event.insert(key.into(), Value::from(v)),
            None => event.remove(key),
        };
    }

    logentry(event);
    values_form(event, "exception");
    values_form(event, "threads");
    values_form(event, "breadcrumbs");
    if let Some(Value::Array(crumbs)) = event
        .get_mut("breadcrumbs")
        .and_then(|b| b.get_mut("values"))
    {
        if crumbs.len() > MAX_BREADCRUMBS {
            crumbs.drain(..crumbs.len() - MAX_BREADCRUMBS);
        }
        for crumb in crumbs.iter_mut() {
            if let Some(data) = crumb.get_mut("data") {
                scrub::scrub_keys(data);
            }
        }
    }
    stacktraces(event, &platform);

    if let Some(fp) = event.get("fingerprint") {
        let parts: Vec<Value> = fp
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| short_string(Some(v), 200))
                    .map(Value::from)
                    .collect()
            })
            .unwrap_or_default();
        if parts.is_empty() {
            event.remove("fingerprint");
        } else {
            event.insert("fingerprint".into(), Value::Array(parts));
        }
    }

    // Privacy: IP addresses, the request, the user, tags, extra data,
    // contexts; then card numbers and credentials in all free text.
    if let Some(Value::Object(sdk)) = event.get_mut("sdk") {
        sdk.retain(|k, _| !scrub::is_ip_key(k));
    }
    request(event);
    user(event, ctx, &mut facts);
    tags(event);
    for key in ["extra", "contexts"] {
        if let Some(v) = event.get_mut(key) {
            scrub::scrub_keys(v);
        }
    }
    event.retain(|k, _| !scrub::is_ip_key(k));
    scrub_free_text(event);
    for value in event.values_mut() {
        trim_strings(value);
    }
    facts
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 32] = [1; 32];

    fn ctx(keep: bool) -> Context<'static> {
        Context {
            project_id: 1,
            keep_user_details: keep,
            hash_key: &KEY,
            received: "2026-10-22T12:00:00Z".parse().unwrap(),
        }
    }

    fn norm(v: Value) -> Value {
        let Value::Object(mut map) = v else { panic!() };
        normalize(&mut map, &ctx(false));
        Value::Object(map)
    }

    #[test]
    fn defaults_are_filled_in() {
        let e = norm(json!({"message": "hi"}));
        assert_eq!(e["level"], "error");
        assert_eq!(e["platform"], "other");
        assert_eq!(e["environment"], "production");
        assert_eq!(e["logentry"]["formatted"], "hi");
        assert!(e.get("message").is_none());
        assert_eq!(e["timestamp"], json!(1792670400.0));
    }

    #[test]
    fn times_are_read_and_clamped() {
        let e = norm(json!({"timestamp": "2026-10-22T11:59:00.5Z"}));
        assert_eq!(e["timestamp"], json!(1792670340.5));
        let e = norm(json!({"timestamp": 1792670340.25}));
        assert_eq!(e["timestamp"], json!(1792670340.25));
        // A day in the future and a year ago both become the receive time.
        for t in [1792670400.0 + 86400.0, 1792670400.0 - 365.0 * 86400.0] {
            assert_eq!(
                norm(json!({ "timestamp": t }))["timestamp"],
                json!(1792670400.0)
            );
        }
        assert_eq!(sql_time(1792670340.5), "2026-10-22 11:59:00");
    }

    #[test]
    fn templates_are_formatted() {
        let e = norm(
            json!({"logentry": {"message": "Order %s failed after %d tries", "params": ["A-1", 3]}}),
        );
        assert_eq!(e["logentry"]["formatted"], "Order A-1 failed after 3 tries");
        assert_eq!(e["logentry"]["message"], "Order %s failed after %d tries");
    }

    #[test]
    fn breadcrumbs_and_frames_are_bounded() {
        let crumbs: Vec<Value> = (0..150)
            .map(|i| json!({"message": format!("c{i}")}))
            .collect();
        let frames: Vec<Value> = (0..300)
            .map(|i| json!({"function": format!("f{i}")}))
            .collect();
        let e = norm(json!({
            "breadcrumbs": crumbs,
            "exception": [{"type": "E", "stacktrace": {"frames": frames}}],
            "extra": {"big": "x".repeat(20_000)},
        }));
        let crumbs = e["breadcrumbs"]["values"].as_array().unwrap();
        assert_eq!(crumbs.len(), MAX_BREADCRUMBS);
        assert_eq!(crumbs[0]["message"], "c50");
        let frames = e["exception"]["values"][0]["stacktrace"]["frames"]
            .as_array()
            .unwrap();
        assert_eq!(frames.len(), MAX_FRAMES);
        assert_eq!(frames.last().unwrap()["function"], "f299");
        assert!(e["extra"]["big"].as_str().unwrap().len() <= MAX_STRING + 3);
    }

    #[test]
    fn in_app_is_derived_per_platform() {
        let frame = |v: Value| v.as_object().unwrap().clone();
        assert!(derive_in_app(
            &frame(json!({"abs_path": "/app/main.py"})),
            "python"
        ));
        assert!(!derive_in_app(
            &frame(json!({"abs_path": "/usr/local/lib/python3.13/site-packages/flask/app.py"})),
            "python"
        ));
        assert!(!derive_in_app(
            &frame(json!({"filename": "/srv/node_modules/x/index.js"})),
            "node"
        ));
        assert!(!derive_in_app(
            &frame(json!({"filename": "node:internal/process"})),
            "javascript"
        ));
        assert!(derive_in_app(
            &frame(json!({"filename": "/srv/app/index.js"})),
            "javascript"
        ));
        assert!(derive_in_app(
            &frame(json!({"package": "/var/Bundle/Application/X/My.app/My"})),
            "cocoa"
        ));
        assert!(!derive_in_app(
            &frame(
                json!({"package": "/var/Bundle/Application/X/My.app/Frameworks/Sentry.framework/Sentry"})
            ),
            "cocoa"
        ));
        assert!(!derive_in_app(
            &frame(json!({"function": "std::panicking::begin_panic"})),
            "rust"
        ));
        assert!(derive_in_app(
            &frame(json!({"function": "myapp::handler"})),
            "rust"
        ));
        // The SDK's value wins.
        let e = norm(
            json!({"platform": "python", "exception": {"values": [{"stacktrace": {"frames": [
            {"abs_path": "/usr/lib/python3/x.py", "in_app": true}]}}]}}),
        );
        assert_eq!(
            e["exception"]["values"][0]["stacktrace"]["frames"][0]["in_app"],
            true
        );
        // …except for Rust's standard library and SDK frames.
        let e = norm(
            json!({"platform": "native", "exception": {"values": [{"stacktrace": {"frames": [
            {"function": "core[ed37fd7903368ee]::panicking::panic_fmt", "in_app": true},
            {"function": "sentry_panic::panic_handler", "in_app": true},
            {"function": "myapp::main", "in_app": true}]}}]}}),
        );
        let frames = &e["exception"]["values"][0]["stacktrace"]["frames"];
        assert_eq!(frames[0]["in_app"], false);
        assert_eq!(frames[1]["in_app"], false);
        assert_eq!(frames[2]["in_app"], true);
    }

    #[test]
    fn no_ip_ever_and_hashed_user_details() {
        let e = norm(json!({
            "user": {"id": "42", "email": "ada@example.com", "username": "ada", "ip_address": "{{auto}}"},
            "sdk": {"name": "x", "client_ip": "10.9.8.7"},
            "request": {
                "url": "https://shop.example.com/pay?card=4111111111111111&page=2",
                "headers": [["Authorization", "Bearer s3cr3t"], ["X-Forwarded-For", "10.9.8.7"],
                            ["User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36"],
                            ["Accept", "*/*"], ["Cookie", "session=abc"]],
                "env": {"REMOTE_ADDR": "10.9.8.7", "SERVER_NAME": "web"},
                "query_string": "page=2&token=abc",
                "cookies": {"session": "abc"},
                "data": {"password": "hunter2", "note": "card 4111-1111-1111-1111"},
            },
        }));
        let text = e.to_string();
        for secret in [
            "10.9.8.7",
            "ada@example.com",
            "s3cr3t",
            "hunter2",
            "4111",
            "session=abc",
            "Mozilla",
            "{{auto}}",
        ] {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
        assert!(e["user"]["email"].as_str().unwrap().starts_with("hash:"));
        assert_eq!(e["user"]["id"], "42");
        assert_eq!(e["request"]["headers"]["Authorization"], FILTERED);
        assert_eq!(e["request"]["headers"]["Accept"], "*/*");
        assert_eq!(e["request"]["query_string"], "page=2&token=[Filtered]");
        assert_eq!(e["request"]["env"]["SERVER_NAME"], "web");
        assert_eq!(e["contexts"]["browser"]["name"], "Chrome");
        assert_eq!(e["contexts"]["browser"]["version"], "141.0");
        assert_eq!(e["contexts"]["client_os"]["name"], "Windows");
    }

    #[test]
    fn projects_can_keep_user_details() {
        let Value::Object(mut map) =
            json!({"user": {"email": "ada@example.com", "ip_address": "1.2.3.4"}})
        else {
            panic!()
        };
        let facts = normalize(&mut map, &ctx(true));
        assert_eq!(map["user"]["email"], "ada@example.com");
        assert!(map["user"].get("ip_address").is_none());
        assert_eq!(facts.user_hash.unwrap().len(), 16);
    }

    #[test]
    fn extra_contexts_breadcrumbs_and_vars_are_scrubbed() {
        let e = norm(json!({
            "extra": {"db_password": "x", "nested": {"stripe_key": "sk_live"}},
            "contexts": {"app": {"auth_token": "t"}, "trace": {"trace_id": "4111111111111111"}},
            "breadcrumbs": [{"message": "paid 4111111111111111", "data": {"secret": 1, "url": "/x?token=y"}}],
            "exception": [{"type": "E", "value": "card 4111111111111111", "stacktrace": {"frames": [
                {"function": "f", "vars": {"password": "x", "n": "4111111111111111"}}]}}],
            "tags": {"api_token": "abc", "shop": "north"},
        }));
        assert_eq!(e["extra"]["db_password"], FILTERED);
        assert_eq!(e["extra"]["nested"]["stripe_key"], FILTERED);
        assert_eq!(e["contexts"]["app"]["auth_token"], FILTERED);
        assert_eq!(e["contexts"]["trace"]["trace_id"], "4111111111111111");
        assert_eq!(e["breadcrumbs"]["values"][0]["message"], "paid [Filtered]");
        assert_eq!(e["breadcrumbs"]["values"][0]["data"]["secret"], FILTERED);
        assert_eq!(
            e["breadcrumbs"]["values"][0]["data"]["url"],
            "/x?token=[Filtered]"
        );
        assert_eq!(e["exception"]["values"][0]["value"], "card [Filtered]");
        let frame = &e["exception"]["values"][0]["stacktrace"]["frames"][0];
        assert_eq!(frame["vars"]["password"], FILTERED);
        assert_eq!(frame["vars"]["n"], FILTERED);
        assert_eq!(e["tags"]["api_token"], FILTERED);
        assert_eq!(e["tags"]["shop"], "north");
    }

    #[test]
    fn user_agents() {
        let (b, o) = parse_user_agent(
            "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1",
        );
        assert_eq!(b, Some(("Safari".into(), "18.1".into())));
        assert_eq!(o, Some(("iOS".into(), "18.1".into())));
        let (b, o) = parse_user_agent(
            "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0",
        );
        assert_eq!(b, Some(("Firefox".into(), "131.0".into())));
        assert_eq!(o, Some(("Linux".into(), String::new())));
    }
}
