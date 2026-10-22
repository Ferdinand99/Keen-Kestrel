//! How Kestrel words its alerts: vole-kit's [`Alert`] for an issue (new,
//! regressed, spiking, a release's first crash), a digest of several, and
//! the test message. The texts are written in the current language (the
//! channel's); the kit lays them out for each provider.
//!
//! An alert is built when it is sent, from the data its log entry keeps.

use serde_json::Value;
use time::OffsetDateTime;
use vole_kit::i18n;
use vole_kit::notify::alert::{Alert, AlertKind, Level};

/// Facts a digest lists at most.
const DIGEST_FACTS: usize = 20;

fn s<'a>(v: &'a Value, path: &[&str]) -> &'a str {
    let mut cur = v;
    for key in path {
        match cur.get(key) {
            Some(next) => cur = next,
            None => return "",
        }
    }
    cur.as_str().unwrap_or("")
}

fn n(v: &Value, path: &[&str]) -> i64 {
    let mut cur = v;
    for key in path {
        match cur.get(key) {
            Some(next) => cur = next,
            None => return 0,
        }
    }
    cur.as_i64().unwrap_or(0)
}

/// The kit's level for an issue's level.
pub fn level(issue_level: &str) -> Level {
    match issue_level {
        "fatal" | "error" => Level::Critical,
        "warning" => Level::Warning,
        _ => Level::Info,
    }
}

fn emoji(kind: AlertKind, level: Level) -> &'static str {
    if kind == AlertKind::Escalation {
        return kind.emoji();
    }
    match level {
        Level::Critical => "\u{1f534}",
        Level::Warning => "\u{1f7e1}",
        Level::Ok => "\u{1f7e2}",
        Level::Info => "\u{1f535}",
    }
}

/// The name people read for a kind of alert ("New issue").
pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        "new_issue" => i18n::t("alert_msg.event_new_issue"),
        "regression" => i18n::t("alert_msg.event_regression"),
        "spike" => i18n::t("alert_msg.event_spike"),
        "first_crash" => i18n::t("alert_msg.event_first_crash"),
        "digest" => i18n::t("alert_msg.event_digest"),
        _ => i18n::t("alert_msg.event_test"),
    }
}

/// "New issue in Shop" and the like: the alert's headline.
pub fn headline(data: &Value) -> String {
    let project = s(data, &["project", "name"]);
    match s(data, &["kind"]) {
        "regression" => i18n::tf("alert_msg.regression", &[("project", project)]),
        "spike" => i18n::tf(
            "alert_msg.spike",
            &[
                ("project", project),
                ("n", &i18n::fmt_int(n(data, &["spike", "last_hour"]))),
            ],
        ),
        "first_crash" => {
            let release = match s(data, &["crash_release"]) {
                "" => s(data, &["release"]),
                r => r,
            };
            i18n::tf(
                "alert_msg.first_crash",
                &[("project", project), ("release", release)],
            )
        }
        _ => i18n::tf("alert_msg.new_issue", &[("project", project)]),
    }
}

/// `base` joined with a path that starts with `/`.
fn link(base: Option<&str>, path: &str) -> Option<String> {
    let base = base?.trim_end_matches('/');
    (!base.is_empty() && !path.is_empty()).then(|| format!("{base}{path}"))
}

/// The alert for one log entry's data.
pub fn alert(data: &Value, base_url: Option<&str>) -> Alert {
    let kind_name = s(data, &["kind"]).to_string();
    let kind = match kind_name.as_str() {
        "spike" => AlertKind::Escalation,
        _ => AlertKind::Problem,
    };
    let issue_level = s(data, &["issue", "level"]);
    let lvl = if kind_name == "first_crash" {
        Level::Critical
    } else {
        level(issue_level)
    };
    let title = s(data, &["issue", "title"]).to_string();
    let culprit = s(data, &["issue", "culprit"]);
    let project = s(data, &["project", "name"]);
    let mut facts: Vec<(String, String)> = vec![(
        i18n::t("alert_msg.fact_project").to_string(),
        project.to_string(),
    )];
    if !issue_level.is_empty() {
        facts.push((
            i18n::t("alert_msg.fact_level").to_string(),
            i18n::t(crate::issues::level_key(issue_level)).to_string(),
        ));
    }
    let release = match s(data, &["crash_release"]) {
        "" => s(data, &["release"]),
        r => r,
    };
    if !release.is_empty() {
        facts.push((
            i18n::t("alert_msg.fact_release").to_string(),
            release.to_string(),
        ));
    }
    let environment = s(data, &["environment"]);
    if !environment.is_empty() {
        facts.push((
            i18n::t("alert_msg.fact_environment").to_string(),
            environment.to_string(),
        ));
    }
    if !culprit.is_empty() {
        facts.push((
            i18n::t("alert_msg.fact_culprit").to_string(),
            culprit.to_string(),
        ));
    }
    if data.get("spike").is_some() {
        facts.push((
            i18n::t("alert_msg.fact_last_hour").to_string(),
            i18n::fmt_int(n(data, &["spike", "last_hour"])),
        ));
        facts.push((
            i18n::t("alert_msg.fact_limit").to_string(),
            i18n::fmt_int(n(data, &["spike", "limit"])),
        ));
    }
    facts.push((
        i18n::t("alert_msg.fact_events").to_string(),
        i18n::fmt_int(n(data, &["issue", "events"])),
    ));
    let users = n(data, &["issue", "users"]);
    if users > 0 {
        facts.push((
            i18n::t("alert_msg.fact_users").to_string(),
            i18n::fmt_int(users),
        ));
    }
    let url = link(base_url, s(data, &["path"]));
    if url.is_none() {
        facts.push((
            i18n::t("alert_msg.fact_link").to_string(),
            i18n::t("alert_msg.no_link").to_string(),
        ));
    }
    Alert {
        kind,
        event: kind_name.clone(),
        event_label: kind_label(&kind_name).to_string(),
        status: kind_name,
        level: lvl,
        colour: lvl.colour(),
        emoji: emoji(kind, lvl).to_string(),
        title: headline(data),
        detail: (!title.is_empty()).then(|| title.clone()),
        facts,
        link_label: i18n::t("alert_msg.view_issue").to_string(),
        subject_id: data
            .get("issue")
            .and_then(|i| i.get("id"))
            .and_then(Value::as_i64),
        subject: title.clone(),
        subject_type: "issue".to_string(),
        target: project.to_string(),
        message: (!title.is_empty()).then_some(title),
        duration: None,
        expires_in_days: None,
        url,
        occurred_at: OffsetDateTime::now_utc(),
    }
}

/// One message for several alerts (a channel's digest): `(kind, data)` each.
pub fn digest(entries: &[(String, Value)], base_url: Option<&str>) -> Alert {
    if let [(_, data)] = entries {
        return alert(data, base_url);
    }
    let count = i64::try_from(entries.len()).unwrap_or(i64::MAX);
    let lvl = entries
        .iter()
        .map(|(kind, data)| {
            if kind == "first_crash" {
                Level::Critical
            } else {
                level(s(data, &["issue", "level"]))
            }
        })
        .min_by_key(|l| match l {
            Level::Critical => 0,
            Level::Warning => 1,
            Level::Ok => 3,
            Level::Info => 2,
        })
        .unwrap_or(Level::Info);
    let mut facts: Vec<(String, String)> = entries
        .iter()
        .take(DIGEST_FACTS)
        .map(|(kind, data)| {
            (
                kind_label(kind).to_string(),
                format!(
                    "{}: {}",
                    s(data, &["project", "name"]),
                    s(data, &["issue", "title"])
                ),
            )
        })
        .collect();
    if entries.len() > DIGEST_FACTS {
        facts.push((
            "…".to_string(),
            i18n::tn("alert_msg.digest_more", count - DIGEST_FACTS as i64),
        ));
    }
    // One project: link its alerts page; several: the projects.
    let mut projects: Vec<i64> = entries
        .iter()
        .filter_map(|(_, d)| {
            d.get("project")
                .and_then(|p| p.get("id"))
                .and_then(Value::as_i64)
        })
        .collect();
    projects.sort_unstable();
    projects.dedup();
    let path = match projects.as_slice() {
        [one] => format!("/projects/{one}/alerts"),
        _ => "/projects".to_string(),
    };
    let url = link(base_url, &path);
    if url.is_none() {
        facts.push((
            i18n::t("alert_msg.fact_link").to_string(),
            i18n::t("alert_msg.no_link").to_string(),
        ));
    }
    let kind = AlertKind::Problem;
    let title = i18n::tnf(
        "alert_msg.digest_title",
        count,
        &[("product", crate::product::NAME)],
    );
    Alert {
        kind,
        event: "digest".into(),
        event_label: kind_label("digest").to_string(),
        status: "digest".into(),
        level: lvl,
        colour: lvl.colour(),
        emoji: emoji(kind, lvl).to_string(),
        title: title.clone(),
        detail: None,
        facts,
        link_label: i18n::t("alert_msg.view_alerts").to_string(),
        subject_id: None,
        subject: title,
        subject_type: "digest".into(),
        target: String::new(),
        message: None,
        duration: None,
        expires_in_days: None,
        url,
        occurred_at: OffsetDateTime::now_utc(),
    }
}

/// The message "Send test" sends.
pub fn test(base_url: Option<&str>) -> Alert {
    let kind = AlertKind::Test;
    let lvl = kind.level();
    let title = i18n::tf("alert_msg.test_title", &[("product", crate::product::NAME)]);
    let body = i18n::t("alert_msg.test_body").to_string();
    let url = link(base_url, "/projects");
    let mut facts = Vec::new();
    if url.is_none() {
        facts.push((
            i18n::t("alert_msg.fact_link").to_string(),
            i18n::t("alert_msg.no_link").to_string(),
        ));
    }
    Alert {
        kind,
        event: "test".into(),
        event_label: kind_label("test").to_string(),
        status: "test".into(),
        level: lvl,
        colour: lvl.colour(),
        emoji: kind.emoji().to_string(),
        title: title.clone(),
        detail: Some(body.clone()),
        facts,
        link_label: i18n::t("alert_msg.view_projects").to_string(),
        subject_id: None,
        subject: title,
        subject_type: String::new(),
        target: String::new(),
        message: Some(body),
        duration: None,
        expires_in_days: None,
        url,
        occurred_at: OffsetDateTime::now_utc(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn data(kind: &str) -> Value {
        json!({
            "kind": kind,
            "project": {"id": 3, "name": "Shop"},
            "issue": {"id": 7, "title": "ValueError: bad input", "culprit": "shop.views in checkout",
                      "level": "error", "events": 1234, "users": 2},
            "release": "shop@1.2.0",
            "environment": "production",
            "path": "/projects/3/issues/7",
        })
    }

    #[test]
    fn a_new_issue_reads_naturally() {
        crate::product::install();
        let a = alert(&data("new_issue"), Some("https://kestrel.example/"));
        assert_eq!(a.kind, AlertKind::Problem);
        assert_eq!(a.level, Level::Critical);
        assert_eq!(a.title, "New issue in Shop");
        assert_eq!(a.detail.as_deref(), Some("ValueError: bad input"));
        assert_eq!(
            a.url.as_deref(),
            Some("https://kestrel.example/projects/3/issues/7")
        );
        let facts: Vec<String> = a.facts.iter().map(|(k, v)| format!("{k}: {v}")).collect();
        assert!(facts.contains(&"Project: Shop".to_string()), "{facts:?}");
        assert!(
            facts.contains(&"Release: shop@1.2.0".to_string()),
            "{facts:?}"
        );
        assert!(
            facts.contains(&"Environment: production".to_string()),
            "{facts:?}"
        );
        assert!(facts.contains(&"Events: 1,234".to_string()), "{facts:?}");
        assert_eq!(
            vole_kit::notify::summary(&a),
            "\u{1f534} New issue in Shop — ValueError: bad input"
        );
    }

    #[test]
    fn without_a_public_url_the_alert_says_how_to_get_links() {
        crate::product::install();
        let a = alert(&data("regression"), None);
        assert!(a.url.is_none());
        assert!(a.facts.iter().any(|(_, v)| v.contains("KK_PUBLIC_URL")));
        assert_eq!(a.title, "Issue regressed in Shop");
    }

    #[test]
    fn spikes_and_first_crashes_say_what_happened() {
        crate::product::install();
        let mut d = data("spike");
        d["spike"] = json!({"last_hour": 512, "limit": 100, "baseline": 3.5});
        let a = alert(&d, None);
        assert_eq!(a.kind, AlertKind::Escalation);
        assert_eq!(a.title, "Spike in Shop: 512 events in the last hour");
        let mut c = data("first_crash");
        c["crash_release"] = json!("shop@1.3.0");
        let a = alert(&c, None);
        assert_eq!(a.level, Level::Critical);
        assert_eq!(a.title, "First crash in shop@1.3.0 (Shop)");
    }

    #[test]
    fn a_digest_lists_its_alerts() {
        crate::product::install();
        let entries = vec![
            ("new_issue".to_string(), data("new_issue")),
            ("regression".to_string(), data("regression")),
        ];
        let a = digest(&entries, Some("https://k.example"));
        assert_eq!(a.title, "2 alerts from Keen Kestrel");
        assert_eq!(a.facts.len(), 2);
        assert_eq!(
            a.url.as_deref(),
            Some("https://k.example/projects/3/alerts")
        );
    }
}
