//! An event's stored payload (normalised and scrubbed at ingest) turned into
//! what the issue and event pages show: the exception chain with its stack
//! traces, breadcrumbs, request, contexts, user, tags, SDK and the raw JSON.
//!
//! Stack traces are shown innermost frame first. In the default "in-app"
//! view, runs of frames outside the app fold into one collapsed group; the
//! "all frames" view shows every frame. A trace without in-app frames is
//! always shown in full.
//!
//! Native frames symbolicated at ingest show their function, file and line;
//! an inlined function shows as its own frame, marked inlined. An in-app
//! native frame that wasn't symbolicated names the debug id of its image, so
//! the page can say which dSYM was missing.

use serde_json::{Map, Value};

use crate::difs::symbolicate::{self, Image};
use crate::event::grouping;
use crate::event::normalize;

/// One line of source around a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeLine {
    pub no: String,
    pub code: String,
    /// The frame's own line.
    pub current: bool,
}

/// One stack frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameView {
    /// The function, demangled; for a native frame without symbols the
    /// image and offset.
    pub function: String,
    /// `file:line:column`, else the module.
    pub location: String,
    /// The image or package the frame is in, if any.
    pub package: String,
    pub in_app: bool,
    /// The instruction address of a native frame.
    pub address: String,
    pub context: Vec<CodeLine>,
    /// Local variables (scrubbed at ingest).
    pub vars: Vec<(String, String)>,
    /// Shown expanded: the innermost in-app frame with source or variables.
    pub open: bool,
    /// Resolved from a debug file at ingest.
    pub symbolicated: bool,
    /// A function inlined into the frame below it.
    pub inlined: bool,
    /// For an in-app native frame without symbols: the debug id of its
    /// image, whose dSYM was missing when the event arrived.
    pub missing_dsym: String,
}

impl FrameView {
    pub fn has_details(&self) -> bool {
        !self.context.is_empty() || !self.vars.is_empty()
    }
}

/// A frame, or a folded run of frames outside the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameGroup {
    Frame(FrameView),
    System(Vec<FrameView>),
}

/// One exception of the chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExceptionView {
    pub ty: String,
    pub value: String,
    pub module: String,
    /// The mechanism's type (`generic`, `mach`, `panic` …).
    pub mechanism: String,
    pub handled: Option<bool>,
    pub frames: Vec<FrameGroup>,
    /// Frames before folding.
    pub frame_count: usize,
}

/// A thread's stack (the crashed thread when the exceptions have none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadView {
    pub name: String,
    pub crashed: bool,
    pub frames: Vec<FrameGroup>,
    pub frame_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crumb {
    /// `HH:MM:SS` UTC.
    pub time: String,
    /// The full time, `YYYY-MM-DD HH:MM:SS`.
    pub datetime: String,
    pub category: String,
    pub level: String,
    pub message: String,
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestView {
    pub method: String,
    pub url: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub data: String,
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextView {
    /// The context's key (`device`, `os`, `app` …).
    pub key: String,
    /// Translation key of its title for the known ones, else empty.
    pub title_key: &'static str,
    pub rows: Vec<(String, String)>,
}

/// Everything the pages show of one event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventView {
    pub message: String,
    /// The main exception first, then the ones that led to it.
    pub exceptions: Vec<ExceptionView>,
    pub thread: Option<ThreadView>,
    /// Newest first.
    pub breadcrumbs: Vec<Crumb>,
    pub request: Option<RequestView>,
    pub contexts: Vec<ContextView>,
    pub user: Vec<(String, String)>,
    pub tags: Vec<(String, String)>,
    pub sdk: String,
    pub sdk_integrations: String,
    pub extra: Vec<(String, String)>,
    pub fingerprint: String,
    pub platform: String,
    pub server_name: String,
    pub transaction: String,
    pub dist: String,
    /// Some frame anywhere is in-app (so the in-app view means something).
    pub has_in_app: bool,
    /// The payload, pretty-printed as stored.
    pub raw: String,
}

/// Longest value shown in a table cell before it is cut.
const MAX_CELL: usize = 500;

fn text<'a>(map: &'a Map<String, Value>, key: &str) -> &'a str {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
}

fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// A value as one line of text: strings as they are, the rest as JSON.
fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => cut(s, MAX_CELL),
        Value::Null => String::new(),
        other => cut(&other.to_string(), MAX_CELL),
    }
}

/// An object's fields as rows, in its order, without empty ones.
fn rows(map: &Map<String, Value>, skip: &[&str]) -> Vec<(String, String)> {
    map.iter()
        .filter(|(k, _)| !skip.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), scalar(v)))
        .filter(|(_, v)| !v.is_empty())
        .collect()
}

fn values<'a>(event: &'a Map<String, Value>, key: &str) -> Vec<&'a Map<String, Value>> {
    match event.get(key) {
        Some(Value::Object(o)) => o
            .get("values")
            .and_then(Value::as_array)
            .map(|v| v.iter().filter_map(Value::as_object).collect())
            .unwrap_or_default(),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_object).collect(),
        _ => Vec::new(),
    }
}

/// `0x…` text of an address field.
fn hex(v: Option<&Value>) -> Option<u64> {
    v.and_then(grouping::address)
}

fn frame_view(frame: &Map<String, Value>, native: bool, images: &[Image]) -> FrameView {
    let symbolicated = symbolicate::is_symbolicated(frame);
    let missing_dsym = if native && !symbolicated && grouping::in_app(frame) {
        symbolicate::image_of(images, frame)
            .and_then(|i| i.debug_id)
            .map(|d| d.to_string())
            .unwrap_or_default()
    } else {
        String::new()
    };
    let raw_function = text(frame, "function");
    let package = text(frame, "package");
    let package_name = grouping::basename(package).to_string();
    let addr = hex(frame.get("instruction_addr"));
    let function = if raw_function.is_empty() || raw_function == "?" {
        match (addr, hex(frame.get("image_addr")).filter(|a| *a > 0)) {
            (Some(a), Some(base)) if a >= base => {
                let name = if package_name.is_empty() {
                    "?".to_string()
                } else {
                    package_name.clone()
                };
                format!("{name} + {:#x}", a - base)
            }
            _ => text(frame, "symbol").to_string(),
        }
    } else if native {
        grouping::demangle(raw_function)
    } else {
        raw_function.to_string()
    };
    let file = match text(frame, "filename") {
        "" => text(frame, "abs_path"),
        f => f,
    };
    let mut location = file.to_string();
    if !location.is_empty() {
        if let Some(line) = frame.get("lineno").and_then(Value::as_i64) {
            location.push_str(&format!(":{line}"));
            if let Some(col) = frame.get("colno").and_then(Value::as_i64) {
                location.push_str(&format!(":{col}"));
            }
        }
    } else {
        location = text(frame, "module").to_string();
    }
    let mut context = Vec::new();
    let current = text(frame, "context_line");
    if let Some(line) = frame.get("lineno").and_then(Value::as_i64)
        && (frame.contains_key("context_line") || frame.contains_key("pre_context"))
    {
        let lines = |key: &str| -> Vec<String> {
            frame
                .get(key)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|l| l.as_str().unwrap_or("").trim_end().to_string())
                        .collect()
                })
                .unwrap_or_default()
        };
        let pre = lines("pre_context");
        let post = lines("post_context");
        let first = line - pre.len() as i64;
        for (i, code) in pre.into_iter().enumerate() {
            context.push(CodeLine {
                no: (first + i as i64).to_string(),
                code: cut(&code, 300),
                current: false,
            });
        }
        context.push(CodeLine {
            no: line.to_string(),
            code: cut(current.trim_end(), 300),
            current: true,
        });
        for (i, code) in post.into_iter().enumerate() {
            context.push(CodeLine {
                no: (line + 1 + i as i64).to_string(),
                code: cut(&code, 300),
                current: false,
            });
        }
    }
    let vars = frame
        .get("vars")
        .and_then(Value::as_object)
        .map(|v| rows(v, &[]))
        .unwrap_or_default();
    FrameView {
        function: if function.is_empty() {
            "?".to_string()
        } else {
            cut(&function, 300)
        },
        location: cut(&location, 300),
        package: package_name,
        in_app: grouping::in_app(frame),
        address: addr.map(|a| format!("{a:#x}")).unwrap_or_default(),
        context,
        vars,
        open: false,
        symbolicated,
        inlined: frame.get("inlined").and_then(Value::as_bool) == Some(true),
        missing_dsym,
    }
}

/// A holder's frames, innermost first, folded for the in-app view.
fn stack(
    holder: &Map<String, Value>,
    native: bool,
    images: &[Image],
    all_frames: bool,
    opened: &mut bool,
) -> (Vec<FrameGroup>, usize) {
    let mut frames: Vec<FrameView> = grouping::frames(holder)
        .into_iter()
        .rev()
        .map(|f| frame_view(f, native, images))
        .collect();
    let count = frames.len();
    if !*opened && let Some(f) = frames.iter_mut().find(|f| f.in_app && f.has_details()) {
        f.open = true;
        *opened = true;
    }
    let fold = !all_frames && frames.iter().any(|f| f.in_app);
    let mut out: Vec<FrameGroup> = Vec::new();
    for f in frames {
        if !fold || f.in_app {
            out.push(FrameGroup::Frame(f));
        } else if let Some(FrameGroup::System(run)) = out.last_mut() {
            run.push(f);
        } else {
            out.push(FrameGroup::System(vec![f]));
        }
    }
    (out, count)
}

/// A breadcrumb or event time (RFC 3339 text or seconds) as
/// `YYYY-MM-DD HH:MM:SS` UTC.
fn time(v: Option<&Value>) -> String {
    match v {
        Some(Value::Number(n)) => n.as_f64().map(normalize::sql_time).unwrap_or_default(),
        Some(Value::String(s)) => s
            .parse::<jiff::Timestamp>()
            .map(|t| normalize::sql_time(normalize::ts_seconds(t)))
            .or_else(|_| s.parse::<f64>().map(normalize::sql_time))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

const KNOWN_CONTEXTS: &[(&str, &str)] = &[
    ("device", "event.ctx_device"),
    ("os", "event.ctx_os"),
    ("app", "event.ctx_app"),
    ("browser", "event.ctx_browser"),
    ("runtime", "event.ctx_runtime"),
];

/// Builds the view of a stored payload; `all_frames` turns folding off.
pub fn build(payload: &Value, all_frames: bool) -> EventView {
    let empty = Map::new();
    let event = payload.as_object().unwrap_or(&empty);
    let platform = text(event, "platform").to_string();
    let native = matches!(
        platform.as_str(),
        "cocoa" | "native" | "objc" | "swift" | "c"
    );
    let images = symbolicate::images(event);
    let mut opened = false;

    let chain = values(event, "exception");
    let mut exceptions: Vec<ExceptionView> = chain
        .iter()
        .rev()
        .map(|e| {
            let (frames, frame_count) = stack(e, native, &images, all_frames, &mut opened);
            let mechanism = e.get("mechanism").and_then(Value::as_object);
            ExceptionView {
                ty: text(e, "type").to_string(),
                value: cut(text(e, "value"), 2000),
                module: text(e, "module").to_string(),
                mechanism: mechanism
                    .map(|m| text(m, "type").to_string())
                    .unwrap_or_default(),
                handled: mechanism
                    .and_then(|m| m.get("handled"))
                    .and_then(Value::as_bool),
                frames,
                frame_count,
            }
        })
        .collect();
    exceptions.retain(|e| !e.ty.is_empty() || !e.value.is_empty() || e.frame_count > 0);

    let thread = if exceptions.iter().all(|e| e.frame_count == 0) {
        grouping::main_thread(event).and_then(|t| {
            let (frames, frame_count) = stack(t, native, &images, all_frames, &mut opened);
            (frame_count > 0).then(|| ThreadView {
                name: match (text(t, "name"), t.get("id")) {
                    ("", Some(id)) => scalar(id),
                    (name, _) => name.to_string(),
                },
                crashed: t.get("crashed").and_then(Value::as_bool) == Some(true),
                frames,
                frame_count,
            })
        })
    } else {
        None
    };

    let has_in_app = chain
        .iter()
        .chain(grouping::main_thread(event).iter())
        .flat_map(|h| grouping::frames(h))
        .any(grouping::in_app);

    let message = event
        .get("logentry")
        .and_then(Value::as_object)
        .map(|l| match text(l, "formatted") {
            "" => text(l, "message").to_string(),
            f => f.to_string(),
        })
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| text(event, "message").to_string());

    let mut breadcrumbs: Vec<Crumb> = values(event, "breadcrumbs")
        .into_iter()
        .map(|b| {
            let datetime = time(b.get("timestamp"));
            Crumb {
                time: datetime.get(11..19).unwrap_or("").to_string(),
                datetime,
                category: match text(b, "category") {
                    "" => text(b, "type").to_string(),
                    c => c.to_string(),
                },
                level: text(b, "level").to_string(),
                message: cut(text(b, "message"), MAX_CELL),
                data: b
                    .get("data")
                    .filter(|d| d.as_object().is_some_and(|o| !o.is_empty()))
                    .map(scalar)
                    .unwrap_or_default(),
            }
        })
        .collect();
    breadcrumbs.reverse();

    let request = event.get("request").and_then(Value::as_object).map(|r| {
        let pairs = |key: &str| -> Vec<(String, String)> {
            match r.get(key) {
                Some(Value::Object(o)) => rows(o, &[]),
                Some(Value::Array(a)) => a
                    .iter()
                    .filter_map(|p| {
                        let p = p.as_array()?;
                        Some((scalar(p.first()?), scalar(p.get(1)?)))
                    })
                    .collect(),
                _ => Vec::new(),
            }
        };
        let query = match r.get("query_string") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        };
        RequestView {
            method: text(r, "method").to_string(),
            url: text(r, "url").to_string(),
            query,
            headers: pairs("headers"),
            data: match r.get("data") {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(s)) => cut(s, 10_000),
                Some(other) => cut(
                    &serde_json::to_string_pretty(other).unwrap_or_default(),
                    10_000,
                ),
            },
            env: pairs("env"),
        }
    });

    let mut contexts: Vec<ContextView> = Vec::new();
    if let Some(ctx) = event.get("contexts").and_then(Value::as_object) {
        for (key, title_key) in KNOWN_CONTEXTS {
            if let Some(c) = ctx.get(*key).and_then(Value::as_object) {
                contexts.push(ContextView {
                    key: (*key).to_string(),
                    title_key,
                    rows: rows(c, &["type"]),
                });
            }
        }
        let mut others: Vec<(&String, &Value)> = ctx
            .iter()
            .filter(|(k, _)| !KNOWN_CONTEXTS.iter().any(|(known, _)| known == k))
            .collect();
        others.sort_by_key(|(k, _)| k.as_str());
        for (key, c) in others {
            if let Some(c) = c.as_object() {
                contexts.push(ContextView {
                    key: key.clone(),
                    title_key: "",
                    rows: rows(c, &["type"]),
                });
            }
        }
        contexts.retain(|c| !c.rows.is_empty());
    }

    let user = event
        .get("user")
        .and_then(Value::as_object)
        .map(|u| rows(u, &[]))
        .unwrap_or_default();
    let tags = event
        .get("tags")
        .and_then(Value::as_object)
        .map(|t| rows(t, &[]))
        .unwrap_or_default();
    let sdk_map = event.get("sdk").and_then(Value::as_object);
    let sdk = sdk_map
        .map(|s| format!("{} {}", text(s, "name"), text(s, "version")))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let sdk_integrations = sdk_map
        .and_then(|s| s.get("integrations"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let extra = event
        .get("extra")
        .and_then(Value::as_object)
        .map(|e| rows(e, &[]))
        .unwrap_or_default();
    let fingerprint = event
        .get("fingerprint")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(scalar).collect::<Vec<_>>().join(", "))
        .unwrap_or_default();

    EventView {
        message,
        exceptions,
        thread,
        breadcrumbs,
        request,
        contexts,
        user,
        tags,
        sdk,
        sdk_integrations,
        extra,
        fingerprint,
        server_name: text(event, "server_name").to_string(),
        transaction: text(event, "transaction").to_string(),
        dist: text(event, "dist").to_string(),
        platform,
        has_in_app,
        raw: serde_json::to_string_pretty(payload).unwrap_or_default(),
    }
}

/// Why an issue's events were grouped, in words: one line per part of the
/// stored grouping components.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupingView {
    pub hash: String,
    pub config: String,
    /// Translation key of what decided the grouping.
    pub kind_key: &'static str,
    /// The parts, in order (`ValueError`, `views.py · checkout` …).
    pub parts: Vec<String>,
    /// The stored components, pretty-printed.
    pub json: String,
}

fn frame_part(f: &Value) -> String {
    let s = |k: &str| f.get(k).and_then(Value::as_str).unwrap_or("");
    match (s("module"), s("function"), s("image")) {
        ("", "", image) => match s("offset") {
            "" => image.to_string(),
            off => format!("{image} + {off}"),
        },
        ("", f, _) => f.to_string(),
        (m, "", _) => m.to_string(),
        (m, f, _) => format!("{m} · {f}"),
    }
}

fn component_parts(c: &Value, out: &mut Vec<String>) {
    let s = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    match c.get("kind").and_then(Value::as_str).unwrap_or("") {
        "fingerprint" => {
            for v in c
                .get("values")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                out.push(scalar(v));
            }
            if let Some(d) = c.get("default") {
                component_parts(d, out);
            }
        }
        "exception" => {
            for e in c
                .get("values")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let ty = e.get("type").and_then(Value::as_str).unwrap_or("");
                if !ty.is_empty() {
                    out.push(ty.to_string());
                }
                for f in e
                    .get("frames")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    out.push(frame_part(f));
                }
            }
        }
        "exception_value" => {
            for e in c
                .get("values")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let ty = e.get("type").and_then(Value::as_str).unwrap_or("");
                let value = e.get("value").and_then(Value::as_str).unwrap_or("");
                out.push(
                    format!("{ty}: {value}")
                        .trim_matches([' ', ':'])
                        .to_string(),
                );
            }
        }
        "message" => out.push(match s("template") {
            t if !t.is_empty() => t,
            _ => s("message"),
        }),
        "thread" => {
            for f in c
                .get("frames")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                out.push(frame_part(f));
            }
        }
        _ => {}
    }
}

/// The view of one stored grouping hash.
pub fn grouping(hash: &str, config: &str, components: &Value) -> GroupingView {
    let kind_key = match components.get("kind").and_then(Value::as_str).unwrap_or("") {
        "fingerprint" => "grouping.by_fingerprint",
        "exception" => "grouping.by_exception",
        "exception_value" => "grouping.by_exception_value",
        "message" => "grouping.by_message",
        "thread" => "grouping.by_thread",
        _ => "grouping.by_fallback",
    };
    let mut parts = Vec::new();
    component_parts(components, &mut parts);
    parts.retain(|p| !p.is_empty());
    GroupingView {
        hash: hash.to_string(),
        config: config.to_string(),
        kind_key,
        parts,
        json: serde_json::to_string_pretty(components).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn frames(groups: &[FrameGroup]) -> Vec<String> {
        groups
            .iter()
            .map(|g| match g {
                FrameGroup::Frame(f) => f.function.clone(),
                FrameGroup::System(run) => format!("[{} system]", run.len()),
            })
            .collect()
    }

    fn event() -> Value {
        json!({
            "platform": "python",
            "exception": {"values": [
                {"type": "KeyError", "value": "'id'", "stacktrace": {"frames": [
                    {"function": "load", "module": "app", "filename": "app.py", "lineno": 3, "in_app": true}]}},
                {"type": "ValueError", "value": "bad", "mechanism": {"type": "generic", "handled": false},
                 "stacktrace": {"frames": [
                    {"function": "main", "module": "app", "in_app": true},
                    {"function": "dispatch", "module": "flask.app", "in_app": false},
                    {"function": "wrapper", "module": "flask.app", "in_app": false},
                    {"function": "checkout", "module": "shop.views", "filename": "views.py", "lineno": 12,
                     "colno": 4, "in_app": true, "pre_context": ["a", "b"], "context_line": "boom()",
                     "post_context": ["c"], "vars": {"order": 7}},
                    {"function": "get", "module": "dict", "in_app": false}]}}]},
            "breadcrumbs": {"values": [
                {"timestamp": "2026-10-22T12:00:01.500Z", "category": "http", "message": "GET /"},
                {"timestamp": 1792670402.0, "type": "default", "data": {"k": 1}}]},
            "request": {"method": "POST", "url": "https://shop.example/checkout", "query_string": "a=1",
                        "headers": {"Accept": "*/*"}, "data": {"n": 1}},
            "contexts": {"zeta": {"x": 1}, "os": {"type": "os", "name": "Linux", "version": "6.1"},
                         "device": {"model": "iPhone17,3"}},
            "user": {"id": "hash:abc", "email": "hash:def"},
            "tags": {"shop": "north"},
            "sdk": {"name": "sentry.python", "version": "2.73.0", "integrations": ["flask", "logging"]},
        })
    }

    #[test]
    fn the_chain_runs_from_the_main_exception_and_system_frames_fold() {
        let v = build(&event(), false);
        let types: Vec<&str> = v.exceptions.iter().map(|e| e.ty.as_str()).collect();
        assert_eq!(types, ["ValueError", "KeyError"]);
        let main = &v.exceptions[0];
        assert_eq!(main.mechanism, "generic");
        assert_eq!(main.handled, Some(false));
        assert_eq!(main.frame_count, 5);
        assert_eq!(
            frames(&main.frames),
            ["[1 system]", "checkout", "[2 system]", "main"]
        );
        let FrameGroup::Frame(checkout) = &main.frames[1] else {
            panic!()
        };
        assert!(checkout.open && checkout.in_app);
        assert_eq!(checkout.location, "views.py:12:4");
        assert_eq!(
            checkout
                .context
                .iter()
                .map(|l| (l.no.as_str(), l.current))
                .collect::<Vec<_>>(),
            [("10", false), ("11", false), ("12", true), ("13", false)]
        );
        assert_eq!(checkout.vars, [("order".to_string(), "7".to_string())]);
        assert!(v.has_in_app && v.thread.is_none());
        // All frames: nothing folds, the order stays innermost first.
        let all = build(&event(), true);
        assert_eq!(
            frames(&all.exceptions[0].frames),
            ["get", "checkout", "wrapper", "dispatch", "main"]
        );
    }

    #[test]
    fn crumbs_request_contexts_user_tags_and_sdk() {
        let v = build(&event(), false);
        assert_eq!(v.breadcrumbs.len(), 2);
        assert_eq!(v.breadcrumbs[1].time, "12:00:01");
        assert_eq!(v.breadcrumbs[1].message, "GET /");
        assert_eq!(v.breadcrumbs[0].category, "default");
        assert_eq!(v.breadcrumbs[0].data, r#"{"k":1}"#);
        let r = v.request.unwrap();
        assert_eq!((r.method.as_str(), r.query.as_str()), ("POST", "a=1"));
        assert_eq!(r.headers, [("Accept".to_string(), "*/*".to_string())]);
        assert!(r.data.contains("\"n\": 1"));
        let keys: Vec<&str> = v.contexts.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(keys, ["device", "os", "zeta"]);
        assert_eq!(v.contexts[1].title_key, "event.ctx_os");
        assert_eq!(v.contexts[1].rows.len(), 2, "type is left out");
        assert_eq!(v.user.len(), 2);
        assert_eq!(v.tags, [("shop".to_string(), "north".to_string())]);
        assert_eq!(v.sdk, "sentry.python 2.73.0");
        assert_eq!(v.sdk_integrations, "flask, logging");
        assert!(v.raw.contains("\"platform\": \"python\""));
    }

    #[test]
    fn native_frames_without_symbols_show_image_and_offset() {
        let v = build(
            &json!({
                "platform": "cocoa",
                "exception": {"values": [{"type": "EXC_BREAKPOINT", "thread_id": 0}]},
                "threads": {"values": [{"id": 0, "crashed": true, "stacktrace": {"frames": [
                    {"package": "/usr/lib/dyld", "instruction_addr": "0x1000", "image_addr": "0x0"},
                    {"package": "/private/var/App.app/App", "instruction_addr": "0x104c36558",
                     "image_addr": "0x104c34000", "in_app": true}]}}]},
            }),
            false,
        );
        assert_eq!(v.exceptions[0].frame_count, 0);
        let t = v.thread.unwrap();
        assert!(t.crashed);
        assert_eq!(t.name, "0");
        assert_eq!(frames(&t.frames), ["App + 0x2558", "[1 system]"]);
    }

    #[test]
    fn symbolicated_inlined_and_missing_dsym_frames() {
        let v = build(
            &json!({
                "platform": "cocoa",
                "debug_meta": {"images": [
                    {"debug_id": "74B62DF4-771F-3338-8A76-EFFF43E57038", "image_addr": "0x104c34000",
                     "image_size": 4096, "code_file": "/x/App.app/App"}]},
                "exception": {"values": [{"type": "EXC_BREAKPOINT", "stacktrace": {"frames": [
                    {"package": "/x/App.app/App", "instruction_addr": "0x104c34010",
                     "image_addr": "0x104c34000", "in_app": true},
                    {"package": "/x/App.app/App", "instruction_addr": "0x104c36048",
                     "image_addr": "0x104c34000", "in_app": true, "function": "Stage.detonate(CrashKind)",
                     "filename": "CrashSite.swift", "lineno": 43, "data": {"symbolicator_status": "symbolicated"}},
                    {"package": "/x/App.app/App", "instruction_addr": "0x104c36048",
                     "image_addr": "0x104c34000", "in_app": true, "function": "trigger(_: CrashKind, depth: Int)",
                     "filename": "CrashSite.swift", "lineno": 21, "inlined": true,
                     "data": {"symbolicator_status": "symbolicated"}}]}}]},
            }),
            true,
        );
        let frames: Vec<&FrameView> = v.exceptions[0]
            .frames
            .iter()
            .map(|g| match g {
                FrameGroup::Frame(f) => f,
                FrameGroup::System(_) => panic!(),
            })
            .collect();
        assert_eq!(frames[0].location, "CrashSite.swift:21");
        assert!(frames[0].inlined && frames[0].symbolicated && frames[0].missing_dsym.is_empty());
        assert!(!frames[1].inlined && frames[1].symbolicated);
        assert_eq!(frames[2].function, "App + 0x10");
        assert_eq!(
            frames[2].missing_dsym,
            "74b62df4-771f-3338-8a76-efff43e57038"
        );
    }

    #[test]
    fn grouping_reads_as_parts() {
        let g = grouping(
            "0123",
            "kestrel:v1",
            &json!({"kind": "exception", "values": [{"type": "ValueError",
                "frames": [{"module": "app", "function": "main"}, {"image": "App", "offset": "0x10"}]}]}),
        );
        assert_eq!(g.kind_key, "grouping.by_exception");
        assert_eq!(g.parts, ["ValueError", "app · main", "App + 0x10"]);
        let g = grouping(
            "0123",
            "kestrel:v1",
            &json!({"kind": "fingerprint", "values": ["checkout", "{{ default }}"],
                    "default": {"kind": "message", "template": "Payment %s failed"}}),
        );
        assert_eq!(g.parts, ["checkout", "{{ default }}", "Payment %s failed"]);
    }
}
