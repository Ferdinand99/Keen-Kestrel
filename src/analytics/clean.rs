//! What a web event keeps of what the page sent (docs/design.md §5.1–5.2):
//! the host and path without the query (the `utm_*` parameters apart), the
//! referrer reduced to its domain (none for self-referrals), a screen-width
//! bucket, and custom event names and properties with anything that looks
//! like an email address, a phone number or a card number replaced.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};

use crate::event::scrub;

/// What replaces personal data.
pub const FILTERED: &str = "[Filtered]";
/// Longest path kept (characters).
pub const MAX_PATH: usize = 512;
/// Longest UTM value, event name or property value kept (characters).
pub const MAX_VALUE: usize = 200;
/// Longest custom event name (characters).
pub const MAX_NAME: usize = 100;
/// Properties kept per custom event.
pub const MAX_PROPS: usize = 20;

static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[a-z0-9._%+\-]+@[a-z0-9\-]+(?:\.[a-z0-9\-]+)*\.[a-z]{2,}")
        .expect("valid regex")
});

/// IP addresses (v4, and v6 written out with at least four groups).
static IP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:(?:\d{1,3}\.){3}\d{1,3}|(?:[0-9a-f]{1,4}:){3,7}[0-9a-f]{1,4})\b")
        .expect("valid regex")
});

/// Phone-number candidates: an optional `+` or `00`, then digits with the
/// separators people write them with. [`looks_like_phone`] decides.
static PHONE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:\+|\(\d|\b\d)[0-9 ().\-]{6,22}\d").expect("valid regex"));

/// A phone number: 9–15 digits that start with `+` (or `00`), or that are
/// written in at least three groups. Plain digit runs (order numbers, ids)
/// and dates aren't phone numbers here; long digit runs that pass the Luhn
/// check are card numbers ([`scrub::scrub_text`]).
fn looks_like_phone(candidate: &str) -> bool {
    let digits = candidate.chars().filter(char::is_ascii_digit).count();
    if !(9..=15).contains(&digits) {
        return false;
    }
    let international = candidate.starts_with('+') || candidate.starts_with("00");
    let groups = candidate
        .split(|c: char| !c.is_ascii_digit())
        .filter(|g| !g.is_empty())
        .count();
    international || groups >= 3
}

/// `text` with email addresses, IP addresses, phone numbers, card numbers
/// and credentials replaced by [`FILTERED`].
pub fn scrub(text: &str) -> String {
    let mut out = scrub::scrub_text(text).unwrap_or_else(|| text.to_string());
    if EMAIL.is_match(&out) {
        out = EMAIL.replace_all(&out, FILTERED).into_owned();
    }
    if IP.is_match(&out) {
        out = IP.replace_all(&out, FILTERED).into_owned();
    }
    if PHONE.is_match(&out) {
        out = PHONE
            .replace_all(&out, |caps: &regex::Captures<'_>| {
                if looks_like_phone(&caps[0]) {
                    FILTERED.to_string()
                } else {
                    caps[0].to_string()
                }
            })
            .into_owned();
    }
    out
}

/// At most `max` characters of `text`, trimmed, without control characters.
pub fn short(text: &str, max: usize) -> String {
    text.trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect()
}

/// The page as the dashboards see it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Page {
    /// Lowercase, without the port (`www.` stays).
    pub hostname: String,
    /// Percent-decoded, without the query; with the fragment only in hash
    /// mode. Scrubbed.
    pub path: String,
    pub utm_source: String,
    pub utm_medium: String,
    pub utm_campaign: String,
    pub utm_term: String,
    pub utm_content: String,
}

/// Reads the page's URL (`http` or `https` only). With `hash`, a fragment
/// is part of the path (single-page apps that route by `#/…`).
pub fn page(raw: &str, hash: bool) -> Option<Page> {
    let url = url::Url::parse(raw.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let hostname = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    if hostname.is_empty() {
        return None;
    }
    let mut path = percent_decode(url.path());
    // A hash route keeps its path; its own query goes like the page's.
    if hash
        && let Some(route) = url
            .fragment()
            .map(|f| f.split('?').next().unwrap_or_default())
            .filter(|f| !f.is_empty())
    {
        path.push('#');
        path.push_str(&percent_decode(route));
    }
    let path = short(&scrub(&path), MAX_PATH);
    let mut page = Page {
        hostname: short(&hostname, 253),
        path: if path.is_empty() { "/".into() } else { path },
        ..Page::default()
    };
    for (k, v) in url.query_pairs() {
        let slot = match k.to_ascii_lowercase().as_str() {
            "utm_source" => &mut page.utm_source,
            "utm_medium" => &mut page.utm_medium,
            "utm_campaign" => &mut page.utm_campaign,
            "utm_term" => &mut page.utm_term,
            "utm_content" => &mut page.utm_content,
            _ => continue,
        };
        if slot.is_empty() {
            *slot = short(&scrub(&v), MAX_VALUE);
        }
    }
    Some(page)
}

/// Percent-decoding that keeps invalid sequences and UTF-8 readable.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// A host (as URLs give it: without a port) without `www.` or a trailing
/// dot, lowercase.
fn bare_host(host: &str) -> String {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    host.strip_prefix("www.")
        .map(str::to_string)
        .unwrap_or(host)
}

/// The referring site's domain (without `www.`), unless it is the page's
/// own host (a self-referral) or not a web page.
pub fn referrer_domain(referrer: Option<&str>, own_host: &str) -> String {
    let Some(url) = referrer
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .and_then(|r| url::Url::parse(r).ok())
    else {
        return String::new();
    };
    if !matches!(url.scheme(), "http" | "https") {
        return String::new();
    }
    let Some(host) = url.host_str() else {
        return String::new();
    };
    let host = bare_host(host);
    if host.is_empty() || host.len() > 253 || host == bare_host(own_host) {
        return String::new();
    }
    host
}

/// Screen-width buckets, as stored: `(code, lowest width)`.
pub const SCREENS: &[(&str, u32)] = &[
    ("0-575", 0),
    ("576-991", 576),
    ("992-1439", 992),
    ("1440-1919", 1440),
    ("1920+", 1920),
];

/// The bucket of a screen width in CSS pixels; empty when there is none.
pub fn screen_bucket(width: Option<u64>) -> &'static str {
    match width {
        Some(w) if (1..=100_000).contains(&w) => SCREENS
            .iter()
            .rev()
            .find(|(_, min)| u64::from(*min) <= w)
            .map_or("", |(code, _)| code),
        _ => "",
    }
}

/// The name of a custom event, or `None` when it has none (or is called
/// `pageview`, which only k.js's page views may be).
pub fn event_name(raw: &str) -> Option<String> {
    let name = short(&scrub(raw), MAX_NAME);
    (!name.is_empty() && !name.eq_ignore_ascii_case("pageview")).then_some(name)
}

/// A custom event's properties: at most [`MAX_PROPS`] keys with string,
/// number or boolean values (as text), each at most [`MAX_VALUE`]
/// characters, scrubbed; values under keys that look secret or name an IP
/// address or a user agent are dropped.
/// `None` without any.
pub fn props(raw: Option<&Value>) -> Option<String> {
    let Some(Value::Object(map)) = raw else {
        return None;
    };
    let mut out = Map::new();
    for (k, v) in map {
        if out.len() >= MAX_PROPS {
            break;
        }
        let key = short(k, MAX_NAME);
        if key.is_empty()
            || scrub::is_sensitive_key(&key)
            || scrub::is_ip_key(&key)
            || scrub::is_user_agent_key(&key)
        {
            continue;
        }
        let text = match v {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            _ => continue,
        };
        out.insert(key, Value::from(short(&scrub(&text), MAX_VALUE)));
    }
    (!out.is_empty()).then(|| Value::Object(out).to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_query_goes_except_utm() {
        let p = page(
            "https://Shop.Example.com:8443/pricing/?plan=pro&utm_source=Newsletter&utm_medium=email&utm_campaign=autumn%2026&utm_term=x&utm_content=y&gclid=abc#top",
            false,
        )
        .unwrap();
        assert_eq!(p.hostname, "shop.example.com");
        assert_eq!(p.path, "/pricing/");
        assert_eq!(p.utm_source, "Newsletter");
        assert_eq!(p.utm_medium, "email");
        assert_eq!(p.utm_campaign, "autumn 26");
        assert_eq!((p.utm_term.as_str(), p.utm_content.as_str()), ("x", "y"));
        assert_eq!(page("https://a.example", false).unwrap().path, "/");
        assert_eq!(
            page("https://a.example/caf%C3%A9/%ZZ", false).unwrap().path,
            "/café/%ZZ"
        );
        assert!(page("ftp://a.example/x", false).is_none());
        assert!(page("not a url", false).is_none());
        assert!(page("file:///home/ada/index.html", false).is_none());
    }

    #[test]
    fn hash_routes_only_in_hash_mode() {
        let url = "https://app.example/#/settings/profile?tab=2";
        assert_eq!(page(url, false).unwrap().path, "/");
        assert_eq!(page(url, true).unwrap().path, "/#/settings/profile");
    }

    #[test]
    fn personal_data_in_paths_and_utm_is_scrubbed() {
        let p = page(
            "https://shop.example/users/ada.lovelace@example.com/orders?utm_source=ada%40example.com&utm_campaign=call+%2B47+912+34+567",
            false,
        )
        .unwrap();
        assert_eq!(p.path, "/users/[Filtered]/orders");
        assert_eq!(p.utm_source, "[Filtered]");
        assert_eq!(p.utm_campaign, "call [Filtered]");
        let p = page(
            "https://shop.example/pay/4111111111111111/x/ada%40example.com",
            false,
        )
        .unwrap();
        assert_eq!(p.path, "/pay/[Filtered]/x/[Filtered]");
        assert_eq!(scrub("/call/(555) 123-4567"), "/call/[Filtered]");
        assert_eq!(scrub("/call/0047-912-34-567"), "/call/[Filtered]");
        assert_eq!(scrub("from 203.0.113.7 now"), "from [Filtered] now");
        assert_eq!(scrub("/ip/2001:db8:0:0:1:0:0:1"), "/ip/[Filtered]");
        // Not phone numbers: ids, dates, versions.
        for keep in [
            "/orders/1234567890",
            "/blog/2026-10-22/autumn",
            "/v/1.2.3",
            "/posts/123/comments/456",
        ] {
            assert_eq!(scrub(keep), keep);
        }
    }

    #[test]
    fn referrers_are_foreign_domains() {
        assert_eq!(
            referrer_domain(Some("https://www.Google.com/search?q=x"), "shop.example"),
            "google.com"
        );
        assert_eq!(
            referrer_domain(Some("https://shop.example/a"), "www.shop.example"),
            "",
            "self-referral"
        );
        assert_eq!(
            referrer_domain(Some("https://www.shop.example:443/a"), "shop.example"),
            ""
        );
        assert_eq!(
            referrer_domain(Some("http://news.example:8080/x"), "shop.example"),
            "news.example"
        );
        assert_eq!(
            referrer_domain(Some("android-app://com.slack/"), "shop.example"),
            ""
        );
        assert_eq!(referrer_domain(Some(""), "shop.example"), "");
        assert_eq!(referrer_domain(None, "shop.example"), "");
        assert_eq!(referrer_domain(Some("garbage"), "shop.example"), "");
    }

    #[test]
    fn screens_are_bucketed() {
        assert_eq!(screen_bucket(Some(390)), "0-575");
        assert_eq!(screen_bucket(Some(576)), "576-991");
        assert_eq!(screen_bucket(Some(1280)), "992-1439");
        assert_eq!(screen_bucket(Some(1440)), "1440-1919");
        assert_eq!(screen_bucket(Some(2560)), "1920+");
        assert_eq!(screen_bucket(Some(0)), "");
        assert_eq!(screen_bucket(None), "");
    }

    #[test]
    fn custom_events_are_bounded_and_scrubbed() {
        assert_eq!(event_name(" Signup "), Some("Signup".into()));
        assert_eq!(event_name("pageview"), None);
        assert_eq!(event_name("   "), None);
        assert_eq!(event_name(&"x".repeat(300)).unwrap().len(), MAX_NAME);
        let p = props(Some(&json!({
            "plan": "pro",
            "seats": 5,
            "trial": true,
            "email": "ada@example.com",
            "api_token": "secret-123",
            "nested": {"a": 1}
        })))
        .unwrap();
        let v: Value = serde_json::from_str(&p).unwrap();
        assert_eq!(
            v,
            json!({"plan": "pro", "seats": "5", "trial": "true", "email": "[Filtered]"})
        );
        assert_eq!(props(Some(&json!("text"))), None);
        assert_eq!(props(Some(&json!({}))), None);
        let many: Map<String, Value> = (0..30).map(|i| (format!("k{i}"), json!(i))).collect();
        let p = props(Some(&Value::Object(many))).unwrap();
        let v: Value = serde_json::from_str(&p).unwrap();
        assert_eq!(v.as_object().unwrap().len(), MAX_PROPS);
    }
}
