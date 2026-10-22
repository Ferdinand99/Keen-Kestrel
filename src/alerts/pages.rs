//! A project's Alerts tab (docs/design.md §4.7): its rules and what they
//! sent. Everyone signed in reads it; editors and admins add, change and
//! delete rules.

use askama::Template;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum_extra::extract::cookie::SignedCookieJar;
use serde::Deserialize;
use vole_kit::auth::CurrentUser;
use vole_kit::http::{ApiError, ApiResult, HtmlTemplate};
use vole_kit::i18n::{self, Tr as _};
use vole_kit::page::{PageCtx, flash};
use vole_kit::state::Core;

use super::message;
use crate::db::alerts::{self as db, Channel, LogEntry, Rule, RuleInput, RuleKind};
use crate::db::projects::{self, Project};
use crate::issues::Paging;
use crate::pages::ProjectView;

/// History entries per page.
pub const HISTORY_PAGE: i64 = 50;
/// Longest rule name.
pub const NAME_MAX: usize = 100;
/// Cooldown choices in minutes (0: none).
pub const COOLDOWNS: [i64; 9] = [0, 5, 15, 30, 60, 180, 360, 720, 1440];
/// Digest choices in minutes.
pub const DIGESTS: [i64; 6] = [5, 15, 30, 60, 180, 1440];

pub fn router() -> Router<Core> {
    Router::new()
        .route("/projects/{id}/alerts", get(alerts_page).post(create))
        .route("/projects/{id}/alerts/new", get(new_page))
        .route(
            "/projects/{id}/alerts/{rule_id}",
            get(edit_page).post(update),
        )
        .route("/projects/{id}/alerts/{rule_id}/delete", post(delete))
}

async fn load_project(core: &Core, id: i64) -> ApiResult<Project> {
    core.with_conn(move |c| projects::get(c, id))
        .await?
        .ok_or(ApiError::NotFound)
}

/// The translation key of a rule kind's name.
pub fn kind_key(kind: RuleKind) -> &'static str {
    match kind {
        RuleKind::NewIssue => "alerts.kind_new_issue",
        RuleKind::Regression => "alerts.kind_regression",
        RuleKind::Spike => "alerts.kind_spike",
        RuleKind::FirstCrash => "alerts.kind_first_crash",
    }
}

fn kind_help_key(kind: RuleKind) -> &'static str {
    match kind {
        RuleKind::NewIssue => "alerts.kind_new_issue_help",
        RuleKind::Regression => "alerts.kind_regression_help",
        RuleKind::Spike => "alerts.kind_spike_help",
        RuleKind::FirstCrash => "alerts.kind_first_crash_help",
    }
}

/// "60 min", "3 h", "1 d".
pub fn minutes_label(minutes: i64) -> String {
    if minutes == 0 {
        return i18n::t("alerts.cooldown_none").to_string();
    }
    i18n::fmt_duration(minutes * 60)
}

/// What a rule's row says about it: "Spike: more than 100 events an hour
/// and 5× the usual · every 60 min at most".
fn rule_summary(r: &Rule) -> String {
    let mut parts = Vec::new();
    match r.kind {
        RuleKind::Spike => parts.push(i18n::tf(
            "alerts.spike_summary",
            &[
                ("n", &i18n::fmt_int(r.threshold)),
                (
                    "k",
                    &i18n::fmt_dec(
                        r.multiplier,
                        if r.multiplier.fract() == 0.0 { 0 } else { 1 },
                    ),
                ),
            ],
        )),
        _ if !r.environment.is_empty() => {
            parts.push(i18n::tf("alerts.env_summary", &[("env", &r.environment)]))
        }
        _ => {}
    }
    if r.cooldown_minutes > 0 {
        parts.push(i18n::tf(
            "alerts.cooldown_summary",
            &[("time", &minutes_label(r.cooldown_minutes))],
        ));
    }
    if let Some(d) = r.digest_minutes {
        parts.push(i18n::tf(
            "alerts.digest_summary",
            &[("time", &minutes_label(d))],
        ));
    }
    parts.join(" · ")
}

pub struct RuleRow {
    pub id: i64,
    pub href: String,
    pub name: String,
    pub kind_label: &'static str,
    pub summary: String,
    pub channels: String,
    pub enabled: bool,
}

pub struct HistoryRow {
    pub time: String,
    pub kind_label: &'static str,
    pub headline: String,
    pub issue_title: String,
    pub issue_href: String,
    pub rule: String,
    pub channel: String,
    pub status: String,
    pub status_label: &'static str,
    pub status_class: &'static str,
    pub error: String,
    pub attempts: i64,
}

fn history_row(project_id: i64, e: &LogEntry) -> HistoryRow {
    let (status_label, status_class) = match e.status.as_str() {
        "sent" => (i18n::t("alerts.status_sent"), "status-success"),
        "failed" => (i18n::t("alerts.status_failed"), "status-error"),
        "digest" => (i18n::t("alerts.status_digest"), "status-info"),
        "skipped" => (i18n::t("alerts.status_skipped"), "status-paused"),
        _ => (i18n::t("alerts.status_pending"), "status-pending"),
    };
    let error = match e.error.as_deref() {
        Some("channel paused") => i18n::t("alerts.err_paused").to_string(),
        Some("channel deleted") => i18n::t("alerts.err_channel_deleted").to_string(),
        Some("rule deleted") => i18n::t("alerts.err_rule_deleted").to_string(),
        Some(other) => other.chars().take(300).collect(),
        None => String::new(),
    };
    HistoryRow {
        time: e.sent_at.clone().unwrap_or_else(|| e.created_at.clone()),
        kind_label: message::kind_label(&e.kind),
        headline: message::headline(&e.data),
        issue_title: e
            .data
            .get("issue")
            .and_then(|i| i.get("title"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string(),
        issue_href: e
            .issue_id
            .map(|i| format!("/projects/{project_id}/issues/{i}"))
            .unwrap_or_default(),
        rule: e.rule_name.clone(),
        channel: e.channel_name.clone(),
        status: e.status.clone(),
        status_label,
        status_class,
        error,
        attempts: e.attempts,
    }
}

#[derive(Template)]
#[template(path = "alerts.html")]
struct AlertsTemplate {
    ctx: PageCtx,
    project: ProjectView,
    tab: &'static str,
    rules: Vec<RuleRow>,
    history: Vec<HistoryRow>,
    paging: Paging,
    has_channels: bool,
    new_href: String,
}

#[derive(Deserialize, Default)]
struct PageParams {
    #[serde(default)]
    page: Option<i64>,
}

async fn alerts_page(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    Path(id): Path<i64>,
    Query(params): Query<PageParams>,
) -> ApiResult<Response> {
    let project = load_project(&core, id).await?;
    let page = params.page.unwrap_or(1).max(1);
    let (rules, channels, (log, total)) = core
        .with_conn(move |c| {
            Ok((
                db::rules(c, id)?,
                db::channels(c)?,
                db::history(c, id, HISTORY_PAGE, (page - 1) * HISTORY_PAGE)?,
            ))
        })
        .await?;
    let names = |ids: &[i64]| -> String {
        let list: Vec<&str> = ids
            .iter()
            .filter_map(|i| channels.iter().find(|c| c.id == *i))
            .map(|c| c.name.as_str())
            .collect();
        if list.is_empty() {
            i18n::t("alerts.no_channels").to_string()
        } else {
            list.join(", ")
        }
    };
    let rows = rules
        .iter()
        .map(|r| RuleRow {
            id: r.id,
            href: format!("/projects/{id}/alerts/{}", r.id),
            name: r.name.clone(),
            kind_label: i18n::t(kind_key(r.kind)),
            summary: rule_summary(r),
            channels: names(&r.channel_ids),
            enabled: r.enabled,
        })
        .collect();
    let base = format!("/projects/{id}/alerts");
    let paging = Paging::new(page, HISTORY_PAGE, total, |p| {
        format!("{base}?page={p}#history")
    });
    let title = i18n::tf("alerts.page_title", &[("name", &project.name)]);
    let (ctx, jar) = PageCtx::build(&core, Some(&user), &jar, title, "projects", "").await;
    let page = AlertsTemplate {
        ctx,
        project: ProjectView::from(&project),
        tab: "alerts",
        rules: rows,
        history: log.iter().map(|e| history_row(id, e)).collect(),
        paging,
        has_channels: !channels.is_empty(),
        new_href: format!("/projects/{id}/alerts/new"),
    };
    Ok((jar, HtmlTemplate(page)).into_response())
}

// ---------------------------------------------------------------------------
// The rule form
// ---------------------------------------------------------------------------

pub struct KindChoice {
    pub value: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub checked: bool,
}

pub struct ChannelChoice {
    pub id: i64,
    pub name: String,
    pub kind_label: String,
    pub checked: bool,
    pub active: bool,
}

pub struct MinuteChoice {
    pub value: i64,
    pub label: String,
    pub selected: bool,
}

/// The rule form's values and errors.
pub struct RuleForm {
    pub id: i64,
    pub name: String,
    pub kind: &'static str,
    pub enabled: bool,
    pub environment: String,
    pub threshold: String,
    pub multiplier: String,
    pub cooldown: Vec<MinuteChoice>,
    pub digest: bool,
    pub digest_choices: Vec<MinuteChoice>,
    pub kinds: Vec<KindChoice>,
    pub channels: Vec<ChannelChoice>,
    pub environments: Vec<String>,
    pub errors: Vec<(&'static str, String)>,
}

impl RuleForm {
    pub fn error(&self, field: &str) -> String {
        self.errors
            .iter()
            .find(|(f, _)| *f == field)
            .map(|(_, m)| m.clone())
            .unwrap_or_default()
    }
}

fn form_view(
    id: i64,
    input: &RuleInput,
    threshold: String,
    multiplier: String,
    channels: &[Channel],
    environments: Vec<String>,
    errors: Vec<(&'static str, String)>,
) -> RuleForm {
    let mut cooldowns: Vec<i64> = COOLDOWNS.to_vec();
    if !cooldowns.contains(&input.cooldown_minutes) {
        cooldowns.push(input.cooldown_minutes);
        cooldowns.sort_unstable();
    }
    let digest_minutes = input.digest_minutes.unwrap_or(60);
    let mut digests: Vec<i64> = DIGESTS.to_vec();
    if !digests.contains(&digest_minutes) {
        digests.push(digest_minutes);
        digests.sort_unstable();
    }
    RuleForm {
        id,
        name: input.name.clone(),
        kind: input.kind.as_str(),
        enabled: input.enabled,
        environment: input.environment.clone(),
        threshold,
        multiplier,
        cooldown: cooldowns
            .into_iter()
            .map(|m| MinuteChoice {
                value: m,
                label: minutes_label(m),
                selected: m == input.cooldown_minutes,
            })
            .collect(),
        digest: input.digest_minutes.is_some(),
        digest_choices: digests
            .into_iter()
            .map(|m| MinuteChoice {
                value: m,
                label: minutes_label(m),
                selected: m == digest_minutes,
            })
            .collect(),
        kinds: RuleKind::ALL
            .into_iter()
            .map(|k| KindChoice {
                value: k.as_str(),
                label: i18n::t(kind_key(k)),
                help: i18n::t(kind_help_key(k)),
                checked: k == input.kind,
            })
            .collect(),
        channels: channels
            .iter()
            .map(|c| ChannelChoice {
                id: c.id,
                name: c.name.clone(),
                kind_label: vole_kit::notify::provider(&c.kind)
                    .map_or_else(|| c.kind.clone(), |p| p.tr_label().to_string()),
                checked: input.channel_ids.contains(&c.id),
                active: c.active,
            })
            .collect(),
        environments,
        errors,
    }
}

#[derive(Template)]
#[template(path = "alert_rule.html")]
struct RuleTemplate {
    ctx: PageCtx,
    project: ProjectView,
    form: RuleForm,
    action: String,
    back_href: String,
    delete_action: String,
    is_new: bool,
}

/// Checks a rule's settings; `(field, message)` for each problem.
pub fn check_rule(r: &RuleInput) -> Vec<(&'static str, String)> {
    let mut errors = Vec::new();
    let name = r.name.trim();
    if name.is_empty() {
        errors.push(("name", i18n::t("alerts.err_name").to_string()));
    } else if name.chars().count() > NAME_MAX {
        errors.push(("name", i18n::t("alerts.err_name_long").to_string()));
    }
    if r.environment.chars().count() > 200 {
        errors.push(("environment", i18n::t("alerts.err_environment").to_string()));
    }
    if !(1..=10_000_000).contains(&r.threshold) {
        errors.push(("threshold", i18n::t("alerts.err_threshold").to_string()));
    }
    if !(1.0..=1000.0).contains(&r.multiplier) || r.multiplier.is_nan() {
        errors.push(("multiplier", i18n::t("alerts.err_multiplier").to_string()));
    }
    if !(0..=525_600).contains(&r.cooldown_minutes) {
        errors.push((
            "cooldown_minutes",
            i18n::t("alerts.err_cooldown").to_string(),
        ));
    }
    if r.digest_minutes.is_some_and(|d| !(1..=1440).contains(&d)) {
        errors.push(("digest_minutes", i18n::t("alerts.err_digest").to_string()));
    }
    if r.channel_ids.is_empty() {
        errors.push(("channels", i18n::t("alerts.err_channels").to_string()));
    }
    errors
}

/// A rule from the form's fields, with the texts of the number fields (to
/// show them back as typed).
fn parse_form(pairs: &[(String, String)]) -> (RuleInput, String, String) {
    let get = |k: &str| {
        pairs
            .iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.trim().to_string())
            .unwrap_or_default()
    };
    let kind = RuleKind::parse(&get("kind")).unwrap_or(RuleKind::NewIssue);
    let threshold_text = get("threshold");
    let multiplier_text = get("multiplier").replace(',', ".");
    let mut input = RuleInput::new(kind);
    input.name = get("name");
    input.enabled = matches!(get("enabled").as_str(), "true" | "on" | "1");
    input.environment = if kind == RuleKind::Spike {
        String::new()
    } else {
        get("environment")
    };
    if kind == RuleKind::Spike {
        input.threshold = threshold_text.parse().unwrap_or(0);
        input.multiplier = multiplier_text.parse().unwrap_or(0.0);
    }
    input.cooldown_minutes = get("cooldown_minutes").parse().unwrap_or(-1);
    input.digest_minutes = if matches!(get("digest").as_str(), "true" | "on" | "1") {
        Some(get("digest_minutes").parse().unwrap_or(0))
    } else {
        None
    };
    input.channel_ids = pairs
        .iter()
        .filter(|(k, _)| k == "channels")
        .filter_map(|(_, v)| v.parse().ok())
        .collect();
    (input, threshold_text, multiplier_text)
}

async fn form_data(core: &Core, id: i64) -> ApiResult<(Vec<Channel>, Vec<String>)> {
    core.with_conn(move |c| Ok((db::channels(c)?, crate::db::triage::environments(c, id)?)))
        .await
}

#[allow(clippy::too_many_arguments)]
async fn render_form(
    core: &Core,
    user: &CurrentUser,
    jar: &SignedCookieJar,
    project: &Project,
    rule_id: Option<i64>,
    input: &RuleInput,
    texts: (String, String),
    errors: Vec<(&'static str, String)>,
) -> ApiResult<(SignedCookieJar, HtmlTemplate<RuleTemplate>)> {
    let id = project.id;
    let (channels, environments) = form_data(core, id).await?;
    let title = match rule_id {
        Some(_) => input.name.clone(),
        None => i18n::t("alerts.new_rule").to_string(),
    };
    let (ctx, jar) = PageCtx::build(core, Some(user), jar, title, "projects", "").await;
    let (threshold, multiplier) = texts;
    Ok((
        jar,
        HtmlTemplate(RuleTemplate {
            ctx,
            project: ProjectView::from(project),
            form: form_view(
                rule_id.unwrap_or(0),
                input,
                threshold,
                multiplier,
                &channels,
                environments,
                errors,
            ),
            action: match rule_id {
                Some(r) => format!("/projects/{id}/alerts/{r}"),
                None => format!("/projects/{id}/alerts"),
            },
            back_href: format!("/projects/{id}/alerts"),
            delete_action: rule_id
                .map(|r| format!("/projects/{id}/alerts/{r}/delete"))
                .unwrap_or_default(),
            is_new: rule_id.is_none(),
        }),
    ))
}

fn number_texts(r: &RuleInput) -> (String, String) {
    let k = if r.multiplier.fract() == 0.0 {
        format!("{}", r.multiplier as i64)
    } else {
        format!("{}", r.multiplier)
    };
    (r.threshold.to_string(), k)
}

#[derive(Deserialize, Default)]
struct NewParams {
    #[serde(default)]
    kind: String,
}

async fn new_page(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    Path(id): Path<i64>,
    Query(params): Query<NewParams>,
) -> ApiResult<Response> {
    user.require_editor()?;
    let project = load_project(&core, id).await?;
    let kind = RuleKind::parse(&params.kind).unwrap_or(RuleKind::NewIssue);
    let mut input = RuleInput::new(kind);
    input.name = i18n::t(kind_key(kind)).to_string();
    // Every channel by default: most instances have one or two.
    let channels = core.with_conn(|c| db::channels(c)).await?;
    input.channel_ids = channels.iter().filter(|c| c.active).map(|c| c.id).collect();
    let texts = number_texts(&input);
    Ok(render_form(
        &core,
        &user,
        &jar,
        &project,
        None,
        &input,
        texts,
        Vec::new(),
    )
    .await?
    .into_response())
}

async fn create(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    Path(id): Path<i64>,
    body: axum::body::Bytes,
) -> ApiResult<Response> {
    user.require_editor()?;
    let project = load_project(&core, id).await?;
    let pairs: Vec<(String, String)> = url::form_urlencoded::parse(&body).into_owned().collect();
    let (input, threshold, multiplier) = parse_form(&pairs);
    let errors = check_rule(&input);
    if !errors.is_empty() {
        let (jar, page) = render_form(
            &core,
            &user,
            &jar,
            &project,
            None,
            &input,
            (threshold, multiplier),
            errors,
        )
        .await?;
        return Ok((StatusCode::UNPROCESSABLE_ENTITY, jar, page).into_response());
    }
    let mut stored = input.clone();
    stored.name = input.name.trim().to_string();
    core.with_conn(move |c| db::insert_rule(c, id, &stored))
        .await?;
    tracing::info!(user = %user.username, project = id, kind = input.kind.as_str(), "alert rule created");
    Ok((
        flash(
            jar,
            "success",
            i18n::tf("alerts.created", &[("name", input.name.trim())]),
        ),
        Redirect::to(&format!("/projects/{id}/alerts")),
    )
        .into_response())
}

async fn edit_page(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    Path((id, rule_id)): Path<(i64, i64)>,
) -> ApiResult<Response> {
    user.require_editor()?;
    let project = load_project(&core, id).await?;
    let rule = core
        .with_conn(move |c| db::rule(c, id, rule_id))
        .await?
        .ok_or(ApiError::NotFound)?;
    let input = RuleInput::from_rule(&rule);
    let texts = number_texts(&input);
    Ok(render_form(
        &core,
        &user,
        &jar,
        &project,
        Some(rule_id),
        &input,
        texts,
        Vec::new(),
    )
    .await?
    .into_response())
}

async fn update(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    Path((id, rule_id)): Path<(i64, i64)>,
    body: axum::body::Bytes,
) -> ApiResult<Response> {
    user.require_editor()?;
    let project = load_project(&core, id).await?;
    let exists = core
        .with_conn(move |c| db::rule(c, id, rule_id))
        .await?
        .is_some();
    if !exists {
        return Err(ApiError::NotFound);
    }
    let pairs: Vec<(String, String)> = url::form_urlencoded::parse(&body).into_owned().collect();
    let (input, threshold, multiplier) = parse_form(&pairs);
    let errors = check_rule(&input);
    if !errors.is_empty() {
        let (jar, page) = render_form(
            &core,
            &user,
            &jar,
            &project,
            Some(rule_id),
            &input,
            (threshold, multiplier),
            errors,
        )
        .await?;
        return Ok((StatusCode::UNPROCESSABLE_ENTITY, jar, page).into_response());
    }
    let mut stored = input.clone();
    stored.name = input.name.trim().to_string();
    core.with_conn(move |c| db::update_rule(c, id, rule_id, &stored))
        .await?;
    tracing::info!(user = %user.username, project = id, rule = rule_id, "alert rule updated");
    Ok((
        flash(
            jar,
            "success",
            i18n::tf("alerts.saved", &[("name", input.name.trim())]),
        ),
        Redirect::to(&format!("/projects/{id}/alerts")),
    )
        .into_response())
}

async fn delete(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    Path((id, rule_id)): Path<(i64, i64)>,
) -> ApiResult<Response> {
    user.require_editor()?;
    let rule = core
        .with_conn(move |c| db::rule(c, id, rule_id))
        .await?
        .ok_or(ApiError::NotFound)?;
    core.with_conn(move |c| db::delete_rule(c, id, rule_id))
        .await?;
    tracing::info!(user = %user.username, project = id, rule = rule_id, "alert rule deleted");
    Ok((
        flash(
            jar,
            "success",
            i18n::tf("alerts.deleted", &[("name", &rule.name)]),
        ),
        Redirect::to(&format!("/projects/{id}/alerts")),
    )
        .into_response())
}
