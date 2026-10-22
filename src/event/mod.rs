//! From an SDK's event to what the writer stores: normalisation and privacy
//! ([`normalize`]), grouping ([`grouping`]), title, culprit and tags.

pub mod grouping;
pub mod normalize;
pub mod scrub;

use serde_json::{Map, Value};

use crate::db::issues::{EventRecord, STANDARD_TAGS};
pub use normalize::Context;

/// Longest title and culprit.
const MAX_TITLE: usize = 200;
/// Custom tags taken from one event.
const MAX_CUSTOM_TAGS: usize = 20;

/// Why an event can't be taken.
#[derive(Debug, PartialEq, Eq)]
pub enum EventError {
    /// Not JSON, or not a JSON object.
    Invalid(String),
}

impl std::fmt::Display for EventError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EventError::Invalid(why) => write!(f, "invalid event: {why}"),
        }
    }
}

/// Parses, normalises and groups one event. `fallback_id` is the id from
/// the envelope or item header, used when the payload has none.
pub fn prepare(
    raw: &[u8],
    ctx: &Context<'_>,
    fallback_id: Option<&str>,
) -> Result<EventRecord, EventError> {
    let value: Value =
        serde_json::from_slice(raw).map_err(|e| EventError::Invalid(e.to_string()))?;
    let Value::Object(mut event) = value else {
        return Err(EventError::Invalid("not a JSON object".into()));
    };
    let event_id = event
        .get("event_id")
        .and_then(Value::as_str)
        .and_then(normalize::event_id)
        .or_else(|| fallback_id.and_then(normalize::event_id))
        .unwrap_or_else(normalize::new_event_id);
    event.insert("event_id".into(), Value::from(event_id.clone()));

    let facts = normalize::normalize(&mut event, ctx);
    let grouping = grouping::group(&event);
    let title = title(&event);
    let culprit = culprit(&event);
    let tags = tags(&event);
    let field = |key: &str| event.get(key).and_then(Value::as_str).map(str::to_string);
    let timestamp = event
        .get("timestamp")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    let payload = crate::store::compress(&serde_json::to_vec(&event).unwrap_or_default());
    Ok(EventRecord {
        project_id: ctx.project_id,
        event_id,
        timestamp: normalize::sql_time(timestamp),
        received_at: normalize::sql_time(normalize::ts_seconds(ctx.received)),
        level: field("level").unwrap_or_else(|| "error".into()),
        platform: field("platform").unwrap_or_else(|| "other".into()),
        release: field("release"),
        environment: field("environment").unwrap_or_else(|| "production".into()),
        title,
        culprit,
        user_hash: facts.user_hash,
        tags,
        grouping_config: grouping.config,
        hash: grouping.hash,
        components: grouping.components.to_string(),
        unsymbolicated: grouping.unsymbolicated,
        payload,
    })
}

fn cut(s: &str, max: usize) -> String {
    let line = s
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    let mut out: String = line.chars().take(max).collect();
    if line.chars().count() > max {
        out.push('…');
    }
    out
}

fn text<'a>(map: &'a Map<String, Value>, key: &str) -> &'a str {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
}

/// The exception that names the event: the last in the chain.
fn main_exception(event: &Map<String, Value>) -> Option<&Map<String, Value>> {
    event
        .get("exception")?
        .get("values")?
        .as_array()?
        .iter()
        .rev()
        .find_map(Value::as_object)
}

/// `Type: value` of the main exception, else the message, else a
/// placeholder.
pub fn title(event: &Map<String, Value>) -> String {
    if let Some(exc) = main_exception(event) {
        let ty = text(exc, "type");
        let value = text(exc, "value");
        let title = match (ty.is_empty(), value.is_empty()) {
            (false, false) => format!("{ty}: {value}"),
            (false, true) => ty.to_string(),
            (true, false) => value.to_string(),
            (true, true) => "Error".to_string(),
        };
        return cut(&title, MAX_TITLE);
    }
    if let Some(entry) = event.get("logentry").and_then(Value::as_object) {
        for key in ["formatted", "message"] {
            let t = text(entry, key);
            if !t.is_empty() {
                return cut(t, MAX_TITLE);
            }
        }
    }
    "<unlabeled event>".to_string()
}

/// Where it happened: the transaction when the SDK set one, else the
/// innermost in-app frame (any frame when none is in-app) of the main
/// exception or the crashed thread, as `module in function`.
pub fn culprit(event: &Map<String, Value>) -> String {
    let transaction = text(event, "transaction");
    if !transaction.is_empty() {
        return cut(transaction, MAX_TITLE);
    }
    let holder = main_exception(event)
        .filter(|e| !grouping::frames(e).is_empty())
        .or_else(|| grouping::main_thread(event));
    let Some(holder) = holder else {
        return String::new();
    };
    let frames = grouping::frames(holder);
    let frame = frames
        .iter()
        .rev()
        .find(|f| grouping::in_app(f))
        .or_else(|| frames.last());
    let Some(frame) = frame else {
        return String::new();
    };
    let mut unsym = false;
    let Some(part) = grouping::frame_component(frame, &[], &mut unsym) else {
        return String::new();
    };
    let s = |k: &str| part.get(k).and_then(Value::as_str).unwrap_or("");
    let culprit = if unsym {
        match s("offset") {
            "" => s("image").to_string(),
            offset => format!("{} +{offset}", s("image")),
        }
    } else {
        match (s("module"), s("function")) {
            ("", f) => f.to_string(),
            (m, "") => m.to_string(),
            (m, f) => format!("{m} in {f}"),
        }
    };
    cut(&culprit, MAX_TITLE)
}

/// The standard tags Kestrel derives, then at most 20 of the SDK's own.
pub fn tags(event: &Map<String, Value>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut add = |key: &str, value: &str| {
        let value = value.trim();
        if !value.is_empty() && !out.iter().any(|(k, _)| k == key) {
            out.push((key.to_string(), value.chars().take(200).collect()));
        }
    };
    for key in [
        "level",
        "environment",
        "release",
        "dist",
        "server_name",
        "transaction",
    ] {
        add(key, text(event, key));
    }
    let contexts = event.get("contexts").and_then(Value::as_object);
    let ctx = |name: &str| {
        contexts
            .and_then(|c| c.get(name))
            .and_then(Value::as_object)
    };
    let joined = |c: &Map<String, Value>| {
        let name = text(c, "name");
        let version = text(c, "version");
        if version.is_empty() {
            name.to_string()
        } else {
            format!("{name} {version}")
        }
    };
    for (name, key) in [("os", "os.name"), ("client_os", "client_os.name")] {
        if let Some(os) = ctx(name) {
            add(key, text(os, "name"));
            if !text(os, "name").is_empty() {
                add(name, &joined(os));
            }
        }
    }
    if let Some(browser) = ctx("browser") {
        add("browser.name", text(browser, "name"));
        if !text(browser, "name").is_empty() {
            add("browser", &joined(browser));
        }
    }
    if let Some(device) = ctx("device") {
        add("device", text(device, "model"));
        add("device.family", text(device, "family"));
    }
    if let Some(runtime) = ctx("runtime") {
        add("runtime.name", text(runtime, "name"));
        if !text(runtime, "name").is_empty() {
            add("runtime", &joined(runtime));
        }
    }
    if let Some(mechanism) = main_exception(event)
        .and_then(|e| e.get("mechanism"))
        .and_then(Value::as_object)
    {
        if let Some(handled) = mechanism.get("handled").and_then(Value::as_bool) {
            add("handled", if handled { "yes" } else { "no" });
        }
        add("mechanism", text(mechanism, "type"));
    }
    if let Some(custom) = event.get("tags").and_then(Value::as_object) {
        let mut keys: Vec<&String> = custom
            .keys()
            .filter(|k| !STANDARD_TAGS.contains(&k.as_str()))
            .collect();
        keys.sort();
        for key in keys.into_iter().take(MAX_CUSTOM_TAGS) {
            if let Some(v) = custom.get(key).and_then(Value::as_str) {
                add(key, v);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const KEY: [u8; 32] = [3; 32];

    fn prep(v: Value) -> EventRecord {
        let ctx = Context {
            project_id: 9,
            keep_user_details: false,
            hash_key: &KEY,
            received: "2026-10-22T12:00:00Z".parse().unwrap(),
        };
        prepare(v.to_string().as_bytes(), &ctx, None).unwrap()
    }

    #[test]
    fn titles_culprits_and_tags() {
        let r = prep(json!({
            "event_id": "9EC79C33-EC99-42AB-8353-589FCB2E04DC",
            "platform": "python",
            "release": "shop@1.2.0",
            "tags": {"shop": "north", "level": "ignored"},
            "contexts": {"os": {"name": "Linux", "version": "6.1"}, "runtime": {"name": "CPython", "version": "3.13.1"}},
            "exception": {"values": [
                {"type": "KeyError", "value": "'id'"},
                {"type": "ValueError", "value": "bad input\nsecond line", "mechanism": {"type": "generic", "handled": false},
                 "stacktrace": {"frames": [
                    {"module": "flask.app", "function": "dispatch", "abs_path": "/venv/lib/python3.13/site-packages/flask/app.py"},
                    {"module": "shop.views", "function": "checkout", "abs_path": "/srv/shop/views.py"}]}}]},
        }));
        assert_eq!(r.event_id, "9ec79c33ec9942ab8353589fcb2e04dc");
        assert_eq!(r.title, "ValueError: bad input");
        assert_eq!(r.culprit, "shop.views in checkout");
        assert_eq!(r.level, "error");
        assert_eq!(r.release.as_deref(), Some("shop@1.2.0"));
        assert_eq!(r.timestamp, "2026-10-22 12:00:00");
        let tags: Vec<String> = r.tags.iter().map(|(k, v)| format!("{k}={v}")).collect();
        assert_eq!(
            tags,
            [
                "level=error",
                "environment=production",
                "release=shop@1.2.0",
                "os.name=Linux",
                "os=Linux 6.1",
                "runtime.name=CPython",
                "runtime=CPython 3.13.1",
                "handled=no",
                "mechanism=generic",
                "shop=north"
            ]
        );
    }

    #[test]
    fn messages_and_empty_events_have_titles() {
        assert_eq!(
            prep(json!({"message": "Disk almost full"})).title,
            "Disk almost full"
        );
        let r = prep(json!({}));
        assert_eq!(r.title, "<unlabeled event>");
        assert_eq!(r.culprit, "");
        assert_eq!(r.event_id.len(), 32);
    }

    #[test]
    fn invalid_events_are_refused() {
        let ctx = Context {
            project_id: 1,
            keep_user_details: false,
            hash_key: &KEY,
            received: jiff::Timestamp::now(),
        };
        assert!(prepare(b"[1, 2]", &ctx, None).is_err());
        assert!(prepare(b"{nope", &ctx, None).is_err());
    }
}
