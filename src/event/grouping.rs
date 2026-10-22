//! Grouping, configuration `kestrel:v2` (docs/design.md §4.4).
//!
//! An event's grouping components are worked out in this order, and the
//! first that applies wins:
//!
//! 1. the SDK's `fingerprint`, with `{{ default }}` standing for what the
//!    rules below give;
//! 2. the exception chain: each exception's type and its frames (in-app
//!    frames only when the event has any), each frame being the normalised
//!    module or file name and function, at most the innermost 50;
//! 3. an exception chain without frames: type plus the value with numbers,
//!    hex, UUIDs and quoted strings replaced;
//! 4. no exception: the `logentry` template (else the normalised message),
//!    and failing that the crashed thread's stack.
//!
//! Native frames without a function name contribute their image's name and
//! the offset into it, and mark the issue unsymbolicated. The components are
//! stored next to the hash so the issue page can explain the grouping.
//! Changing anything here that changes a hash means a new configuration.
//!
//! `kestrel:v2` is `kestrel:v1` plus one rule: frames of the Sentry SDKs
//! themselves (`SentrySDK…`, `SentryCrash…`, `-[Sentry…]`, `@sentry/…`,
//! `sentry_sdk`, the Rust SDK's crates) are never in-app
//! ([`mark_sdk_frames`]), so they can't drive grouping when an app links the
//! SDK statically. Issues made by v1 keep their configuration: every event is
//! grouped by both, and an event whose v2 hash is unknown joins the issue its
//! v1 hash points at (the writer then adds the v2 hash to that issue).

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use symbolic_common::{Language, Name, NameMangling};
use symbolic_demangle::{Demangle, DemangleOptions};

/// The grouping configuration this build applies to new events.
pub const CONFIG: &str = "kestrel:v2";
/// The configuration before it, still worked out so that its issues keep
/// getting their events.
pub const LEGACY_CONFIG: &str = "kestrel:v1";

/// Frames that count per stack (the innermost).
pub const MAX_GROUPING_FRAMES: usize = 50;

/// The result of grouping one event.
#[derive(Debug, Clone)]
pub struct Grouping {
    pub config: &'static str,
    /// 32 hex characters.
    pub hash: String,
    pub components: Value,
    pub unsymbolicated: bool,
}

/// Groups a normalised event with [`CONFIG`] (after [`mark_sdk_frames`]).
pub fn group(event: &Map<String, Value>) -> Grouping {
    group_with(event, CONFIG)
}

/// Groups a normalised event with `config` ([`CONFIG`] or
/// [`LEGACY_CONFIG`]); the configurations differ only in the frames the
/// caller marked in-app, and in the hash.
pub fn group_with(event: &Map<String, Value>, config: &'static str) -> Grouping {
    let images = images(event);
    let (default, default_unsym) = default_components(event, &images);
    let (components, unsymbolicated) = match fingerprint(event) {
        Some(parts) => {
            let uses_default = parts.iter().any(|p| is_default(p));
            let values: Vec<Value> = parts
                .iter()
                .map(|p| {
                    if is_default(p) {
                        Value::from("{{ default }}")
                    } else {
                        Value::from(p.as_str())
                    }
                })
                .collect();
            let mut c = json!({"kind": "fingerprint", "values": values});
            if uses_default {
                c["default"] = default;
            }
            (c, uses_default && default_unsym)
        }
        None => (default, default_unsym),
    };
    Grouping {
        config,
        hash: hash_with(config, &components),
        components,
        unsymbolicated,
    }
}

/// 32 hex characters of `sha256(CONFIG ‖ components)`.
pub fn hash(components: &Value) -> String {
    hash_with(CONFIG, components)
}

/// 32 hex characters of `sha256(config ‖ components)`.
pub fn hash_with(config: &str, components: &Value) -> String {
    let mut h = Sha256::new();
    h.update(config.as_bytes());
    h.update(b"\n");
    h.update(components.to_string().as_bytes());
    hex::encode(&h.finalize()[..16])
}

fn fingerprint(event: &Map<String, Value>) -> Option<Vec<String>> {
    let parts: Vec<String> = event
        .get("fingerprint")?
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    (!parts.is_empty()).then_some(parts)
}

fn is_default(part: &str) -> bool {
    part.split_whitespace().collect::<String>() == "{{default}}"
}

fn default_components(event: &Map<String, Value>, images: &[Image]) -> (Value, bool) {
    // An exception the SDK made up for a message (`captureMessage` with a
    // stack trace) groups by the message.
    let synthetic = values(event, "exception").last().is_some_and(|e| {
        e.get("mechanism")
            .and_then(|m| m.get("synthetic"))
            .and_then(Value::as_bool)
            == Some(true)
    });
    if synthetic && let Some(c) = message_components(event) {
        return (c, false);
    }
    if let Some(c) = exception_components(event, images) {
        return c;
    }
    if let Some(c) = message_components(event) {
        return (c, false);
    }
    if let Some(c) = thread_components(event, images) {
        return c;
    }
    (json!({"kind": "fallback"}), false)
}

fn values<'a>(event: &'a Map<String, Value>, key: &str) -> Vec<&'a Map<String, Value>> {
    event
        .get(key)
        .and_then(|v| v.get("values"))
        .and_then(Value::as_array)
        .map(|v| v.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default()
}

pub fn frames(holder: &Map<String, Value>) -> Vec<&Map<String, Value>> {
    holder
        .get("stacktrace")
        .and_then(|s| s.get("frames"))
        .and_then(Value::as_array)
        .map(|f| f.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default()
}

pub fn in_app(frame: &Map<String, Value>) -> bool {
    frame.get("in_app").and_then(Value::as_bool) == Some(true)
}

/// Every stack trace of an event: the exceptions', the threads' and the
/// legacy top-level one.
fn stacktraces_mut(event: &mut Map<String, Value>) -> Vec<&mut Value> {
    let mut out = Vec::new();
    for (key, value) in event.iter_mut() {
        match key.as_str() {
            "exception" | "threads" => {
                if let Some(values) = value.get_mut("values").and_then(Value::as_array_mut) {
                    for holder in values.iter_mut() {
                        if let Some(st) = holder.get_mut("stacktrace") {
                            out.push(st);
                        }
                    }
                }
            }
            "stacktrace" => out.push(value),
            _ => {}
        }
    }
    out
}

/// `kestrel:v2`: marks the frames of the Sentry SDKs as not in-app, in place.
/// Returns how many it changed.
pub fn mark_sdk_frames(event: &mut Map<String, Value>) -> usize {
    let platform = text(event, "platform").to_string();
    let mut n = 0;
    for st in stacktraces_mut(event) {
        let Some(frames) = st.get_mut("frames").and_then(Value::as_array_mut) else {
            continue;
        };
        for frame in frames.iter_mut().filter_map(Value::as_object_mut) {
            if in_app(frame) && is_sdk_frame(frame, &platform) {
                frame.insert("in_app".into(), Value::Bool(false));
                n += 1;
            }
        }
    }
    n
}

/// A Swift or Objective-C name of Sentry's Cocoa SDK: `-[SentryClient …]`,
/// `SentrySDK.capture(error:)`, `closure #1 in SentryHub…`, the
/// `sentrycrash…` C functions.
fn cocoa_sdk_name(name: &str) -> bool {
    let name = name.trim();
    if name.starts_with("sentrycrash") || name.starts_with("_sentrycrash") {
        return true;
    }
    let name = name
        .strip_prefix("-[")
        .or_else(|| name.strip_prefix("+["))
        .unwrap_or(name);
    let name = SWIFT_PREFIX.replace(name, "");
    let name = name.strip_prefix("static ").unwrap_or(&name);
    let name = name.strip_prefix("Sentry.").unwrap_or(name);
    let Some(rest) = name.strip_prefix("Sentry") else {
        return false;
    };
    // `SentrySDK`, `SentryCrash…`, `Sentry.capture`: not a word that only
    // starts the same way (`Sentryman`).
    rest.chars()
        .next()
        .is_none_or(|c| c.is_ascii_uppercase() || matches!(c, '.' | ' ' | '(' | ']' | '_'))
}

/// Whether a frame belongs to one of the Sentry SDKs (`kestrel:v2`): Cocoa
/// (`Sentry…`, `SentryCrash…`, `-[Sentry…]`, `Sentry.framework`),
/// JavaScript (`@sentry/…` packages), Python (`sentry_sdk`) and Rust or
/// sentry-native (the `sentry` crates, `sentry_…` functions).
pub fn is_sdk_frame(frame: &Map<String, Value>, event_platform: &str) -> bool {
    let platform = match text(frame, "platform") {
        "" => event_platform,
        p => p,
    };
    let function = text(frame, "function");
    let module = text(frame, "module");
    let package = text(frame, "package");
    let paths = [text(frame, "abs_path"), text(frame, "filename")];
    // JavaScript: the SDK's packages, bundled or from node_modules.
    if module.starts_with("@sentry/")
        || module.starts_with("@sentry.")
        || module.starts_with("@sentry-internal")
        || paths
            .iter()
            .any(|p| p.contains("/@sentry/") || p.contains("/@sentry-internal/"))
    {
        return true;
    }
    // Python.
    if module == "sentry_sdk"
        || module.starts_with("sentry_sdk.")
        || paths
            .iter()
            .any(|p| p.contains("/sentry_sdk/") || p.contains("\\sentry_sdk\\"))
    {
        return true;
    }
    match platform {
        "cocoa" | "native" | "objc" | "swift" | "c" => {
            package.contains("/Sentry.framework/")
                || basename(package) == "Sentry"
                || [function, text(frame, "raw_function"), text(frame, "symbol")]
                    .iter()
                    .any(|n| cocoa_sdk_name(n))
                || (platform == "native" && rust_sdk_frame(function, package))
        }
        "rust" => rust_sdk_frame(function, package),
        _ => false,
    }
}

/// The Rust SDK's crates (`sentry`, `sentry_core`, `sentry_panic` …) and
/// sentry-native's `sentry_…` functions.
fn rust_sdk_frame(function: &str, package: &str) -> bool {
    let sdk_crate = |s: &str| s == "sentry" || s.starts_with("sentry_") || s.starts_with("sentry-");
    sdk_crate(package)
        || function.starts_with("sentry::")
        || function.starts_with("sentry_")
        || function.starts_with("<sentry::")
        || function.starts_with("<sentry_")
}

fn text<'a>(map: &'a Map<String, Value>, key: &str) -> &'a str {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
}

fn exception_components(event: &Map<String, Value>, images: &[Image]) -> Option<(Value, bool)> {
    let chain = values(event, "exception");
    if chain.is_empty() {
        return None;
    }
    let any_frames = chain.iter().any(|e| !frames(e).is_empty());
    if any_frames {
        let in_app_only = chain.iter().flat_map(|e| frames(e)).any(in_app);
        let mut unsym = false;
        let values: Vec<Value> = chain
            .iter()
            .map(|e| {
                json!({
                    "type": text(e, "type"),
                    "frames": frame_components(&frames(e), in_app_only, images, &mut unsym),
                })
            })
            .collect();
        Some((json!({"kind": "exception", "values": values}), unsym))
    } else {
        let values: Vec<Value> = chain
            .iter()
            .map(|e| json!({"type": text(e, "type"), "value": normalize_value(text(e, "value"))}))
            .collect();
        Some((json!({"kind": "exception_value", "values": values}), false))
    }
}

fn message_components(event: &Map<String, Value>) -> Option<Value> {
    let entry = event.get("logentry")?.as_object()?;
    let template = text(entry, "message");
    if !template.is_empty() {
        return Some(json!({"kind": "message", "template": template}));
    }
    let formatted = text(entry, "formatted");
    (!formatted.is_empty())
        .then(|| json!({"kind": "message", "message": normalize_value(formatted)}))
}

/// The crashed thread, else the current one, else the only one.
pub fn main_thread(event: &Map<String, Value>) -> Option<&Map<String, Value>> {
    let threads = values(event, "threads");
    let flag =
        |t: &&Map<String, Value>, key: &str| t.get(key).and_then(Value::as_bool) == Some(true);
    threads
        .iter()
        .find(|t| flag(t, "crashed"))
        .or_else(|| threads.iter().find(|t| flag(t, "current")))
        .or_else(|| (threads.len() == 1).then(|| &threads[0]))
        .copied()
}

fn thread_components(event: &Map<String, Value>, images: &[Image]) -> Option<(Value, bool)> {
    let thread = main_thread(event)?;
    let frames = frames(thread);
    if frames.is_empty() {
        return None;
    }
    let in_app_only = frames.iter().copied().any(in_app);
    let mut unsym = false;
    let parts = frame_components(&frames, in_app_only, images, &mut unsym);
    Some((json!({"kind": "thread", "frames": parts}), unsym))
}

fn frame_components(
    frames: &[&Map<String, Value>],
    in_app_only: bool,
    images: &[Image],
    unsym: &mut bool,
) -> Vec<Value> {
    let mut parts: Vec<Value> = frames
        .iter()
        .filter(|f| !in_app_only || in_app(f))
        .filter_map(|f| frame_component(f, images, unsym))
        .collect();
    if parts.len() > MAX_GROUPING_FRAMES {
        parts.drain(..parts.len() - MAX_GROUPING_FRAMES);
    }
    parts
}

/// One frame's contribution: `{module, function}`, or `{image, offset}` for
/// a native frame without symbols.
pub fn frame_component(
    frame: &Map<String, Value>,
    images: &[Image],
    unsym: &mut bool,
) -> Option<Value> {
    let function = text(frame, "function");
    let symbolless = matches!(function, "" | "?" | "<redacted>" | "<unknown>");
    if symbolless && let Some(addr) = frame.get("instruction_addr").and_then(address) {
        let image = images.iter().find(|i| addr >= i.start && addr < i.end);
        let name = match text(frame, "package") {
            "" => image.map(|i| i.name.clone()).unwrap_or_default(),
            package => basename(package).to_string(),
        };
        let base = frame
            .get("image_addr")
            .and_then(address)
            .filter(|a| *a > 0)
            .or(image.map(|i| i.start));
        *unsym = true;
        let mut part = json!({"image": name});
        if let Some(base) = base
            && addr >= base
        {
            part["offset"] = Value::from(format!("{:#x}", addr - base));
        }
        return Some(part);
    }
    let module = match text(frame, "module") {
        "" => {
            let file = match text(frame, "filename") {
                "" => text(frame, "abs_path"),
                f => f,
            };
            normalize_file(file)
        }
        m => normalize_file(m),
    };
    let function = if symbolless {
        String::new()
    } else {
        normalize_function(function)
    };
    if module.is_empty() && function.is_empty() {
        return None;
    }
    let mut part = Map::new();
    if !module.is_empty() {
        part.insert("module".into(), Value::from(module));
    }
    if !function.is_empty() {
        part.insert("function".into(), Value::from(function));
    }
    Some(Value::Object(part))
}

/// A loaded image from `debug_meta`.
#[derive(Debug, Clone)]
pub struct Image {
    pub start: u64,
    pub end: u64,
    pub name: String,
}

fn images(event: &Map<String, Value>) -> Vec<Image> {
    event
        .get("debug_meta")
        .and_then(|d| d.get("images"))
        .and_then(Value::as_array)
        .map(|images| {
            images
                .iter()
                .filter_map(Value::as_object)
                .filter_map(|i| {
                    let start = i.get("image_addr").and_then(address)?;
                    let size = i.get("image_size").and_then(Value::as_u64).unwrap_or(0);
                    let name = basename(text(i, "code_file")).to_string();
                    Some(Image {
                        start,
                        end: start.saturating_add(size.max(1)),
                        name,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// An address from `"0x…"` or a number.
pub fn address(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => {
            let s = s.trim();
            match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                Some(hex) => u64::from_str_radix(hex, 16).ok(),
                None => s.parse().ok(),
            }
        }
        _ => None,
    }
}

/// The last path segment, without query or fragment.
pub fn basename(path: &str) -> &str {
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let path = path.trim_end_matches(['/', '\\']);
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

static HASH_PART: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[.\-_][0-9a-fA-F]{8,}\b").expect("valid regex"));

/// A module or file name as it counts for grouping: the base name with
/// build hashes (`main.3f2a1b9c.js`) removed.
pub fn normalize_file(raw: &str) -> String {
    let name = basename(raw);
    HASH_PART.replace_all(name, "").into_owned()
}

/// A Swift-mangled name demangled; anything else as it is.
pub fn demangle(name: &str) -> String {
    let swift = ["$s", "_$s", "$S", "_$S", "$e", "_$e", "_T0"]
        .iter()
        .any(|p| name.starts_with(p));
    if !swift {
        return name.to_string();
    }
    let n = Name::new(name, NameMangling::Mangled, Language::Swift);
    n.demangle(DemangleOptions::name_only())
        .unwrap_or_else(|| name.to_string())
}

static RUST_HASH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"::h[0-9a-f]{16}$").expect("valid regex"));
static ADDRESS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*(\+\s*\d+|0x[0-9a-fA-F]+)").expect("valid regex"));
static SWIFT_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:specialized |partial apply for |merged |@objc |async |(?:implicit )?closure #\d+ in |thunk for )+")
        .expect("valid regex")
});
static TRAIT_IMPL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^<([^<>]+?)(?: as [^<>]+)?>::").expect("valid regex"));
/// Rust v0 crate disambiguators (`core[ed37fd7903368ee]`), closures and shims.
static RUST_NOISE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[[0-9a-f]{8,}\]|::\{(?:closure|shim)[^}]*\}|\{\{closure\}\}")
        .expect("valid regex")
});

/// A function name as it counts for grouping: demangled, without
/// addresses, generic arguments, closure markers and hash suffixes.
pub fn normalize_function(raw: &str) -> String {
    let mut f = demangle(raw.trim());
    f = RUST_HASH.replace(&f, "").into_owned();
    f = f.replace("::{{closure}}", "");
    f = RUST_NOISE.replace_all(&f, "").into_owned();
    f = TRAIT_IMPL.replace(&f, "${1}::").into_owned();
    f = SWIFT_PREFIX.replace(&f, "").into_owned();
    f = ADDRESS.replace_all(&f, "").into_owned();
    f = strip_generics(&f);
    f.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Removes `<…>` groups unless they are a single lowercase word like
/// `<anonymous>`, `<lambda>` or `<module>`.
fn strip_generics(f: &str) -> String {
    let mut out = String::with_capacity(f.len());
    let mut depth = 0usize;
    let mut group = String::new();
    for c in f.chars() {
        match c {
            '<' => {
                if depth > 0 {
                    group.push(c);
                }
                depth += 1;
            }
            '>' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    if !group.is_empty() && group.chars().all(|c| c.is_ascii_lowercase()) {
                        out.push('<');
                        out.push_str(&group);
                        out.push('>');
                    }
                    group.clear();
                } else {
                    group.push(c);
                }
            }
            _ if depth > 0 => group.push(c),
            _ => out.push(c),
        }
    }
    if depth > 0 {
        // Unbalanced: keep what was there.
        out.push('<');
        out.push_str(&group);
    }
    out
}

static QUOTED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"'[^']*'|"[^"]*"|“[^”]*”"#).expect("valid regex"));
static UUID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b")
        .expect("valid regex")
});
static HEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b0[xX][0-9a-fA-F]+\b|\b[0-9a-fA-F]*(?:[0-9][a-fA-F]|[a-fA-F][0-9])[0-9a-fA-F]*\b")
        .expect("valid regex")
});
static NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d+(?:\.\d+)?\b").expect("valid regex"));

/// An exception value or message with what varies between occurrences
/// replaced: quoted strings, UUIDs, hex and numbers.
pub fn normalize_value(raw: &str) -> String {
    let v: String = raw.chars().take(1000).collect();
    let v = QUOTED.replace_all(&v, "<str>");
    let v = UUID.replace_all(&v, "<uuid>");
    let v = HEX.replace_all(&v, |c: &regex::Captures<'_>| {
        // A hex-looking word needs eight characters, or the 0x prefix.
        if c[0].len() >= 8 || c[0].starts_with("0x") || c[0].starts_with("0X") {
            "<hex>".to_string()
        } else {
            c[0].to_string()
        }
    });
    NUMBER.replace_all(&v, "<num>").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group_of(v: Value) -> Grouping {
        group(v.as_object().unwrap())
    }

    #[test]
    fn values_lose_what_varies() {
        assert_eq!(
            normalize_value("handled(code: 42) (Code: 0)"),
            "handled(code: <num>) (Code: <num>)"
        );
        assert_eq!(
            normalize_value(
                "user 'ada' not found in 3f1c2b4a-0000-4000-8000-1234567890ab at 0x7ffee1"
            ),
            "user <str> not found in <uuid> at <hex>"
        );
        assert_eq!(
            normalize_value("hash deadbeef42 failed"),
            "hash <hex> failed"
        );
        assert_eq!(normalize_value("took 1.5 s"), "took <num> s");
        assert_eq!(normalize_value("bad state"), "bad state");
    }

    #[test]
    fn function_names_lose_noise() {
        assert_eq!(
            normalize_function("myapp::handler::h0123456789abcdef"),
            "myapp::handler"
        );
        assert_eq!(normalize_function("myapp::run::{{closure}}"), "myapp::run");
        assert_eq!(
            normalize_function("kk_fixtures::main::{closure#4}"),
            "kk_fixtures::main"
        );
        assert_eq!(
            normalize_function("core[ed37fd7903368ee]::panicking::panic_fmt"),
            "core::panicking::panic_fmt"
        );
        assert_eq!(
            normalize_function("<sentry_panic::PanicIntegration>::event_from_panic_info"),
            "sentry_panic::PanicIntegration::event_from_panic_info"
        );
        assert_eq!(
            normalize_function("<myapp::Foo as core::fmt::Display>::fmt"),
            "myapp::Foo::fmt"
        );
        assert_eq!(normalize_function("Vec<String>::push"), "Vec::push");
        assert_eq!(
            normalize_function("Object.<anonymous>"),
            "Object.<anonymous>"
        );
        assert_eq!(normalize_function("<module>"), "<module>");
        assert_eq!(
            normalize_function("closure #1 in CrashSite.crash(_:)"),
            "CrashSite.crash(_:)"
        );
        assert_eq!(
            normalize_function("objc_exception_throw + 120"),
            "objc_exception_throw"
        );
    }

    #[test]
    fn swift_names_are_demangled() {
        let d = demangle("$s8mangling9r137577441xySaySiG_tF");
        assert!(d.contains("r13757744"), "{d}");
        assert!(!d.starts_with('$'), "{d}");
        assert_eq!(demangle("plain_c_function"), "plain_c_function");
    }

    #[test]
    fn files_lose_build_hashes() {
        assert_eq!(
            normalize_file("https://x.example/static/js/main.3f2a1b9c.js?v=1"),
            "main.js"
        );
        assert_eq!(normalize_file("app:///chunk-a1b2c3d4e5.js"), "chunk.js");
        assert_eq!(normalize_file("app.views"), "app.views");
        assert_eq!(normalize_file("C:\\src\\main.rs"), "main.rs");
    }

    #[test]
    fn the_rules_apply_in_order() {
        let exc = json!({"exception": {"values": [{"type": "ValueError", "value": "bad 1", "stacktrace": {"frames": [
            {"module": "lib", "function": "x", "in_app": false},
            {"module": "app.views", "function": "index", "in_app": true}]}}]}});
        let g = group_of(exc.clone());
        assert_eq!(g.components["kind"], "exception");
        assert_eq!(
            g.components["values"][0]["frames"],
            json!([{"module": "app.views", "function": "index"}])
        );
        // The value doesn't matter when there are frames.
        let mut other = exc.clone();
        other["exception"]["values"][0]["value"] = json!("bad 2 entirely different");
        assert_eq!(group_of(other).hash, g.hash);

        let mut fp = exc.clone();
        fp["fingerprint"] = json!(["db", "{{ default }}"]);
        let gfp = group_of(fp);
        assert_eq!(gfp.components["kind"], "fingerprint");
        assert_eq!(gfp.components["default"], g.components);
        assert_ne!(gfp.hash, g.hash);
        let mut fp2 = exc;
        fp2["fingerprint"] = json!(["db"]);
        assert!(group_of(fp2).components.get("default").is_none());

        let no_stack = group_of(json!({"exception": {"values": [{"type": "E", "value": "id 7"}]}}));
        assert_eq!(no_stack.components["values"][0]["value"], "id <num>");
        assert_eq!(
            no_stack.hash,
            group_of(json!({"exception": {"values": [{"type": "E", "value": "id 8"}]}})).hash
        );

        let msg = group_of(
            json!({"logentry": {"message": "Order %s failed", "formatted": "Order 1 failed"}}),
        );
        assert_eq!(
            msg.components,
            json!({"kind": "message", "template": "Order %s failed"})
        );

        let thread = group_of(json!({"threads": {"values": [
            {"crashed": false, "stacktrace": {"frames": [{"function": "idle"}]}},
            {"crashed": true, "stacktrace": {"frames": [{"function": "boom", "module": "m"}]}}]}}));
        assert_eq!(
            thread.components["frames"],
            json!([{"module": "m", "function": "boom"}])
        );

        assert_eq!(group_of(json!({})).components, json!({"kind": "fallback"}));
    }

    #[test]
    fn native_frames_without_symbols_use_image_and_offset() {
        let event = json!({
            "debug_meta": {"images": [{"code_file": "/x/My.app/My", "image_addr": "0x1000", "image_size": 4096}]},
            "exception": {"values": [{"type": "EXC_BREAKPOINT", "stacktrace": {"frames": [
                {"instruction_addr": "0x1234", "in_app": true},
                {"instruction_addr": "0x9999", "image_addr": "0x9000", "package": "/usr/lib/libz.dylib", "in_app": false}]}}]},
        });
        let g = group_of(event);
        assert!(g.unsymbolicated);
        assert_eq!(
            g.components["values"][0]["frames"],
            json!([{"image": "My", "offset": "0x234"}])
        );
    }

    #[test]
    fn sdk_frames_are_never_in_app() {
        let frame = |v: Value| v.as_object().unwrap().clone();
        for (f, platform) in [
            (
                json!({"function": "-[SentryClient captureError:withScope:]"}),
                "cocoa",
            ),
            (json!({"function": "+[SentrySDK captureError:]"}), "cocoa"),
            (json!({"function": "SentrySDK.capture(error:)"}), "cocoa"),
            (
                json!({"function": "static SentrySDK.capture(error:)"}),
                "cocoa",
            ),
            (
                json!({"function": "closure #1 in SentryHub.capture(event:)"}),
                "cocoa",
            ),
            (
                json!({"function": "sentrycrashcm_handleException"}),
                "cocoa",
            ),
            (
                json!({"function": "SentryCrashMonitor_NSException.handleException"}),
                "native",
            ),
            (
                json!({"function": "x", "package": "/private/var/App.app/Frameworks/Sentry.framework/Sentry"}),
                "cocoa",
            ),
            (
                json!({"function": "captureException", "module": "@sentry.core.build.cjs:exports", "filename": "/app/node_modules/@sentry/core/build/cjs/exports.js"}),
                "node",
            ),
            (
                json!({"function": "captureException", "filename": "app:///node_modules/@sentry/browser/esm/sdk.js"}),
                "javascript",
            ),
            (
                json!({"function": "capture_event", "module": "sentry_sdk.client"}),
                "python",
            ),
            (
                json!({"function": "x", "abs_path": "/srv/venv/lib/python3.13/site-packages/sentry_sdk/hub.py"}),
                "python",
            ),
            (
                json!({"function": "sentry_panic::panic_handler", "package": "sentry_panic"}),
                "rust",
            ),
            (
                json!({"function": "<sentry_core::hub::Hub>::capture_event"}),
                "native",
            ),
            (json!({"function": "sentry_capture_event"}), "native"),
        ] {
            assert!(is_sdk_frame(&frame(f.clone()), platform), "{f}");
        }
        for (f, platform) in [
            (json!({"function": "CrashSite.crash(_:)"}), "cocoa"),
            (json!({"function": "Sentryman.watch()"}), "cocoa"),
            (
                json!({"function": "-[AppDelegate sentryDidStart]"}),
                "cocoa",
            ),
            (
                json!({"function": "checkout", "module": "shop.views"}),
                "python",
            ),
            (
                json!({"function": "sentry_hook", "module": "app.sentry_setup"}),
                "python",
            ),
            (
                json!({"function": "main", "filename": "/app/src/sentry.js"}),
                "node",
            ),
            (json!({"function": "kk_fixtures::main"}), "rust"),
        ] {
            assert!(!is_sdk_frame(&frame(f.clone()), platform), "{f}");
        }
    }

    #[test]
    fn v2_groups_past_statically_linked_sdk_frames() {
        // A handled error whose stack starts in the app and ends in a
        // statically linked Sentry (all frames in the app's binary).
        let mut event = json!({"platform": "cocoa", "exception": {"values": [{"type": "AppError", "stacktrace": {"frames": [
            {"function": "AppDelegate.application(_:didFinishLaunchingWithOptions:)", "in_app": true},
            {"function": "Checkout.pay()", "in_app": true},
            {"function": "SentrySDK.capture(error:)", "in_app": true},
            {"function": "-[SentryClient captureError:withScope:]", "in_app": true}]}}]}});
        let map = event.as_object_mut().unwrap();
        let v1 = group_with(map, LEGACY_CONFIG);
        assert_eq!(mark_sdk_frames(map), 2);
        let v2 = group(map);
        assert_eq!(v1.config, "kestrel:v1");
        assert_eq!(v2.config, "kestrel:v2");
        let names = |g: &Grouping| g.components["values"][0]["frames"].to_string();
        assert!(names(&v1).contains("SentryClient"), "{}", names(&v1));
        assert!(!names(&v2).contains("Sentry"), "{}", names(&v2));
        assert!(names(&v2).contains("Checkout.pay()"));
        // The same components hash differently per configuration.
        assert_ne!(
            hash_with(LEGACY_CONFIG, &v2.components),
            hash_with(CONFIG, &v2.components)
        );
    }
}
