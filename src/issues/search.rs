//! The issue search syntax (docs/design.md §4.5):
//!
//! ```text
//! is:unresolved level:error release:1.4 env:production os:iOS free text
//! ```
//!
//! * `is:` takes a status (`unresolved`, `resolved`, `ignored`, `all`), a
//!   substatus (`new`, `regressed`, `ongoing`) or `unsymbolicated`;
//! * `level:` one of the five levels;
//! * `release:` a version as sent (`shop@1.4.0+12`), or only its version
//!   (`1.4.0`), with `*` as a wildcard (`1.4*`);
//! * `env:` (or `environment:`) an environment;
//! * `os:`, `browser:`, `device:` and `runtime:` a name with or without its
//!   version (`os:iOS`, `os:"iOS 26.2"`);
//! * any other `key:value` a tag the SDK sent (`shop:north`);
//! * `!` before a filter turns it around (`!env:staging`);
//! * values and phrases with spaces go in double quotes;
//! * everything else is free text, matched against titles and culprits
//!   (each word as a prefix); an event id finds the issue it belongs to.
//!
//! Parsing never fails: what can't be understood is listed in
//! [`Query::unknown`] and otherwise left out.

use crate::event::normalize::{self, LEVELS};

/// An issue's status filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Unresolved,
    Resolved,
    Ignored,
    /// Any status.
    All,
}

impl Status {
    pub const ALL: [Status; 4] = [
        Status::Unresolved,
        Status::Resolved,
        Status::Ignored,
        Status::All,
    ];

    pub fn parse(s: &str) -> Option<Status> {
        match s.trim().to_ascii_lowercase().as_str() {
            "unresolved" => Some(Status::Unresolved),
            "resolved" => Some(Status::Resolved),
            "ignored" | "archived" => Some(Status::Ignored),
            "all" | "any" => Some(Status::All),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Status::Unresolved => "unresolved",
            Status::Resolved => "resolved",
            Status::Ignored => "ignored",
            Status::All => "all",
        }
    }

    /// The `issues.status` value, `None` for any.
    pub fn column(self) -> Option<&'static str> {
        match self {
            Status::All => None,
            s => Some(s.as_str()),
        }
    }
}

/// How a tag filter compares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagKind {
    /// `release:` — the version as sent, or the part after `package@`.
    Release,
    /// `os:`, `browser:`, `device:`, `runtime:` — the name or name and
    /// version (the derived `<key>.name` and `<key>` tags).
    Named,
    /// Any other key: the value as sent.
    Plain,
}

/// `key:value`, matched against the issue's tag values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagFilter {
    pub key: String,
    pub value: String,
    pub kind: TagKind,
    pub negated: bool,
}

impl TagFilter {
    pub fn new(key: &str, value: &str, negated: bool) -> TagFilter {
        let key = key.to_ascii_lowercase();
        let (key, kind) = match key.as_str() {
            "release" => ("release".to_string(), TagKind::Release),
            "env" | "environment" => ("environment".to_string(), TagKind::Plain),
            "os" | "browser" | "device" | "runtime" | "client_os" => (key, TagKind::Named),
            _ => (key, TagKind::Plain),
        };
        TagFilter {
            key,
            value: value.to_string(),
            kind,
            negated,
        }
    }

    /// The tag keys this filter looks at.
    pub fn keys(&self) -> Vec<String> {
        match self.kind {
            TagKind::Named if self.key == "device" => vec!["device".into(), "device.family".into()],
            TagKind::Named => vec![self.key.clone(), format!("{}.name", self.key)],
            _ => vec![self.key.clone()],
        }
    }

    /// `LIKE` patterns (escape `\`) a matching value fits, case-insensitively.
    pub fn patterns(&self) -> Vec<String> {
        let v = like_pattern(&self.value);
        match self.kind {
            TagKind::Release if !self.value.contains('@') => {
                vec![
                    v.clone(),
                    format!("%@{v}"),
                    format!("%@{v}+%"),
                    format!("{v}+%"),
                ]
            }
            _ => vec![v],
        }
    }
}

/// `value` as a `LIKE` pattern: `*` is the wildcard, `%`, `_` and `\` are
/// literal.
pub fn like_pattern(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' | '%' | '_' => {
                out.push('\\');
                out.push(c);
            }
            '*' => out.push('%'),
            c => out.push(c),
        }
    }
    out
}

/// A parsed search.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    /// `is:unresolved` and friends; `None` leaves the status to the page.
    pub status: Option<Status>,
    /// `is:new`, `is:regressed`, `is:ongoing`.
    pub substatus: Option<String>,
    /// `is:unsymbolicated`.
    pub unsymbolicated: bool,
    /// `level:` values, and whether each is negated.
    pub levels: Vec<(String, bool)>,
    pub tags: Vec<TagFilter>,
    /// Free words (prefixes) and quoted phrases, for full-text search.
    pub words: Vec<String>,
    pub phrases: Vec<String>,
    /// Event ids found among the free words.
    pub event_ids: Vec<String>,
    /// Terms that weren't understood, as typed.
    pub unknown: Vec<String>,
}

/// One term of a search.
struct Term<'a> {
    /// Without the quotes.
    text: String,
    /// As typed.
    raw: &'a str,
    /// It began with a quote: a phrase, never a filter.
    phrase: bool,
}

/// Splits a search into terms: whitespace separates them, except inside
/// double quotes, which are removed.
fn terms(input: &str) -> Vec<Term<'_>> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut start: Option<usize> = None;
    let mut quoted = false;
    let mut phrase = false;
    for (i, c) in input.char_indices() {
        if c.is_whitespace() && !quoted {
            if let Some(s) = start.take() {
                out.push(Term {
                    text: std::mem::take(&mut text),
                    raw: &input[s..i],
                    phrase,
                });
            }
            continue;
        }
        if start.is_none() {
            start = Some(i);
            phrase = c == '"';
        }
        if c == '"' {
            quoted = !quoted;
        } else {
            text.push(c);
        }
    }
    if let Some(s) = start {
        out.push(Term {
            text,
            raw: &input[s..],
            phrase,
        });
    }
    out.retain(|t| !t.text.trim().is_empty());
    out
}

fn is_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 32
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// Parses a search. At most 20 terms are read.
pub fn parse(input: &str) -> Query {
    let mut q = Query::default();
    for Term {
        text: term, phrase, ..
    } in terms(input).into_iter().take(20)
    {
        let (negated, body) = match term.strip_prefix('!') {
            Some(rest) if !rest.is_empty() => (true, rest),
            _ => (false, term.as_str()),
        };
        let filter = body
            .split_once(':')
            .filter(|(k, v)| !phrase && is_key(k) && !v.trim().is_empty());
        let Some((key, value)) = filter else {
            if phrase {
                q.phrases.push(term.trim().to_string());
            } else if let Some(id) = normalize::event_id(&term) {
                q.event_ids.push(id);
            } else {
                q.words.push(term);
            }
            continue;
        };
        let value = value.trim();
        match key.to_ascii_lowercase().as_str() {
            "is" => {
                let v = value.to_ascii_lowercase();
                if negated {
                    q.unknown.push(term.clone());
                } else if let Some(status) = Status::parse(&v) {
                    q.status = Some(status);
                } else if matches!(v.as_str(), "new" | "regressed" | "ongoing") {
                    q.substatus = Some(v);
                } else if v == "unsymbolicated" {
                    q.unsymbolicated = true;
                } else {
                    q.unknown.push(term.clone());
                }
            }
            "level" => {
                let v = value.to_ascii_lowercase();
                if LEVELS.contains(&v.as_str()) {
                    q.levels.push((v, negated));
                } else {
                    q.unknown.push(term.clone());
                }
            }
            _ => q.tags.push(TagFilter::new(key, value, negated)),
        }
    }
    q
}

/// The `MATCH` expression for the free text, or `None` without any: each
/// word as a quoted prefix, each phrase as a quoted phrase.
pub fn fts_match(q: &Query) -> Option<String> {
    let quote = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    let mut parts: Vec<String> = q
        .words
        .iter()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .map(|w| format!("{}*", quote(w)))
        .collect();
    parts.extend(
        q.phrases
            .iter()
            .filter(|p| p.chars().any(char::is_alphanumeric))
            .map(|p| quote(p)),
    );
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// The order of the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    #[default]
    LastSeen,
    FirstSeen,
    Events,
    Users,
}

impl Sort {
    pub const ALL: [Sort; 4] = [Sort::LastSeen, Sort::FirstSeen, Sort::Events, Sort::Users];

    pub fn parse(s: &str) -> Option<Sort> {
        match s {
            "last_seen" => Some(Sort::LastSeen),
            "first_seen" => Some(Sort::FirstSeen),
            "events" => Some(Sort::Events),
            "users" => Some(Sort::Users),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Sort::LastSeen => "last_seen",
            Sort::FirstSeen => "first_seen",
            Sort::Events => "events",
            Sort::Users => "users",
        }
    }

    /// `ORDER BY` for `issues i`.
    pub fn order_by(self) -> &'static str {
        match self {
            Sort::LastSeen => "i.last_seen DESC, i.id DESC",
            Sort::FirstSeen => "i.first_seen DESC, i.id DESC",
            Sort::Events => "i.times_seen DESC, i.last_seen DESC, i.id DESC",
            Sort::Users => "i.users_seen DESC, i.times_seen DESC, i.id DESC",
        }
    }

    /// Translation key of its label.
    pub fn label_key(self) -> &'static str {
        match self {
            Sort::LastSeen => "issues.sort_last_seen",
            Sort::FirstSeen => "issues.sort_first_seen",
            Sort::Events => "issues.sort_events",
            Sort::Users => "issues.sort_users",
        }
    }
}

/// Removes the `is:` status terms from a search (when the status chips
/// change the status), keeping everything else as typed.
pub fn without_status(input: &str) -> String {
    terms(input)
        .into_iter()
        .filter(|t| {
            t.phrase
                || !t.text.split_once(':').is_some_and(|(k, v)| {
                    k.eq_ignore_ascii_case("is") && Status::parse(v).is_some()
                })
        })
        .map(|t| t.raw)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(key: &str, value: &str, kind: TagKind, negated: bool) -> TagFilter {
        TagFilter {
            key: key.into(),
            value: value.into(),
            kind,
            negated,
        }
    }

    #[test]
    fn the_design_example() {
        let q = parse("is:unresolved level:error release:1.4 env:production os:iOS free text");
        assert_eq!(q.status, Some(Status::Unresolved));
        assert_eq!(q.levels, [("error".to_string(), false)]);
        assert_eq!(
            q.tags,
            [
                tag("release", "1.4", TagKind::Release, false),
                tag("environment", "production", TagKind::Plain, false),
                tag("os", "iOS", TagKind::Named, false),
            ]
        );
        assert_eq!(q.words, ["free", "text"]);
        assert!(q.unknown.is_empty() && q.phrases.is_empty() && q.event_ids.is_empty());
        assert_eq!(fts_match(&q).as_deref(), Some(r#""free"* "text"*"#));
    }

    #[test]
    fn statuses_substatuses_and_flags() {
        assert_eq!(parse("is:resolved").status, Some(Status::Resolved));
        assert_eq!(parse("IS:Ignored").status, Some(Status::Ignored));
        assert_eq!(parse("is:all").status, Some(Status::All));
        let q = parse("is:regressed is:unsymbolicated");
        assert_eq!(q.status, None);
        assert_eq!(q.substatus.as_deref(), Some("regressed"));
        assert!(q.unsymbolicated);
        let q = parse("is:sleeping !is:resolved level:loud");
        assert_eq!(q.unknown, ["is:sleeping", "!is:resolved", "level:loud"]);
        assert_eq!(q.status, None);
        assert!(q.levels.is_empty());
    }

    #[test]
    fn quotes_negation_and_custom_tags() {
        let q = parse(r#"!env:staging os:"iOS 26.2" shop:north "connection refused" level:FATAL"#);
        assert_eq!(
            q.tags,
            [
                tag("environment", "staging", TagKind::Plain, true),
                tag("os", "iOS 26.2", TagKind::Named, false),
                tag("shop", "north", TagKind::Plain, false),
            ]
        );
        assert_eq!(q.phrases, ["connection refused"]);
        assert_eq!(q.levels, [("fatal".to_string(), false)]);
        assert_eq!(fts_match(&q).as_deref(), Some(r#""connection refused""#));
    }

    #[test]
    fn free_text_is_quoted_for_fts() {
        let q = parse(r#"Value"Error" ** : a:"#);
        // `a:` has no value, so it is a word; `**` and `:` have no letters.
        assert_eq!(q.words, ["ValueError", "**", ":", "a:"]);
        assert_eq!(fts_match(&q).as_deref(), Some(r#""ValueError"* "a:"*"#));
        assert_eq!(fts_match(&parse("")), None);
        assert_eq!(fts_match(&parse("level:error")), None);
    }

    #[test]
    fn event_ids_find_their_issue() {
        let q = parse("9EC79C33-EC99-42AB-8353-589FCB2E04DC");
        assert_eq!(q.event_ids, ["9ec79c33ec9942ab8353589fcb2e04dc"]);
        assert!(q.words.is_empty());
    }

    #[test]
    fn release_patterns_match_the_version_alone() {
        let f = TagFilter::new("release", "1.4", false);
        assert_eq!(f.patterns(), ["1.4", "%@1.4", "%@1.4+%", "1.4+%"]);
        let f = TagFilter::new("release", "shop@1.4*", false);
        assert_eq!(f.patterns(), ["shop@1.4%"]);
        let f = TagFilter::new("os", "iOS", false);
        assert_eq!(f.keys(), ["os", "os.name"]);
        let f = TagFilter::new("device", "iPhone", false);
        assert_eq!(f.keys(), ["device", "device.family"]);
        assert_eq!(like_pattern(r"50%_a\b*"), r"50\%\_a\\b%");
    }

    #[test]
    fn at_most_twenty_terms() {
        let long = (0..30)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(parse(&long).words.len(), 20);
    }

    #[test]
    fn the_status_chips_take_the_status_out_of_the_search() {
        assert_eq!(
            without_status(r#"is:resolved level:error os:"iOS 26" "two words" is:new"#),
            r#"level:error os:"iOS 26" "two words" is:new"#
        );
        assert_eq!(without_status("is:ignored"), "");
    }

    #[test]
    fn sorts_round_trip() {
        for s in Sort::ALL {
            assert_eq!(Sort::parse(s.as_str()), Some(s));
        }
        for s in Status::ALL {
            assert_eq!(Status::parse(s.as_str()), Some(s));
        }
        assert_eq!(Sort::parse("nope"), None);
    }
}
