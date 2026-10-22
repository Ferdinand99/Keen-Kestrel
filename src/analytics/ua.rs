//! What the server reads from a user agent before it forgets it: whether it
//! is a bot, and the browser, operating system and device class
//! (docs/design.md §5.1). Names only, no versions: enough for the dashboards
//! and too little to tell people apart.

use axum::http::HeaderMap;

/// Device classes, as stored (`analytics_events.device`).
pub const DESKTOP: &str = "desktop";
pub const MOBILE: &str = "mobile";
pub const TABLET: &str = "tablet";

/// Browser, OS and device class of a user agent; empty when unknown.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Agent {
    pub browser: &'static str,
    pub os: &'static str,
    pub device: &'static str,
}

/// Crawlers, previews, monitors, scripts and headless or automated browsers
/// (the list Vigilant Vole's status-page analytics uses, plus a few more).
/// Anything that doesn't say it is Mozilla-compatible is not a browser.
pub fn is_bot(user_agent: &str) -> bool {
    const MARKERS: &[&str] = &[
        "bot",
        "crawl",
        "spider",
        "slurp",
        "curl",
        "wget",
        "python",
        "httpclient",
        "http-client",
        "okhttp",
        "java/",
        "go-http",
        "headless",
        "lighthouse",
        "pagespeed",
        "gtmetrix",
        "preview",
        "facebookexternalhit",
        "embedly",
        "monitor",
        "uptime",
        "pingdom",
        "statuscake",
        "axios",
        "node-fetch",
        "undici",
        "scrapy",
        "phantomjs",
        "selenium",
        "puppeteer",
        "playwright",
        "cypress",
        "electron/",
        "feedfetcher",
        "vigilant-vole",
        "keenkestrel",
        "libwww",
        "postman",
        "insomnia",
        "archive.org",
        "yandex.com/bots",
    ];
    let ua = user_agent.trim().to_ascii_lowercase();
    ua.is_empty() || !ua.starts_with("mozilla/") || MARKERS.iter().any(|m| ua.contains(m))
}

/// Headless and automation markers besides the user agent: a prefetch or
/// prerender (not a view), and client hints naming a headless browser.
pub fn is_automated(headers: &HeaderMap) -> bool {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_ascii_lowercase)
    };
    let prefetch = ["purpose", "sec-purpose", "x-purpose", "x-moz"]
        .iter()
        .filter_map(|h| header(h))
        .any(|v| v.contains("prefetch") || v.contains("prerender"));
    let headless_hint = header("sec-ch-ua").is_some_and(|v| v.contains("headless"));
    prefetch || headless_hint
}

/// Reads browser, OS and device class from `ua` (a real browser's).
pub fn parse(ua: &str) -> Agent {
    let has = |m: &str| ua.contains(m);
    let browser = if has("Edg/") || has("EdgA/") || has("EdgiOS/") || has("Edge/") {
        "Edge"
    } else if has("SamsungBrowser/") {
        "Samsung Internet"
    } else if has("OPR/") || has("OPT/") || has("OPiOS/") || has("Opera") {
        "Opera"
    } else if has("YaBrowser/") {
        "Yandex Browser"
    } else if has("Vivaldi/") {
        "Vivaldi"
    } else if has("UCBrowser/") {
        "UC Browser"
    } else if has("DuckDuckGo/") || has("Ddg/") {
        "DuckDuckGo"
    } else if has("FBAN/") || has("FBAV/") || has("FB_IAB/") {
        "Facebook"
    } else if has("Instagram") {
        "Instagram"
    } else if has("Firefox/") || has("FxiOS/") {
        "Firefox"
    } else if has("; wv)") && has("Chrome/") {
        "Android WebView"
    } else if has("CriOS/") || has("Chrome/") || has("Chromium/") {
        "Chrome"
    } else if has("Safari/") || (has("AppleWebKit/") && (has("iPhone") || has("iPad"))) {
        "Safari"
    } else if has("Trident/") || has("MSIE ") {
        "Internet Explorer"
    } else {
        ""
    };
    let os = if has("iPad") {
        "iPadOS"
    } else if has("iPhone") || has("iPod") {
        "iOS"
    } else if has("Android") {
        "Android"
    } else if has("CrOS") {
        "ChromeOS"
    } else if has("Windows") {
        "Windows"
    } else if has("Macintosh") || has("Mac OS X") {
        "macOS"
    } else if has("KaiOS") {
        "KaiOS"
    } else if has("FreeBSD") || has("OpenBSD") {
        "BSD"
    } else if has("Linux") || has("X11") {
        "Linux"
    } else {
        ""
    };
    let device = if has("iPad")
        || has("Tablet")
        || (has("Android") && !has("Mobile"))
        || has("Kindle")
        || has("Silk/")
    {
        TABLET
    } else if has("Mobi") || has("iPhone") || has("iPod") || has("Android") || has("KaiOS") {
        MOBILE
    } else if os.is_empty() && browser.is_empty() {
        ""
    } else {
        DESKTOP
    };
    Agent {
        browser,
        os,
        device,
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    #[test]
    fn real_browsers() {
        let cases = [
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36",
                ("Chrome", "Windows", DESKTOP),
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36 Edg/141.0.0.0",
                ("Edge", "Windows", DESKTOP),
            ),
            (
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15",
                ("Safari", "macOS", DESKTOP),
            ),
            (
                "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0",
                ("Firefox", "Linux", DESKTOP),
            ),
            (
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1",
                ("Safari", "iOS", MOBILE),
            ),
            (
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/141.0 Mobile/15E148 Safari/604.1",
                ("Chrome", "iOS", MOBILE),
            ),
            (
                "Mozilla/5.0 (iPad; CPU OS 17_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.6 Mobile/15E148 Safari/604.1",
                ("Safari", "iPadOS", TABLET),
            ),
            (
                "Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Mobile Safari/537.36",
                ("Chrome", "Android", MOBILE),
            ),
            (
                "Mozilla/5.0 (Linux; Android 14; SM-X710) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36",
                ("Chrome", "Android", TABLET),
            ),
            (
                "Mozilla/5.0 (Linux; Android 14; SAMSUNG SM-S921B) AppleWebKit/537.36 (KHTML, like Gecko) SamsungBrowser/26.0 Chrome/122.0.0.0 Mobile Safari/537.36",
                ("Samsung Internet", "Android", MOBILE),
            ),
            (
                "Mozilla/5.0 (Linux; Android 14; Pixel 8; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/141.0.0.0 Mobile Safari/537.36",
                ("Android WebView", "Android", MOBILE),
            ),
            (
                "Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36",
                ("Chrome", "ChromeOS", DESKTOP),
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36 OPR/120.0.0.0",
                ("Opera", "Windows", DESKTOP),
            ),
            (
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) FxiOS/131.0 Mobile/15E148 Safari/605.1.15",
                ("Firefox", "iOS", MOBILE),
            ),
        ];
        for (ua, (browser, os, device)) in cases {
            assert!(!is_bot(ua), "{ua}");
            assert_eq!(
                parse(ua),
                Agent {
                    browser,
                    os,
                    device
                },
                "{ua}"
            );
        }
        assert_eq!(parse("Mozilla/5.0 (Something)"), Agent::default());
    }

    #[test]
    fn bots_and_scripts() {
        for ua in [
            "",
            "curl/8.4.0",
            "Wget/1.21",
            "python-requests/2.31",
            "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)",
            "Mozilla/5.0 (compatible; bingbot/2.0; +http://www.bing.com/bingbot.htm)",
            "Mozilla/5.0 (Windows NT 10.0) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/141.0.0.0 Safari/537.36",
            "Mozilla/5.0 (Macintosh) AppleWebKit/537.36 (KHTML, like Gecko) Chrome-Lighthouse",
            "Mozilla/5.0 (compatible; UptimeRobot/2.0)",
            "facebookexternalhit/1.1",
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Cypress/13.0 Chrome/120 Electron/27.0.0 Safari/537.36",
        ] {
            assert!(is_bot(ua), "{ua}");
        }
    }

    #[test]
    fn prefetches_and_headless_hints() {
        let mut h = HeaderMap::new();
        assert!(!is_automated(&h));
        h.insert(
            "sec-ch-ua",
            HeaderValue::from_static("\"Chromium\";v=\"141\""),
        );
        assert!(!is_automated(&h));
        h.insert(
            "sec-ch-ua",
            HeaderValue::from_static("\"HeadlessChrome\";v=\"141\""),
        );
        assert!(is_automated(&h));
        let mut h = HeaderMap::new();
        h.insert("sec-purpose", HeaderValue::from_static("prefetch"));
        assert!(is_automated(&h));
    }
}
