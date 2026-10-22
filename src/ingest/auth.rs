//! Which DSN key sent a request: the `X-Sentry-Auth` header (or
//! `Authorization: Sentry …`), the `sentry_key` query parameter, or the
//! envelope header's `dsn`. Legacy secret keys are accepted and ignored.

use axum::http::HeaderMap;
use rusqlite::{Connection, OptionalExtension};

/// A DSN key as ingest needs it.
#[derive(Debug, Clone)]
pub struct KeyInfo {
    pub id: i64,
    pub project_id: i64,
    pub enabled: bool,
    pub rate_limit_per_minute: Option<i64>,
    /// Empty: any origin.
    pub allowed_origins: Vec<String>,
    pub keep_user_details: bool,
}

impl KeyInfo {
    /// Whether a browser at `origin` may send with this key.
    pub fn allows_origin(&self, origin: &str) -> bool {
        let origin = origin.trim().trim_end_matches('/').to_ascii_lowercase();
        self.allowed_origins.is_empty()
            || self
                .allowed_origins
                .iter()
                .any(|o| o == "*" || *o == origin)
    }

    /// Whether the key lets any origin send (`Access-Control-Allow-Origin: *`).
    pub fn any_origin(&self) -> bool {
        self.allowed_origins.is_empty() || self.allowed_origins.iter().any(|o| o == "*")
    }
}

/// A public key as stored: 32 lowercase hex characters.
fn well_formed(key: &str) -> Option<String> {
    let key = key.trim().to_ascii_lowercase();
    (key.len() == 32 && key.bytes().all(|b| b.is_ascii_hexdigit())).then_some(key)
}

/// `sentry_key` from `Sentry sentry_version=7, sentry_key=…, …`.
pub fn from_auth_header(value: &str) -> Option<String> {
    let rest = value.trim();
    let rest = rest
        .strip_prefix("Sentry ")
        .or_else(|| rest.strip_prefix("sentry "))
        .unwrap_or(rest);
    rest.split(',').find_map(|part| {
        let (k, v) = part.split_once('=')?;
        (k.trim() == "sentry_key").then(|| v.trim().to_string())
    })
}

/// The public key and project id of a DSN.
pub fn from_dsn(dsn: &str) -> Option<(String, Option<i64>)> {
    let url = url::Url::parse(dsn.trim()).ok()?;
    let key = url.username();
    let project = url
        .path_segments()
        .and_then(|mut s| s.rfind(|p| !p.is_empty()))
        .and_then(|p| p.parse().ok());
    Some((key.to_string(), project))
}

/// The key from the headers or the query string, if any. `Some(Err(()))`
/// when one was given but isn't a well-formed key.
pub fn from_request(headers: &HeaderMap, query: Option<&str>) -> Option<Result<String, ()>> {
    let header = headers
        .get("x-sentry-auth")
        .or_else(|| {
            headers.get(axum::http::header::AUTHORIZATION).filter(|v| {
                v.to_str()
                    .is_ok_and(|s| s.trim_start().starts_with("Sentry "))
            })
        })
        .and_then(|v| v.to_str().ok())
        .and_then(from_auth_header);
    let query_key = || {
        url::form_urlencoded::parse(query.unwrap_or("").as_bytes())
            .find(|(k, _)| k == "sentry_key")
            .map(|(_, v)| v.into_owned())
    };
    header.or_else(query_key).map(|k| well_formed(&k).ok_or(()))
}

/// Normalises a key from a DSN.
pub fn checked(key: &str) -> Option<String> {
    well_formed(key)
}

/// Looks a public key up.
pub fn lookup(c: &Connection, public_key: &str) -> rusqlite::Result<Option<KeyInfo>> {
    c.prepare_cached(
        "SELECT k.id, k.project_id, k.enabled, k.rate_limit_per_minute, k.allowed_origins,
                p.keep_user_details
         FROM project_keys k JOIN projects p ON p.id = k.project_id
         WHERE k.public_key = ?1",
    )?
    .query_row([public_key], |r| {
        let origins: String = r.get(4)?;
        Ok(KeyInfo {
            id: r.get(0)?,
            project_id: r.get(1)?,
            enabled: r.get(2)?,
            rate_limit_per_minute: r.get(3)?,
            allowed_origins: origins
                .lines()
                .map(str::trim)
                .filter(|o| !o.is_empty())
                .map(str::to_string)
                .collect(),
            keep_user_details: r.get(5)?,
        })
    })
    .optional()
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    const KEY: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn keys_from_headers_queries_and_dsns() {
        assert_eq!(
            from_auth_header(&format!(
                "Sentry sentry_version=7,sentry_client=sentry.cocoa/9.30.2,sentry_key={KEY}"
            ))
            .as_deref(),
            Some(KEY)
        );
        let mut h = HeaderMap::new();
        assert_eq!(from_request(&h, None), None);
        assert_eq!(
            from_request(&h, Some(&format!("sentry_version=7&sentry_key={KEY}"))),
            Some(Ok(KEY.into()))
        );
        h.insert(
            "x-sentry-auth",
            HeaderValue::from_str(&format!(
                "Sentry sentry_key={}, sentry_secret=x",
                KEY.to_uppercase()
            ))
            .unwrap(),
        );
        assert_eq!(from_request(&h, None), Some(Ok(KEY.into())));
        h.insert(
            "x-sentry-auth",
            HeaderValue::from_static("Sentry sentry_key=nope"),
        );
        assert_eq!(from_request(&h, None), Some(Err(())));
        assert_eq!(
            from_dsn(&format!("https://{KEY}@kestrel.example.com/kestrel/12")),
            Some((KEY.to_string(), Some(12)))
        );
    }

    #[test]
    fn origins() {
        let mut k = KeyInfo {
            id: 1,
            project_id: 1,
            enabled: true,
            rate_limit_per_minute: None,
            allowed_origins: vec![],
            keep_user_details: false,
        };
        assert!(k.allows_origin("https://evil.example") && k.any_origin());
        k.allowed_origins = vec!["https://shop.example".into()];
        assert!(k.allows_origin("https://Shop.example/"));
        assert!(!k.allows_origin("https://evil.example") && !k.any_origin());
        k.allowed_origins.push("*".into());
        assert!(k.allows_origin("https://evil.example") && k.any_origin());
    }
}
