//! `GET /js/k.js`: the web snippet (docs/design.md §5.4). The readable
//! source is `src/analytics/k.js`; it is served with its comment lines,
//! indentation and blank lines removed (a rule simple enough not to change
//! what the script does: the source has no comments after code and no
//! multi-line strings), gzip-compressed for browsers that take it, with an
//! ETag and a day of caching. It must stay under 2 KB gzip-compressed.

use std::io::Write as _;
use std::sync::LazyLock;

use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

/// The readable source.
pub const SOURCE: &str = include_str!("k.js");
/// The most the served script may weigh gzip-compressed.
pub const MAX_GZIP: usize = 2048;

/// The script as served.
pub struct Script {
    pub body: String,
    pub gzip: Vec<u8>,
    pub etag: String,
}

/// The source without comment lines, indentation and blank lines.
pub fn minify(source: &str) -> String {
    source
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// The served script, made once.
pub static SCRIPT: LazyLock<Script> = LazyLock::new(|| {
    let body = minify(SOURCE);
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    // Writing to memory can't fail.
    let _ = gz.write_all(body.as_bytes());
    let gzip = gz.finish().unwrap_or_default();
    let etag = format!(
        "\"k-{}\"",
        hex::encode(&Sha256::digest(body.as_bytes())[..8])
    );
    Script { body, gzip, etag }
});

/// `GET /js/k.js`.
pub async fn serve(headers: HeaderMap) -> Response {
    let script = &*SCRIPT;
    let etag = HeaderValue::from_str(&script.etag).unwrap_or(HeaderValue::from_static("\"k\""));
    let fresh = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',')
                .any(|t| t.trim().trim_start_matches("W/") == script.etag || t.trim() == "*")
        });
    let gzip = headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',').any(|e| {
                let mut parts = e.split(';');
                let name = parts.next().unwrap_or("").trim();
                let q0 = parts.any(|p| p.trim().replace(' ', "") == "q=0");
                name.eq_ignore_ascii_case("gzip") && !q0
            })
        });
    let mut res = if fresh {
        StatusCode::NOT_MODIFIED.into_response()
    } else if gzip {
        let mut r = script.gzip.clone().into_response();
        r.headers_mut()
            .insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        r
    } else {
        script.body.clone().into_response()
    };
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/javascript; charset=utf-8"),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=86400"),
    );
    h.insert(header::ETAG, etag);
    h.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    h.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    // `Cross-Origin-Resource-Policy: cross-origin` comes from the route's
    // header policy (`headers::public_asset`).
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_stays_under_two_kilobytes_gzipped() {
        let s = &*SCRIPT;
        assert!(
            s.gzip.len() < MAX_GZIP,
            "k.js is {} bytes gzip-compressed",
            s.gzip.len()
        );
        assert!(s.body.len() < SOURCE.len());
        let mut out = String::new();
        use std::io::Read as _;
        flate2::read::GzDecoder::new(&s.gzip[..])
            .read_to_string(&mut out)
            .unwrap();
        assert_eq!(out, s.body);
    }

    #[test]
    fn minifying_only_drops_comment_lines_and_whitespace() {
        // The rule is safe only if no line has code and then a comment, and
        // no string spans lines.
        for line in SOURCE.lines().map(str::trim) {
            if line.starts_with("//") {
                continue;
            }
            assert!(!line.contains(" //"), "comment after code: {line}");
            assert!(!line.contains("/*"), "block comment: {line}");
            assert!(!line.contains('`'), "template string: {line}");
        }
        let min = minify(SOURCE);
        assert!(
            !min.lines()
                .any(|l| l.starts_with("//") || l.starts_with(' '))
        );
        // Every line of code is still there, in order.
        let code: Vec<&str> = SOURCE
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("//"))
            .collect();
        assert_eq!(min.lines().collect::<Vec<_>>(), code);
        // What the script must never do.
        for banned in [
            "document.cookie",
            "localStorage.setItem",
            "sessionStorage",
            "indexedDB",
        ] {
            assert!(!min.contains(banned), "{banned}");
        }
    }
}
