//! Privacy scrubbing (docs/design.md §4.3): values under sensitive keys,
//! credentials in headers and free text, card numbers (Luhn), query
//! strings, IP addresses, and the keyed hash that replaces user details.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// What a scrubbed value becomes.
pub const FILTERED: &str = "[Filtered]";

/// Keys whose values are never kept (extra data, contexts, breadcrumbs,
/// request data, user data, tags and frame variables): the design's list,
/// plus `cookie` for cookies kept under other names (`HTTP_COOKIE`).
static SENSITIVE_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)pass|secret|token|auth|key|card|cookie").expect("valid regex")
});

/// `name=value` in free text (query strings, log lines) with a sensitive name.
static SENSITIVE_PAIR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\b([\w.\-]*(?:pass|secret|token|auth|key|card|cookie)[\w.\-]*)=([^&\s'",;]+)"#,
    )
    .expect("valid regex")
});

/// 13 to 19 digits, optionally grouped by single spaces or dashes.
static CARD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(?:\d[ -]?){12,18}\d\b").expect("valid regex"));

/// `Bearer …` / `Basic …` credentials anywhere in text.
static CREDENTIAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(bearer|basic)\s+[A-Za-z0-9\-._~+/]+=*").expect("valid regex")
});

/// Headers whose value is replaced (credentials).
const SECRET_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "x-sentry-auth",
];

/// Keys and headers that carry an IP address; they are removed everywhere.
const IP_KEYS: &[&str] = &[
    "ip_address",
    "ip",
    "client_ip",
    "remote_addr",
    "x-forwarded-for",
    "x-real-ip",
    "forwarded",
    "cf-connecting-ip",
    "true-client-ip",
    "x-client-ip",
    "x-cluster-client-ip",
    "fastly-client-ip",
    // The same headers in a WSGI/CGI environment.
    "http_x_forwarded_for",
    "http_x_real_ip",
    "http_forwarded",
    "http_cf_connecting_ip",
    "http_true_client_ip",
    "http_x_client_ip",
];

/// Keys that carry a user agent; removed like IP addresses (the browser and
/// OS contexts keep what is useful).
const USER_AGENT_KEYS: &[&str] = &["user-agent", "user_agent", "http_user_agent"];

pub fn is_sensitive_key(key: &str) -> bool {
    SENSITIVE_KEY.is_match(key)
}

pub fn is_ip_key(key: &str) -> bool {
    IP_KEYS.iter().any(|k| k.eq_ignore_ascii_case(key))
}

pub fn is_user_agent_key(key: &str) -> bool {
    USER_AGENT_KEYS.iter().any(|k| k.eq_ignore_ascii_case(key))
}

pub fn is_secret_header(name: &str) -> bool {
    SECRET_HEADERS.iter().any(|h| h.eq_ignore_ascii_case(name)) || is_sensitive_key(name)
}

/// The Luhn checksum of a digit string.
pub fn luhn(digits: &str) -> bool {
    let mut sum = 0;
    for (i, b) in digits.bytes().rev().enumerate() {
        let mut d = u32::from(b - b'0');
        if i % 2 == 1 {
            d *= 2;
            if d > 9 {
                d -= 9;
            }
        }
        sum += d;
    }
    sum % 10 == 0
}

/// `text` with card numbers, `Bearer`/`Basic` credentials and sensitive
/// `name=value` pairs replaced, or `None` when nothing matched.
pub fn scrub_text(text: &str) -> Option<String> {
    let mut changed = false;
    let mut out = std::borrow::Cow::Borrowed(text);
    if CARD.is_match(&out) {
        let replaced = CARD.replace_all(&out, |caps: &regex::Captures<'_>| {
            let digits: String = caps[0].chars().filter(char::is_ascii_digit).collect();
            if luhn(&digits) {
                changed = true;
                FILTERED.to_string()
            } else {
                caps[0].to_string()
            }
        });
        out = std::borrow::Cow::Owned(replaced.into_owned());
    }
    if CREDENTIAL.is_match(&out) {
        changed = true;
        out = std::borrow::Cow::Owned(CREDENTIAL.replace_all(&out, "$1 [Filtered]").into_owned());
    }
    if SENSITIVE_PAIR.is_match(&out) {
        let replaced = SENSITIVE_PAIR.replace_all(&out, |caps: &regex::Captures<'_>| {
            if &caps[2] == FILTERED {
                caps[0].to_string()
            } else {
                changed = true;
                format!("{}={FILTERED}", &caps[1])
            }
        });
        out = std::borrow::Cow::Owned(replaced.into_owned());
    }
    changed.then(|| out.into_owned())
}

/// Whether a JSON number looks like a card number.
fn card_number(n: &serde_json::Number) -> bool {
    n.as_u64().is_some_and(|v| {
        let digits = v.to_string();
        (13..=19).contains(&digits.len()) && luhn(&digits)
    })
}

/// Scrubs free text in a value tree: card numbers and credentials in strings,
/// card numbers in integers (`numbers`), recursively.
pub fn scrub_strings(value: &mut Value, numbers: bool) {
    match value {
        Value::String(s) => {
            if let Some(clean) = scrub_text(s) {
                *s = clean;
            }
        }
        Value::Number(n) if numbers && card_number(n) => *value = Value::from(FILTERED),
        Value::Array(items) => items.iter_mut().for_each(|v| scrub_strings(v, numbers)),
        Value::Object(map) => map.values_mut().for_each(|v| scrub_strings(v, numbers)),
        _ => {}
    }
}

/// Replaces values under sensitive keys, removes IP keys, and scrubs query
/// strings in URL-ish values, recursively.
pub fn scrub_keys(value: &mut Value) {
    match value {
        Value::Object(map) => scrub_map(map),
        Value::Array(items) => items.iter_mut().for_each(scrub_keys),
        _ => {}
    }
}

fn scrub_map(map: &mut Map<String, Value>) {
    map.retain(|k, _| !is_ip_key(k) && !is_user_agent_key(k));
    for (key, value) in map.iter_mut() {
        if is_sensitive_key(key) {
            *value = Value::from(FILTERED);
        } else if let Value::String(s) = value {
            if looks_like_url(s)
                && let Some(clean) = scrub_query_in_url(s)
            {
                *s = clean;
            }
        } else {
            scrub_keys(value);
        }
    }
}

fn looks_like_url(s: &str) -> bool {
    s.contains('?') && (s.contains("://") || s.starts_with('/'))
}

/// A query string (`a=1&token=x`) with sensitive parameters filtered, or
/// `None` when nothing changed.
pub fn scrub_query(query: &str) -> Option<String> {
    let mut changed = false;
    let parts: Vec<String> = query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((k, _)) if is_sensitive_key(k) || is_ip_key(k) => {
                changed = true;
                format!("{k}={FILTERED}")
            }
            _ => pair.to_string(),
        })
        .collect();
    changed.then(|| parts.join("&"))
}

/// A URL with sensitive query parameters filtered, or `None`.
pub fn scrub_query_in_url(url: &str) -> Option<String> {
    let (base, rest) = url.split_once('?')?;
    let (query, fragment) = match rest.split_once('#') {
        Some((q, f)) => (q, Some(f)),
        None => (rest, None),
    };
    let query = scrub_query(query)?;
    Some(match fragment {
        Some(f) => format!("{base}?{query}#{f}"),
        None => format!("{base}?{query}"),
    })
}

/// A keyed, truncated hash: 16 hex characters of
/// `sha256(key ‖ project ‖ scope ‖ value)`. Stable for one instance and
/// project, useless without the instance's key.
pub fn keyed_hash(key: &[u8; 32], project_id: i64, scope: &str, value: &str) -> String {
    let mut h = Sha256::new();
    h.update(key);
    h.update(project_id.to_le_bytes());
    h.update(scope.as_bytes());
    h.update([0]);
    h.update(value.as_bytes());
    let digest = h.finalize();
    hex::encode(&digest[..8])
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn luhn_numbers() {
        assert!(luhn("4111111111111111"));
        assert!(luhn("5500005555555559"));
        assert!(!luhn("4111111111111112"));
    }

    #[test]
    fn card_numbers_and_credentials_in_text() {
        assert_eq!(
            scrub_text("paid with 4111 1111 1111 1111 today").as_deref(),
            Some("paid with [Filtered] today")
        );
        assert_eq!(scrub_text("order 1234567890123 shipped"), None, "not Luhn");
        assert_eq!(
            scrub_text("header was Bearer abc.def-123=").as_deref(),
            Some("header was Bearer [Filtered]")
        );
        assert_eq!(scrub_text("0x0000000180044955"), None);
        assert_eq!(
            scrub_text("'step=pay&token=abc&api_key=k1'").as_deref(),
            Some("'step=pay&token=[Filtered]&api_key=[Filtered]'")
        );
        assert_eq!(scrub_text("token=[Filtered]"), None);
        assert_eq!(scrub_text("plain words"), None);
    }

    #[test]
    fn sensitive_keys_and_ips() {
        let mut v = json!({
            "password": "hunter2",
            "nested": {"api_key": 5, "ok": "fine", "ip_address": "10.1.2.3"},
            "list": [{"Secret": {"a": 1}}],
            "url": "https://example.com/cb?code=1&access_token=abc#top",
            "REMOTE_ADDR": "10.1.2.3",
        });
        scrub_keys(&mut v);
        assert_eq!(
            v,
            json!({
                "password": FILTERED,
                "nested": {"api_key": FILTERED, "ok": "fine"},
                "list": [{"Secret": FILTERED}],
                "url": "https://example.com/cb?code=1&access_token=[Filtered]#top",
            })
        );
    }

    #[test]
    fn numbers_that_look_like_cards() {
        let mut v = json!({"cc": 4111111111111111u64, "count": 42, "ms": 1792612977123u64});
        scrub_strings(&mut v, true);
        assert_eq!(v["cc"], FILTERED);
        assert_eq!(v["count"], 42);
    }

    #[test]
    fn keyed_hashes_depend_on_key_project_and_scope() {
        let k = [7u8; 32];
        let a = keyed_hash(&k, 1, "email", "ada@example.com");
        assert_eq!(a.len(), 16);
        assert_eq!(a, keyed_hash(&k, 1, "email", "ada@example.com"));
        assert_ne!(a, keyed_hash(&k, 2, "email", "ada@example.com"));
        assert_ne!(a, keyed_hash(&k, 1, "username", "ada@example.com"));
        assert_ne!(a, keyed_hash(&[8u8; 32], 1, "email", "ada@example.com"));
    }
}
