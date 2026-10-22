//! Web analytics (docs/design.md §5): pageviews and custom events from the
//! `k.js` snippet, without cookies and without storing IP addresses or user
//! agents.
//!
//! * [`kjs`]: `GET /js/k.js`, the snippet;
//! * `crate::ingest::web`: `POST /ingest/v1/event`, which checks the key,
//!   drops bots and hands one [`WebEvent`] to the writer;
//! * [`clean`]: what is kept of the page (path, UTM, referrer domain, screen
//!   bucket, scrubbed names and properties), [`ua`]: browser, OS and device
//!   class from the user agent;
//! * [`rollup`]: every minute, raw events (`store.sqlite`, 30 days) become
//!   hourly and daily rollups (`kestrel.sqlite`, 400 days);
//! * [`query`]: what the dashboard, the REST API and MCP read;
//! * [`pages`]: the project's Web tab and the overview's summary.
//!
//! A visitor is `trunc64(sha256(daily salt ‖ project ‖ IP ‖ user agent ‖
//! host))`. The salt ([`DailySalt`]) lives in memory only and is replaced at
//! midnight UTC, so a visitor id can't be tied to an address or to the same
//! browser on another day; unique visitors over a range are the sum of the
//! days' uniques.

pub mod clean;
pub mod kjs;
pub mod pages;
pub mod query;
pub mod rollup;
pub mod ua;

use std::net::IpAddr;
use std::sync::Mutex;

use sha2::{Digest, Sha256};

/// The name raw pageviews have (`analytics_events.name`).
pub const PAGEVIEW: &str = "pageview";
/// The source of k.js's events (`source` columns).
pub const WEB: &str = "web";

/// One pageview or custom event, as the writer stores it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebEvent {
    pub project_id: i64,
    /// Unix seconds (UTC), when it arrived.
    pub ts: i64,
    pub visitor: i64,
    /// [`PAGEVIEW`] or a custom event's name.
    pub name: String,
    pub page: clean::Page,
    pub referrer: String,
    pub browser: &'static str,
    pub os: &'static str,
    pub device: &'static str,
    pub screen: &'static str,
    /// Custom event properties (JSON).
    pub props: Option<String>,
}

/// The dimensions the rollups count by: `(code, raw column)`. `entry` is a
/// visitor's first page of the day; `event` counts custom events by name,
/// the others count pageviews.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dim {
    Page,
    Entry,
    Referrer,
    UtmSource,
    UtmMedium,
    UtmCampaign,
    Browser,
    Os,
    Device,
    Screen,
    Event,
}

impl Dim {
    pub const ALL: [Dim; 11] = [
        Dim::Page,
        Dim::Entry,
        Dim::Referrer,
        Dim::UtmSource,
        Dim::UtmMedium,
        Dim::UtmCampaign,
        Dim::Browser,
        Dim::Os,
        Dim::Device,
        Dim::Screen,
        Dim::Event,
    ];

    /// The name in rollups, URLs, the API and MCP.
    pub fn code(self) -> &'static str {
        match self {
            Dim::Page => "page",
            Dim::Entry => "entry",
            Dim::Referrer => "referrer",
            Dim::UtmSource => "utm_source",
            Dim::UtmMedium => "utm_medium",
            Dim::UtmCampaign => "utm_campaign",
            Dim::Browser => "browser",
            Dim::Os => "os",
            Dim::Device => "device",
            Dim::Screen => "screen",
            Dim::Event => "event",
        }
    }

    pub fn parse(code: &str) -> Option<Dim> {
        Dim::ALL.into_iter().find(|d| d.code() == code)
    }

    /// The raw column that holds the value.
    pub fn column(self) -> &'static str {
        match self {
            Dim::Page | Dim::Entry => "path",
            Dim::Referrer => "referrer",
            Dim::UtmSource => "utm_source",
            Dim::UtmMedium => "utm_medium",
            Dim::UtmCampaign => "utm_campaign",
            Dim::Browser => "browser",
            Dim::Os => "os",
            Dim::Device => "device",
            Dim::Screen => "screen",
            Dim::Event => "name",
        }
    }

    /// The number the rollup's seen-set uses (1–11; 0 is the day's
    /// visitors, 12 an hour's).
    pub fn seen_code(self) -> i64 {
        Dim::ALL
            .iter()
            .position(|d| *d == self)
            .map_or(0, |i| i as i64 + 1)
    }

    /// Whether rows with an empty value are counted (no referrer means
    /// direct; no UTM means nothing to show).
    pub fn counts_empty(self) -> bool {
        matches!(
            self,
            Dim::Referrer | Dim::Browser | Dim::Os | Dim::Device | Dim::Screen
        )
    }

    /// Whether the dashboard filters by it (custom events and entry pages
    /// aren't filters: they link to their page instead).
    pub fn is_filter(self) -> bool {
        !matches!(self, Dim::Entry | Dim::Event)
    }

    /// The value of this dimension in `e` (`None`: the event doesn't count
    /// here). Entry pages are decided by the rollup.
    pub fn value_of(self, e: &WebEventRow) -> Option<&str> {
        let pageview = e.name == PAGEVIEW;
        let v = match self {
            Dim::Event => return (!pageview).then_some(e.name.as_str()),
            _ if !pageview => return None,
            Dim::Page | Dim::Entry => &e.path,
            Dim::Referrer => &e.referrer,
            Dim::UtmSource => &e.utm_source,
            Dim::UtmMedium => &e.utm_medium,
            Dim::UtmCampaign => &e.utm_campaign,
            Dim::Browser => &e.browser,
            Dim::Os => &e.os,
            Dim::Device => &e.device,
            Dim::Screen => &e.screen,
        };
        (self.counts_empty() || !v.is_empty()).then_some(v.as_str())
    }
}

/// A raw event as the rollup and the filtered queries read it back.
#[derive(Debug, Clone, Default)]
pub struct WebEventRow {
    pub id: i64,
    pub project_id: i64,
    pub source: String,
    pub ts: i64,
    pub visitor: i64,
    pub name: String,
    pub path: String,
    pub referrer: String,
    pub utm_source: String,
    pub utm_medium: String,
    pub utm_campaign: String,
    pub browser: String,
    pub os: String,
    pub device: String,
    pub screen: String,
}

/// Days since 1970-01-01 (UTC) of a Unix time.
pub fn day_number(ts: i64) -> i64 {
    ts.div_euclid(86_400)
}

/// `YYYY-MM-DD` of a day number.
pub fn day_string(day: i64) -> String {
    jiff::Timestamp::from_second(day * 86_400)
        .map(|t| t.to_zoned(jiff::tz::TimeZone::UTC).date().to_string())
        .unwrap_or_default()
}

/// `YYYY-MM-DD HH:00:00` of a Unix time.
pub fn hour_string(ts: i64) -> String {
    jiff::Timestamp::from_second(ts.div_euclid(3600) * 3600)
        .map(|t| {
            t.to_zoned(jiff::tz::TimeZone::UTC)
                .strftime("%Y-%m-%d %H:00:00")
                .to_string()
        })
        .unwrap_or_default()
}

/// The day number of a `YYYY-MM-DD` date.
pub fn day_of(date: jiff::civil::Date) -> i64 {
    date.to_zoned(jiff::tz::TimeZone::UTC)
        .map(|z| day_number(z.timestamp().as_second()))
        .unwrap_or_default()
}

/// The visitor id: the first 8 bytes of
/// `sha256(salt ‖ project ‖ ip ‖ user_agent ‖ host)` as a signed integer.
pub fn visitor_id(salt: &[u8; 32], project_id: i64, ip: &str, user_agent: &str, host: &str) -> i64 {
    let mut h = Sha256::new();
    h.update(salt);
    h.update(project_id.to_le_bytes());
    h.update(ip.as_bytes());
    h.update([0]);
    h.update(user_agent.as_bytes());
    h.update([0]);
    h.update(host.as_bytes());
    let digest = h.finalize();
    let mut first = [0u8; 8];
    first.copy_from_slice(&digest[..8]);
    i64::from_be_bytes(first)
}

/// The salt of the visitor ids: random, in memory only, replaced when the
/// UTC day changes (and at every start).
pub struct DailySalt {
    current: Mutex<(i64, [u8; 32])>,
}

impl Default for DailySalt {
    fn default() -> Self {
        Self::new()
    }
}

impl DailySalt {
    pub fn new() -> Self {
        DailySalt {
            current: Mutex::new((i64::MIN, [0; 32])),
        }
    }

    /// The salt of day `day` (a [`day_number`]). A later day gets a fresh
    /// salt and the old one is forgotten; an earlier one (a clock that went
    /// back) gets today's.
    pub fn for_day(&self, day: i64) -> [u8; 32] {
        let mut cur = self.current.lock().unwrap_or_else(|e| e.into_inner());
        if day > cur.0 {
            let mut salt = [0u8; 32];
            rand::fill(&mut salt);
            *cur = (day, salt);
        }
        cur.1
    }

    /// The visitor id of a request on day `day`.
    pub fn visitor(
        &self,
        day: i64,
        project_id: i64,
        ip: Option<IpAddr>,
        user_agent: &str,
        host: &str,
    ) -> i64 {
        let ip = ip.map(|ip| ip.to_string()).unwrap_or_default();
        visitor_id(&self.for_day(day), project_id, &ip, user_agent, host)
    }
}

/// FNV-1a: a small, stable 64-bit hash of a dimension's value for the
/// rollup's seen-set (`analytics_seen.value`).
pub fn value_hash(value: &str) -> i64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in value.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    const UA: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0";

    #[test]
    fn the_salt_changes_at_midnight_and_so_do_visitor_ids() {
        let salt = DailySalt::new();
        let ip: Option<IpAddr> = "203.0.113.7".parse().ok();
        let day = day_of(jiff::civil::date(2026, 10, 22));
        let a = salt.visitor(day, 1, ip, UA, "shop.example");
        assert_eq!(a, salt.visitor(day, 1, ip, UA, "shop.example"), "same day");
        assert_ne!(a, salt.visitor(day, 2, ip, UA, "shop.example"), "project");
        assert_ne!(
            a,
            salt.visitor(day, 1, ip, "Mozilla/5.0 other", "shop.example"),
            "browser"
        );
        assert_ne!(a, salt.visitor(day, 1, ip, UA, "blog.example"), "host");
        let next = salt.visitor(day + 1, 1, ip, UA, "shop.example");
        assert_ne!(a, next, "a new day, a new salt");
        // The old salt is gone: asking for yesterday gets today's.
        assert_eq!(next, salt.visitor(day, 1, ip, UA, "shop.example"));
        // Another instance (a restart) has another salt.
        assert_ne!(
            next,
            DailySalt::new().visitor(day + 1, 1, ip, UA, "shop.example")
        );
        // The id is the documented truncated hash.
        let s = [7u8; 32];
        let mut h = Sha256::new();
        h.update(s);
        h.update(1i64.to_le_bytes());
        h.update(b"203.0.113.7\0");
        h.update(UA.as_bytes());
        h.update(b"\0shop.example");
        let d = h.finalize();
        assert_eq!(
            visitor_id(&s, 1, "203.0.113.7", UA, "shop.example"),
            i64::from_be_bytes(d[..8].try_into().unwrap())
        );
    }

    #[test]
    fn days_and_hours() {
        let ts = 1_792_627_200 + 3 * 3600 + 59; // 2026-10-22 03:00:59 UTC
        assert_eq!(day_string(day_number(ts)), "2026-10-22");
        assert_eq!(hour_string(ts), "2026-10-22 03:00:00");
        assert_eq!(day_of(jiff::civil::date(2026, 10, 22)), day_number(ts));
        assert_eq!(day_number(-1), -1);
    }

    #[test]
    fn dimensions() {
        for d in Dim::ALL {
            assert_eq!(Dim::parse(d.code()), Some(d));
            assert!((1..=11).contains(&d.seen_code()));
        }
        let pv = WebEventRow {
            name: PAGEVIEW.into(),
            path: "/a".into(),
            ..WebEventRow::default()
        };
        assert_eq!(Dim::Page.value_of(&pv), Some("/a"));
        assert_eq!(Dim::Referrer.value_of(&pv), Some(""), "direct");
        assert_eq!(Dim::UtmSource.value_of(&pv), None);
        assert_eq!(Dim::Event.value_of(&pv), None);
        let ev = WebEventRow {
            name: "Signup".into(),
            path: "/a".into(),
            ..WebEventRow::default()
        };
        assert_eq!(Dim::Event.value_of(&ev), Some("Signup"));
        assert_eq!(Dim::Page.value_of(&ev), None);
        assert_ne!(value_hash("/a"), value_hash("/b"));
        assert_eq!(value_hash(""), 0xcbf2_9ce4_8422_2325_u64 as i64);
    }
}
