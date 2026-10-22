//! Projects and DSN keys: the rules both the web pages and the REST API
//! apply (names, slugs, platforms, rate limits, allowed origins) and the
//! public-key generator. Validation errors are translation keys, so each
//! caller can say them in the request's language.

use std::fmt;

/// Longest project or key name.
pub const MAX_NAME_LEN: usize = 80;
/// Longest slug.
pub const MAX_SLUG_LEN: usize = 50;
/// Highest per-minute rate limit a key can have.
pub const MAX_RATE_LIMIT: i64 = 1_000_000;
/// Most allowed origins per key.
pub const MAX_ORIGINS: usize = 50;
/// Most keys per project.
pub const MAX_KEYS: usize = 20;

/// What a project runs on. Picks the setup guide shown first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// iOS and macOS apps (sentry-cocoa).
    Apple,
    /// Websites (@sentry/browser).
    JavaScript,
    Rust,
    Python,
    Other,
}

impl Platform {
    pub const ALL: [Platform; 5] = [
        Platform::Apple,
        Platform::JavaScript,
        Platform::Rust,
        Platform::Python,
        Platform::Other,
    ];

    /// The value stored in `projects.platform` and used by the API.
    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Apple => "apple",
            Platform::JavaScript => "javascript",
            Platform::Rust => "rust",
            Platform::Python => "python",
            Platform::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_str() == s.trim())
    }

    /// Display name. Platform names are proper names; only "Other" is
    /// translated.
    pub fn label(self) -> &'static str {
        match self {
            Platform::Apple => "Swift (iOS, macOS)",
            Platform::JavaScript => "JavaScript",
            Platform::Rust => "Rust",
            Platform::Python => "Python",
            Platform::Other => vole_kit::i18n::t("projects.platform_other"),
        }
    }

    /// Icon from the design system's sprite.
    pub fn icon(self) -> &'static str {
        match self {
            Platform::Apple => "sparkle",
            Platform::JavaScript => "globe",
            Platform::Rust => "bolt",
            Platform::Python => "code",
            Platform::Other => "box",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A trimmed project or key name of 1–80 characters.
pub fn check_name(raw: &str) -> Result<String, &'static str> {
    let name = raw.trim();
    if name.is_empty() {
        Err("projects.err_name_empty")
    } else if name.chars().count() > MAX_NAME_LEN {
        Err("projects.err_name_long")
    } else {
        Ok(name.to_string())
    }
}

/// A valid slug: lowercase letters, digits and dashes, starting with a letter
/// or digit, at most 50 characters.
pub fn check_slug(raw: &str) -> Result<String, &'static str> {
    let slug = raw.trim();
    let valid = !slug.is_empty()
        && slug.len() <= MAX_SLUG_LEN
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !slug.starts_with('-')
        && !slug.ends_with('-');
    if valid {
        Ok(slug.to_string())
    } else {
        Err("projects.err_slug")
    }
}

/// A slug made from a name: "My iOS App" → "my-ios-app". Characters outside
/// a–z and 0–9 become dashes; a name with none of them gives "project".
pub fn slugify(name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
        if slug.len() >= MAX_SLUG_LEN {
            break;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "project".into()
    } else {
        slug.to_string()
    }
}

/// A per-minute rate limit: `None` (no limit) or 1 to 1,000,000.
pub fn check_rate_limit(limit: Option<i64>) -> Result<Option<i64>, &'static str> {
    match limit {
        None => Ok(None),
        Some(n) if (1..=MAX_RATE_LIMIT).contains(&n) => Ok(Some(n)),
        Some(_) => Err("keys.err_rate_limit"),
    }
}

/// Allowed origins, normalised: `*` or `scheme://host[:port]` (http or
/// https, no path), lower case, without duplicates. An empty list allows
/// any origin.
pub fn check_origins<'a>(
    raw: impl IntoIterator<Item = &'a str>,
) -> Result<Vec<String>, &'static str> {
    let mut out: Vec<String> = Vec::new();
    for item in raw {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let origin = if item == "*" {
            "*".to_string()
        } else {
            normalise_origin(item).ok_or("keys.err_origin")?
        };
        if !out.contains(&origin) {
            out.push(origin);
        }
    }
    if out.len() > MAX_ORIGINS {
        return Err("keys.err_origins_many");
    }
    Ok(out)
}

fn normalise_origin(text: &str) -> Option<String> {
    let url = url::Url::parse(text).ok()?;
    let no_path = matches!(url.path(), "" | "/")
        && url.query().is_none()
        && url.fragment().is_none()
        && url.username().is_empty()
        && url.password().is_none();
    if !matches!(url.scheme(), "http" | "https") || !no_path {
        return None;
    }
    let host = url.host_str()?;
    Some(match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    })
}

/// A new public key: 32 lowercase hex characters from the OS-seeded CSPRNG.
pub fn generate_public_key() -> String {
    let mut bytes = [0u8; 16];
    rand::fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_come_from_names() {
        assert_eq!(slugify("My iOS App"), "my-ios-app");
        assert_eq!(slugify("  Sylo -- web  "), "sylo-web");
        assert_eq!(slugify("Ærlig"), "rlig");
        assert_eq!(slugify("日本"), "project");
        assert!(slugify(&"a".repeat(200)).len() <= MAX_SLUG_LEN);
        assert!(check_slug(&slugify("Tofa (beta)!")).is_ok());
    }

    #[test]
    fn slug_rules() {
        assert!(check_slug("tofa-ios").is_ok());
        for bad in [
            "",
            "Tofa",
            "-tofa",
            "tofa-",
            "to fa",
            "tofa_ios",
            &"a".repeat(51),
        ] {
            assert!(check_slug(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn names_are_trimmed_and_bounded() {
        assert_eq!(check_name("  Sylo  ").unwrap(), "Sylo");
        assert!(check_name("   ").is_err());
        assert!(check_name(&"ø".repeat(80)).is_ok());
        assert!(check_name(&"ø".repeat(81)).is_err());
    }

    #[test]
    fn origins_are_normalised() {
        let got = check_origins([
            "https://Example.com/",
            "",
            "*",
            "http://localhost:5173",
            "https://example.com",
        ])
        .unwrap();
        assert_eq!(got, ["https://example.com", "*", "http://localhost:5173"]);
        for bad in [
            "example.com",
            "ftp://example.com",
            "https://example.com/app",
            "https://example.com?x=1",
            "https://user@example.com",
        ] {
            assert!(check_origins([bad]).is_err(), "{bad}");
        }
    }

    #[test]
    fn rate_limits() {
        assert_eq!(check_rate_limit(None), Ok(None));
        assert_eq!(check_rate_limit(Some(60)), Ok(Some(60)));
        assert!(check_rate_limit(Some(0)).is_err());
        assert!(check_rate_limit(Some(-5)).is_err());
    }

    #[test]
    fn public_keys_are_32_hex_characters() {
        let a = generate_public_key();
        assert_eq!(a.len(), 32);
        assert!(
            a.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        assert_ne!(a, generate_public_key());
    }

    #[test]
    fn platforms_round_trip() {
        for p in Platform::ALL {
            assert_eq!(Platform::parse(p.as_str()), Some(p));
        }
        assert_eq!(Platform::parse("cobol"), None);
    }
}
