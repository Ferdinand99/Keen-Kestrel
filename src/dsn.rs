//! DSNs and the setup snippets that carry them.
//!
//! A DSN is `<scheme>://<public key>@<host>[/<path>]/<project id>`: the
//! official Sentry SDKs send to `/api/<project id>/envelope/` under that
//! address. The address is the public URL (`KK_PUBLIC_URL`, kept in the
//! `base_url` setting) when one is set, else the one the browser used.

use axum::http::{HeaderMap, header};
use vole_kit::auth::session;

/// This instance's public address without a trailing slash, e.g.
/// `https://kestrel.example.com`: the configured public URL, else the
/// request's scheme and host (honouring `X-Forwarded-*` only with
/// `KK_TRUST_PROXY`).
pub fn public_base(base_url: Option<String>, headers: &HeaderMap) -> String {
    if let Some(base) = base_url {
        return base.trim_end_matches('/').to_string();
    }
    let trust = session::trust_proxy();
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.split(',').next().unwrap_or_default().trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let host = trust
        .then(|| header("x-forwarded-host"))
        .flatten()
        .or_else(|| header(header::HOST.as_str()))
        .unwrap_or_else(|| "localhost".into());
    let scheme = if session::is_https(headers, trust) {
        "https"
    } else {
        "http"
    };
    format!("{scheme}://{host}")
}

/// The DSN of `public_key` for project `project_id` at `base`.
pub fn dsn(base: &str, public_key: &str, project_id: i64) -> String {
    let (scheme, rest) = base.split_once("://").unwrap_or(("https", base));
    let rest = rest.trim_end_matches('/');
    format!("{scheme}://{public_key}@{rest}/{project_id}")
}

/// One SDK's setup: how to install it and how to start it with the DSN.
pub struct Snippet {
    /// Element id suffix and tab id (`swift`, `javascript`, …).
    pub id: &'static str,
    /// Tab label (a language name).
    pub label: &'static str,
    /// The SDK's package name.
    pub sdk: &'static str,
    pub install: String,
    pub code: String,
}

/// Setup snippets for Swift, JavaScript, Rust and Python, all with `dsn`.
pub fn snippets(dsn: &str) -> Vec<Snippet> {
    vec![
        Snippet {
            id: "swift",
            label: "Swift",
            sdk: "sentry-cocoa",
            install: "// Xcode: File → Add Package Dependencies…\nhttps://github.com/getsentry/sentry-cocoa".into(),
            code: format!(
                "import Sentry\n\nSentrySDK.start {{ options in\n    options.dsn = \"{dsn}\"\n}}"
            ),
        },
        Snippet {
            id: "javascript",
            label: "JavaScript",
            sdk: "@sentry/browser",
            install: "npm install @sentry/browser".into(),
            code: format!(
                "import * as Sentry from \"@sentry/browser\";\n\nSentry.init({{\n  dsn: \"{dsn}\",\n}});"
            ),
        },
        Snippet {
            id: "rust",
            label: "Rust",
            sdk: "sentry",
            install: "cargo add sentry".into(),
            // `ClientOptions` is `#[non_exhaustive]`: a struct literal with
            // `..Default::default()` doesn't compile outside the crate (E0639).
            code: format!(
                "let mut options = sentry::ClientOptions::default();\noptions.release = sentry::release_name!();\noptions.attach_stacktrace = true;\nlet _guard = sentry::init((\"{dsn}\", options));"
            ),
        },
        Snippet {
            id: "python",
            label: "Python",
            sdk: "sentry-sdk",
            install: "pip install sentry-sdk".into(),
            code: format!("import sentry_sdk\n\nsentry_sdk.init(dsn=\"{dsn}\")"),
        },
    ]
}

/// The snippet tab shown first for a platform.
pub fn first_snippet(platform: crate::project::Platform) -> &'static str {
    use crate::project::Platform;
    match platform {
        Platform::Apple | Platform::Other => "swift",
        Platform::JavaScript => "javascript",
        Platform::Rust => "rust",
        Platform::Python => "python",
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    #[test]
    fn dsns_put_the_key_before_the_host_and_the_id_last() {
        let key = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            dsn("https://kestrel.example.com", key, 7),
            "https://0123456789abcdef0123456789abcdef@kestrel.example.com/7"
        );
        assert_eq!(
            dsn("http://10.0.3.26:3002/", key, 12),
            "http://0123456789abcdef0123456789abcdef@10.0.3.26:3002/12"
        );
        assert_eq!(
            dsn("https://example.com/kestrel", key, 1),
            "https://0123456789abcdef0123456789abcdef@example.com/kestrel/1"
        );
    }

    #[test]
    fn the_public_url_wins_over_the_request() {
        crate::product::install();
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("10.0.3.26:3002"));
        assert_eq!(public_base(None, &headers), "http://10.0.3.26:3002");
        assert_eq!(
            public_base(Some("https://errors.example.com/".into()), &headers),
            "https://errors.example.com"
        );
    }

    #[test]
    fn every_snippet_carries_the_dsn() {
        let all = snippets("https://k@h/1");
        let ids: Vec<&str> = all.iter().map(|s| s.id).collect();
        assert_eq!(ids, ["swift", "javascript", "rust", "python"]);
        assert!(all.iter().all(|s| s.code.contains("\"https://k@h/1\"")));
        // No struct literal for the non-exhaustive `ClientOptions`.
        let rust = &all[2].code;
        assert!(rust.contains("sentry::ClientOptions::default()"), "{rust}");
        assert!(!rust.contains("..Default::default()"), "{rust}");
    }
}
