//! The issue pages (docs/design.md §4.5, §6): a project's issues with
//! filters, search, sorting, paging, bulk actions and live updates; one
//! issue with its latest event, events, tags, grouping and activity; one
//! event. Everyone signed in reads them; editors and admins resolve,
//! ignore, merge and delete.
//!
//! The actions run on the writer's connection ([`crate::writer::Writer::run`]),
//! so they never race ingest, and each one is noted in the issue's activity.

pub mod live;
pub mod search;
pub mod view;

use askama::Template;
use axum::Router;
use axum::extract::{Extension, Path, Query as UrlQuery, State};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum_extra::extract::Form;
use axum_extra::extract::cookie::SignedCookieJar;
use serde::Deserialize;
use serde_json::Value;
use vole_kit::auth::CurrentUser;
use vole_kit::http::{ApiError, ApiResult, HtmlTemplate};
use vole_kit::i18n::{self, Tr as _};
use vole_kit::page::{PageCtx, flash};
use vole_kit::state::Core;

use crate::db::projects::{self, Project};
use crate::db::triage::{self, EventRow, Ignore, IssueRow, ListParams};
use crate::event::normalize;
use crate::ingest::Ingest;
use crate::pages::ProjectView;
use search::{Query, Sort, Status, TagFilter};
use view::{EventView, FrameGroup, GroupingView};

/// Issues per page of the list.
pub const PAGE_SIZE: i64 = 25;
/// Events per page of an issue's event list.
pub const EVENTS_PAGE_SIZE: i64 = 50;
/// Largest payload read back from the store (normalised events are smaller).
const MAX_PAYLOAD: usize = 8 << 20;

pub fn router() -> Router<Core> {
    Router::new()
        .route("/projects/{id}/issues", get(list_page))
        .route("/projects/{id}/issues/live", get(live::stream))
        .route("/projects/{id}/issues/bulk", post(bulk))
        .route("/projects/{id}/issues/{issue_id}", get(issue_page))
        .route(
            "/projects/{id}/issues/{issue_id}/action",
            post(issue_action),
        )
        .route(
            "/projects/{id}/issues/{issue_id}/events/{event_id}",
            get(event_page),
        )
}

// ---------------------------------------------------------------------------
// Shared
// ---------------------------------------------------------------------------

async fn load_project(core: &Core, id: i64) -> ApiResult<Project> {
    core.with_conn(move |c| projects::get(c, id))
        .await?
        .ok_or(ApiError::NotFound)
}

/// Now as `YYYY-MM-DD HH:MM:SS` UTC.
pub fn now() -> String {
    normalize::sql_time(normalize::ts_seconds(jiff::Timestamp::now()))
}

/// The design system's status class for a level.
pub fn level_status(level: &str) -> &'static str {
    match level {
        "fatal" => "danger",
        "error" => "error",
        "warning" => "warning",
        "info" => "info",
        _ => "none",
    }
}

/// Translation key of a level's name.
pub fn level_key(level: &str) -> &'static str {
    match level {
        "fatal" => "level.fatal",
        "error" => "level.error",
        "warning" => "level.warning",
        "info" => "level.info",
        _ => "level.debug",
    }
}

/// A small badge next to an issue.
#[derive(Debug, Clone)]
pub struct Badge {
    pub label: &'static str,
    /// `badge-brand`, `status-warning` …
    pub class: &'static str,
    pub tip: &'static str,
}

/// The badges an issue gets: its status unless unresolved, new, regressed,
/// unsymbolicated.
pub fn badges(issue: &IssueRow, now: &str) -> Vec<Badge> {
    let mut out = Vec::new();
    match issue.status.as_str() {
        "resolved" => out.push(Badge {
            label: i18n::t("issue_status.resolved"),
            class: "status-success",
            tip: "",
        }),
        "ignored" => out.push(Badge {
            label: i18n::t("issue_status.ignored"),
            class: "status-paused",
            tip: "",
        }),
        _ => {
            let week_ago = jiff::civil::DateTime::strptime("%Y-%m-%d %H:%M:%S", now)
                .ok()
                .and_then(|d| d.checked_sub(jiff::Span::new().days(7)).ok())
                .map(|d| d.strftime("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_default();
            if issue.substatus == "new" && issue.first_seen.as_str() >= week_ago.as_str() {
                out.push(Badge {
                    label: i18n::t("issue_status.new"),
                    class: "badge-brand",
                    tip: i18n::t("issue_status.new_tip"),
                });
            } else if issue.substatus == "regressed" {
                out.push(Badge {
                    label: i18n::t("issue_status.regressed"),
                    class: "status-warning",
                    tip: i18n::t("issue_status.regressed_tip"),
                });
            }
        }
    }
    if issue.unsymbolicated {
        out.push(Badge {
            label: i18n::t("issue_status.unsymbolicated"),
            class: "",
            tip: i18n::t("issue_status.unsymbolicated_tip"),
        });
    }
    out
}

/// SVG points of a line through `series` in a `w`×`h` box, with a little
/// room at the top and bottom; and the polygon under it.
pub fn chart_points(series: &[i64], w: f64, h: f64) -> (String, String) {
    let n = series.len();
    if n == 0 {
        return (String::new(), String::new());
    }
    let max = series.iter().copied().max().unwrap_or(0).max(1) as f64;
    let step = if n > 1 { w / (n - 1) as f64 } else { 0.0 };
    let pts: Vec<String> = series
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = step * i as f64;
            let y = h - 1.0 - (*v as f64 / max) * (h - 3.0);
            format!("{x:.1},{y:.1}")
        })
        .collect();
    let line = pts.join(" ");
    let area = format!("{line} {w:.1},{h:.1} 0,{h:.1}");
    (line, area)
}

/// One issue in a list.
#[derive(Debug, Clone)]
pub struct IssueRowView {
    pub id: i64,
    pub href: String,
    pub title: String,
    pub culprit: String,
    pub level_status: &'static str,
    pub level_label: &'static str,
    pub badges: Vec<Badge>,
    pub events: i64,
    pub users: i64,
    /// Events in the period the overview's frequent list covers.
    pub recent: i64,
    pub first_seen: String,
    pub last_seen: String,
    pub spark: String,
    pub spark_label: String,
}

pub fn row_view(
    project_id: i64,
    i: &IssueRow,
    daily: Option<&Vec<i64>>,
    now: &str,
) -> IssueRowView {
    let (spark, spark_label) = match daily {
        Some(series) => (
            chart_points(series, 80.0, 24.0).0,
            i18n::tf(
                "issues.spark_label",
                &[("n", &i18n::fmt_int(series.iter().sum()))],
            ),
        ),
        None => (String::new(), String::new()),
    };
    IssueRowView {
        id: i.id,
        href: format!("/projects/{project_id}/issues/{}", i.id),
        title: i.title.clone(),
        culprit: i.culprit.clone(),
        level_status: level_status(&i.level),
        level_label: i18n::t(level_key(&i.level)),
        badges: badges(i, now),
        events: i.times_seen,
        users: i.users_seen,
        recent: 0,
        first_seen: i.first_seen.clone(),
        last_seen: i.last_seen.clone(),
        spark,
        spark_label,
    }
}

/// Paging links.
#[derive(Debug, Clone, Default)]
pub struct Paging {
    pub page: i64,
    pub pages: i64,
    pub first: i64,
    pub last: i64,
    pub total: i64,
    pub prev_href: String,
    pub next_href: String,
}

impl Paging {
    fn new(page: i64, size: i64, total: i64, href: impl Fn(i64) -> String) -> Paging {
        let pages = ((total + size - 1) / size).max(1);
        let page = page.clamp(1, pages);
        Paging {
            page,
            pages,
            first: if total == 0 { 0 } else { (page - 1) * size + 1 },
            last: (page * size).min(total),
            total,
            prev_href: if page > 1 {
                href(page - 1)
            } else {
                String::new()
            },
            next_href: if page < pages {
                href(page + 1)
            } else {
                String::new()
            },
        }
    }

    pub fn shown(&self) -> bool {
        self.pages > 1
    }
}

fn back_to(jar: SignedCookieJar, kind: &str, message: &str, to: &str) -> Response {
    (flash(jar, kind, message), Redirect::to(to)).into_response()
}

fn writer_error(e: crate::writer::WriterError) -> ApiError {
    ApiError::internal(e)
}

// ---------------------------------------------------------------------------
// The list
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListForm {
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub level: String,
    #[serde(default)]
    pub release: String,
    #[serde(default)]
    pub env: String,
    #[serde(default)]
    pub sort: String,
    #[serde(default)]
    pub page: Option<i64>,
    #[serde(default)]
    pub select: String,
    /// The status chip checked when the page last changed (filter requests).
    #[serde(default)]
    pub shown_status: String,
}

/// The list's filters as they apply.
#[derive(Debug, Clone)]
pub struct ListState {
    pub q: String,
    pub status: Status,
    /// The status came from an `is:` in the search.
    pub status_in_q: bool,
    pub level: String,
    pub release: String,
    pub env: String,
    pub sort: Sort,
    pub page: i64,
    pub select: bool,
}

impl ListState {
    pub fn from_form(f: &ListForm) -> (ListState, Query) {
        let q = f.q.trim().chars().take(500).collect::<String>();
        let mut query = search::parse(&q);
        let status_in_q = query.status.is_some();
        let status = query
            .status
            .or_else(|| Status::parse(&f.status))
            .unwrap_or(Status::Unresolved);
        let level = if normalize::LEVELS.contains(&f.level.as_str()) {
            f.level.clone()
        } else {
            String::new()
        };
        if !level.is_empty() {
            query.levels.push((level.clone(), false));
        }
        let release = f.release.trim().chars().take(200).collect::<String>();
        if !release.is_empty() {
            query.tags.push(TagFilter::new("release", &release, false));
        }
        let env = f.env.trim().chars().take(200).collect::<String>();
        if !env.is_empty() {
            query.tags.push(TagFilter::new("environment", &env, false));
        }
        let state = ListState {
            q,
            status,
            status_in_q,
            level,
            release,
            env,
            sort: Sort::parse(&f.sort).unwrap_or_default(),
            page: f.page.unwrap_or(1).max(1),
            select: !f.select.is_empty(),
        };
        (state, query)
    }

    /// The query string for this view on `page` (without `select`).
    pub fn qs(&self, page: i64) -> String {
        let mut s = url::form_urlencoded::Serializer::new(String::new());
        if !self.status_in_q {
            s.append_pair("status", self.status.as_str());
        }
        for (k, v) in [
            ("q", self.q.as_str()),
            ("level", self.level.as_str()),
            ("release", self.release.as_str()),
            ("env", self.env.as_str()),
        ] {
            if !v.is_empty() {
                s.append_pair(k, v);
            }
        }
        if self.sort != Sort::LastSeen {
            s.append_pair("sort", self.sort.as_str());
        }
        if page > 1 {
            s.append_pair("page", &page.to_string());
        }
        s.finish()
    }

    /// Filters besides the status chips and the search box.
    pub fn extra_filters(&self) -> usize {
        [
            !self.level.is_empty(),
            !self.release.is_empty(),
            !self.env.is_empty(),
            self.sort != Sort::LastSeen,
        ]
        .into_iter()
        .filter(|b| *b)
        .count()
    }

    pub fn is_filtered(&self) -> bool {
        !self.q.is_empty()
            || self.status != Status::Unresolved
            || !self.level.is_empty()
            || !self.release.is_empty()
            || !self.env.is_empty()
    }
}

/// Everything the results region shows.
pub struct ListData {
    pub project_id: i64,
    pub rows: Vec<IssueRowView>,
    pub paging: Paging,
    /// The query string without the page, for the live stream.
    pub qs: String,
    pub select: bool,
    pub can_edit: bool,
    pub filtered: bool,
    pub clear_href: String,
    pub unknown: String,
    /// The project has any issue at all.
    pub any: bool,
    pub status: Status,
    pub base: String,
}

impl ListData {
    pub fn select_href(&self) -> String {
        if self.qs.is_empty() {
            format!("{}?select=1", self.base)
        } else {
            format!("{}?{}&select=1", self.base, self.qs)
        }
    }

    pub fn exit_select_href(&self) -> String {
        if self.qs.is_empty() {
            self.base.clone()
        } else {
            format!("{}?{}", self.base, self.qs)
        }
    }

    pub fn empty_title(&self) -> &'static str {
        if self.filtered {
            i18n::t("issues.no_match_title")
        } else {
            match self.status {
                Status::Resolved => i18n::t("issues.none_resolved"),
                Status::Ignored => i18n::t("issues.none_ignored"),
                _ => i18n::t("issues.none_unresolved"),
            }
        }
    }
}

/// Loads one page of the list for `state`.
pub async fn load_list(
    core: &Core,
    project_id: i64,
    state: &ListState,
    query: Query,
    can_edit: bool,
) -> ApiResult<ListData> {
    let status = state.status.column();
    let sort = state.sort;
    let page = state.page;
    let (rows, total, any, daily) = core
        .with_conn(move |c| {
            let params = ListParams {
                project_id,
                status,
                query: &query,
                sort,
                limit: PAGE_SIZE,
                offset: (page - 1) * PAGE_SIZE,
            };
            let (mut rows, mut total) = triage::list(c, &params)?;
            if rows.is_empty() && total > 0 && page > 1 {
                let last = (total + PAGE_SIZE - 1) / PAGE_SIZE;
                let params = ListParams {
                    offset: (last - 1) * PAGE_SIZE,
                    ..params
                };
                (rows, total) = triage::list(c, &params)?;
            }
            let any: bool = c.query_row(
                "SELECT EXISTS (SELECT 1 FROM issues WHERE project_id = ?1)",
                [project_id],
                |r| r.get(0),
            )?;
            let today: String = c.query_row("SELECT date('now')", [], |r| r.get(0))?;
            let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
            let daily = triage::daily_events(c, &ids, &today, 14)?;
            Ok((rows, total, any, daily))
        })
        .await?;
    let base = format!("/projects/{project_id}/issues");
    let now = now();
    let paging = Paging::new(state.page, PAGE_SIZE, total, |p| {
        let qs = state.qs(p);
        if qs.is_empty() {
            base.clone()
        } else {
            format!("{base}?{qs}")
        }
    });
    let unknown = search::parse(&state.q).unknown.join(" ");
    Ok(ListData {
        project_id,
        rows: rows
            .iter()
            .map(|r| row_view(project_id, r, daily.get(&r.id), &now))
            .collect(),
        paging,
        qs: state.qs(state.page),
        select: state.select && can_edit,
        can_edit,
        filtered: state.is_filtered(),
        clear_href: base.clone(),
        unknown,
        any,
        status: state.status,
        base,
    })
}

/// A status chip.
pub struct StatusChip {
    pub value: &'static str,
    pub label: &'static str,
    pub checked: bool,
}

pub struct SortChoice {
    pub value: &'static str,
    pub label: &'static str,
    pub selected: bool,
}

pub struct LevelChoice {
    pub value: &'static str,
    pub label: &'static str,
}

#[derive(Template)]
#[template(path = "issues.html")]
struct ListTemplate {
    ctx: PageCtx,
    project: ProjectView,
    tab: &'static str,
    state: ListState,
    data: ListData,
    chips: Vec<StatusChip>,
    oob: bool,
    sorts: Vec<SortChoice>,
    levels: Vec<LevelChoice>,
    releases: Vec<String>,
    environments: Vec<String>,
}

/// The results region alone (filter changes), with the chips out of band.
#[derive(Template)]
#[template(path = "partials/issue_results.html")]
struct ResultsTemplate {
    data: ListData,
    state: ListState,
    chips: Vec<StatusChip>,
    oob: bool,
}

fn status_chips(current: Status) -> Vec<StatusChip> {
    Status::ALL
        .into_iter()
        .map(|s| StatusChip {
            value: s.as_str(),
            label: match s {
                Status::Unresolved => i18n::t("issues.status_unresolved"),
                Status::Resolved => i18n::t("issues.status_resolved"),
                Status::Ignored => i18n::t("issues.status_ignored"),
                Status::All => i18n::t("common.all"),
            },
            checked: s == current,
        })
        .collect()
}

/// The list alone (live updates).
#[derive(Template)]
#[template(path = "partials/issue_list.html")]
pub struct ListOnlyTemplate {
    pub data: ListData,
}

fn is_htmx_fragment(headers: &HeaderMap) -> bool {
    headers.contains_key("hx-request") && !headers.contains_key("hx-boosted")
}

async fn list_page(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    headers: HeaderMap,
    Path(id): Path<i64>,
    UrlQuery(form): UrlQuery<ListForm>,
) -> ApiResult<Response> {
    let project = load_project(&core, id).await?;
    let (state, query) = ListState::from_form(&form);
    // A status chip was picked while the search names a status: the chip
    // wins, so take the status out of the search and reload.
    let chip_changed = !form.shown_status.is_empty() && form.status != form.shown_status;
    if is_htmx_fragment(&headers) && chip_changed && state.status_in_q {
        let mut chip = state.clone();
        chip.q = search::without_status(&state.q);
        chip.status = Status::parse(&form.status).unwrap_or(Status::Unresolved);
        chip.status_in_q = false;
        chip.page = 1;
        let to = format!("/projects/{id}/issues?{}", chip.qs(1));
        let mut res = axum::http::StatusCode::OK.into_response();
        if let Ok(v) = HeaderValue::from_str(&to) {
            res.headers_mut().insert("hx-redirect", v);
        }
        return Ok(res);
    }
    let data = load_list(&core, id, &state, query, user.can_edit()).await?;
    if is_htmx_fragment(&headers) {
        let chips = status_chips(state.status);
        let mut res = HtmlTemplate(ResultsTemplate {
            data,
            state: state.clone(),
            chips,
            oob: true,
        })
        .into_response();
        let push = format!(
            "/projects/{id}/issues{}",
            match state.qs(state.page) {
                qs if qs.is_empty() => String::new(),
                qs => format!("?{qs}"),
            }
        );
        if let Ok(v) = HeaderValue::from_str(&push) {
            res.headers_mut().insert("hx-push-url", v);
        }
        return Ok(res);
    }
    let (releases, environments) = core
        .with_conn(move |c| Ok((triage::releases(c, id, 100)?, triage::environments(c, id)?)))
        .await?;
    let title = i18n::tf("issues.page_title", &[("name", &project.name)]);
    let (ctx, jar) = PageCtx::build(&core, Some(&user), &jar, title, "projects", "").await;
    let chips = status_chips(state.status);
    let sorts = Sort::ALL
        .into_iter()
        .map(|s| SortChoice {
            value: s.as_str(),
            label: i18n::t(s.label_key()),
            selected: s == state.sort,
        })
        .collect();
    let levels = normalize::LEVELS
        .iter()
        .map(|l| LevelChoice {
            value: l,
            label: i18n::t(level_key(l)),
        })
        .collect();
    let page = ListTemplate {
        ctx,
        project: ProjectView::from(&project),
        tab: "issues",
        state,
        data,
        chips,
        oob: false,
        sorts,
        levels,
        releases,
        environments,
    };
    Ok((jar, HtmlTemplate(page)).into_response())
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

/// What an editor does to issues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Resolve,
    ResolveNextRelease,
    Unresolve,
    Ignore(IgnoreFor),
    Merge,
    Delete,
}

/// An ignore condition as the forms send it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IgnoreFor {
    Forever,
    /// Until this many more events.
    Events(i64),
    /// For this many hours.
    Hours(i64),
}

impl Action {
    /// `resolve`, `resolve_next_release`, `unresolve`, `ignore`,
    /// `ignore_events_N` (1–1,000,000), `ignore_hours_N` (1–8760), `merge`,
    /// `delete`.
    pub fn parse(s: &str) -> Option<Action> {
        Some(match s {
            "resolve" => Action::Resolve,
            "resolve_next_release" => Action::ResolveNextRelease,
            "unresolve" => Action::Unresolve,
            "ignore" => Action::Ignore(IgnoreFor::Forever),
            "merge" => Action::Merge,
            "delete" => Action::Delete,
            other => {
                if let Some(n) = other.strip_prefix("ignore_events_") {
                    let n: i64 = n.parse().ok().filter(|n| (1..=1_000_000).contains(n))?;
                    Action::Ignore(IgnoreFor::Events(n))
                } else {
                    let n = other.strip_prefix("ignore_hours_")?;
                    let n: i64 = n.parse().ok().filter(|n| (1..=8760).contains(n))?;
                    Action::Ignore(IgnoreFor::Hours(n))
                }
            }
        })
    }
}

impl IgnoreFor {
    fn condition(self) -> Ignore {
        match self {
            IgnoreFor::Forever => Ignore::Forever,
            IgnoreFor::Events(n) => Ignore::Count(n),
            IgnoreFor::Hours(h) => Ignore::Until(
                jiff::Timestamp::now()
                    .checked_add(jiff::SignedDuration::from_hours(h))
                    .map(|t| normalize::sql_time(normalize::ts_seconds(t)))
                    .unwrap_or_else(|_| now()),
            ),
        }
    }
}

/// Runs `action` on `ids` of `project_id` as `actor`. Returns how many
/// issues it touched (for a merge: the issue left, as `Some`).
pub async fn apply(
    ingest: &Ingest,
    project_id: i64,
    ids: Vec<i64>,
    action: Action,
    actor: String,
) -> ApiResult<(usize, Option<i64>)> {
    let writer = ingest.writer();
    let action2 = action.clone();
    let result = writer
        .run(move |c| {
            let tx = c.transaction()?;
            let out = match action2 {
                Action::Resolve => (triage::resolve(&tx, project_id, &ids, false, &actor)?, None),
                Action::ResolveNextRelease => {
                    (triage::resolve(&tx, project_id, &ids, true, &actor)?, None)
                }
                Action::Unresolve => (triage::unresolve(&tx, project_id, &ids, &actor)?, None),
                Action::Ignore(how) => (
                    triage::ignore(&tx, project_id, &ids, &how.condition(), &actor)?,
                    None,
                ),
                Action::Merge => {
                    let left = triage::merge(&tx, project_id, &ids, &actor)?;
                    (if left.is_some() { ids.len() } else { 0 }, left)
                }
                Action::Delete => (triage::delete(&tx, project_id, &ids)?, None),
            };
            tx.commit()?;
            Ok(out)
        })
        .await
        .map_err(writer_error)?;
    writer.notify(project_id);
    Ok(result)
}

#[derive(Deserialize)]
struct BulkForm {
    #[serde(default)]
    action: String,
    #[serde(default)]
    ids: Vec<i64>,
    #[serde(default)]
    return_to: String,
}

/// Bulk resolve, unresolve, ignore and merge from the list's select mode.
async fn bulk(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    user: CurrentUser,
    jar: SignedCookieJar,
    Path(id): Path<i64>,
    Form(form): Form<BulkForm>,
) -> ApiResult<Response> {
    user.require_editor()?;
    load_project(&core, id).await?;
    let base = format!("/projects/{id}/issues");
    let back = if form.return_to.starts_with(&base)
        && !form.return_to.contains("//")
        && !form.return_to.contains('\\')
    {
        form.return_to.clone()
    } else {
        base.clone()
    };
    let action = match Action::parse(&form.action) {
        Some(Action::Delete) | None => {
            return Ok(back_to(jar, "error", i18n::t("issues.err_action"), &back));
        }
        Some(a) => a,
    };
    if form.ids.is_empty() {
        return Ok(back_to(jar, "error", i18n::t("issues.err_select"), &back));
    }
    if action == Action::Merge && form.ids.len() < 2 {
        return Ok(back_to(
            jar,
            "error",
            i18n::t("issues.err_merge_two"),
            &back,
        ));
    }
    let ids: Vec<i64> = form.ids.iter().copied().take(500).collect();
    let (n, left) = apply(&ingest, id, ids, action.clone(), user.username.clone()).await?;
    tracing::info!(user = %user.username, project = id, ?action, n, "issues changed");
    let n_i = i64::try_from(n).unwrap_or(i64::MAX);
    let message = match action {
        Action::Resolve => i18n::tn("issues.done_resolved", n_i),
        Action::ResolveNextRelease => i18n::tn("issues.done_resolved_next", n_i),
        Action::Unresolve => i18n::tn("issues.done_unresolved", n_i),
        Action::Ignore(_) => i18n::tn("issues.done_ignored", n_i),
        Action::Merge => i18n::tn("issues.done_merged", n_i),
        Action::Delete => String::new(),
    };
    if action == Action::Merge
        && let Some(left) = left
    {
        return Ok(back_to(
            jar,
            "success",
            &message,
            &format!("/projects/{id}/issues/{left}"),
        ));
    }
    Ok(back_to(jar, "success", &message, &back))
}

#[derive(Deserialize)]
struct ActionForm {
    #[serde(default)]
    action: String,
}

/// One issue's actions from its page.
async fn issue_action(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    user: CurrentUser,
    jar: SignedCookieJar,
    Path((id, issue_id)): Path<(i64, i64)>,
    axum::Form(form): axum::Form<ActionForm>,
) -> ApiResult<Response> {
    user.require_editor()?;
    let found = core
        .with_conn(move |c| triage::get(c, id, issue_id))
        .await?
        .ok_or(ApiError::NotFound)?;
    let here = format!("/projects/{id}/issues/{issue_id}");
    let Some(action) = Action::parse(&form.action).filter(|a| *a != Action::Merge) else {
        return Ok(back_to(jar, "error", i18n::t("issues.err_action"), &here));
    };
    apply(
        &ingest,
        id,
        vec![issue_id],
        action.clone(),
        user.username.clone(),
    )
    .await?;
    tracing::info!(user = %user.username, project = id, issue = issue_id, ?action, "issue changed");
    let message = match action {
        Action::Resolve => i18n::t("issue.done_resolved"),
        Action::ResolveNextRelease => i18n::t("issue.done_resolved_next"),
        Action::Unresolve => i18n::t("issue.done_unresolved"),
        Action::Ignore(_) => i18n::t("issue.done_ignored"),
        Action::Delete => {
            tracing::info!(title = %found.title, "issue deleted");
            return Ok(back_to(
                jar,
                "success",
                i18n::t("issue.deleted"),
                &format!("/projects/{id}/issues"),
            ));
        }
        Action::Merge => "",
    };
    Ok(back_to(jar, "success", message, &here))
}

// ---------------------------------------------------------------------------
// One issue
// ---------------------------------------------------------------------------

/// The issue's header facts.
pub struct IssueView {
    pub id: i64,
    pub title: String,
    pub culprit: String,
    pub level: String,
    pub level_status: &'static str,
    pub level_label: &'static str,
    pub status: String,
    pub badges: Vec<Badge>,
    pub first_seen: String,
    pub last_seen: String,
    pub first_release: String,
    pub last_release: String,
    pub events: i64,
    pub users: i64,
    pub platform: String,
    pub grouping_config: String,
    /// What the status means now ("Ignored until 100 more events" …).
    pub status_note: String,
    pub resolved: bool,
    pub ignored: bool,
    pub href: String,
}

fn fmt_utc(time: &str) -> String {
    let parsed = jiff::civil::DateTime::strptime("%Y-%m-%d %H:%M:%S", time)
        .ok()
        .and_then(|dt| dt.to_zoned(jiff::tz::TimeZone::UTC).ok());
    match parsed {
        Some(z) => format!("{} UTC", i18n::fmt_datetime(&z)),
        None => time.to_string(),
    }
}

fn issue_view(project_id: i64, i: &IssueRow, now: &str) -> IssueView {
    let status_note = match i.status.as_str() {
        "resolved" => match &i.resolved_in_release {
            Some(r) => i18n::tf("issue.note_resolved_next", &[("release", r)]),
            None => i18n::t("issue.note_resolved").to_string(),
        },
        "ignored" => match (&i.ignore_until, i.ignore_until_count) {
            (Some(t), _) => i18n::tf("issue.note_ignored_until", &[("time", &fmt_utc(t))]),
            (None, Some(n)) => i18n::tn("issue.note_ignored_count", (n - i.times_seen).max(1)),
            (None, None) => i18n::t("issue.note_ignored").to_string(),
        },
        _ => String::new(),
    };
    IssueView {
        id: i.id,
        title: i.title.clone(),
        culprit: i.culprit.clone(),
        level: i.level.clone(),
        level_status: level_status(&i.level),
        level_label: i18n::t(level_key(&i.level)),
        status: i.status.clone(),
        badges: badges(i, now),
        first_seen: i.first_seen.clone(),
        last_seen: i.last_seen.clone(),
        first_release: i.first_release.clone().unwrap_or_default(),
        last_release: i.last_release.clone().unwrap_or_default(),
        events: i.times_seen,
        users: i.users_seen,
        platform: i.platform.clone(),
        grouping_config: i.grouping_config.clone(),
        status_note,
        resolved: i.status == "resolved",
        ignored: i.status == "ignored",
        href: format!("/projects/{project_id}/issues/{}", i.id),
    }
}

/// One event's detail, for the issue's latest event and the event page.
pub struct EventDetail {
    pub event_id: String,
    pub short_id: String,
    pub href: String,
    pub timestamp: String,
    pub received_at: String,
    pub release: String,
    pub environment: String,
    pub level_status: &'static str,
    pub level_label: &'static str,
    pub user: String,
    /// `None` once retention removed the details.
    pub view: Option<EventView>,
    pub all_frames: bool,
    pub frames_app_href: String,
    pub frames_all_href: String,
}

async fn load_payload(ingest: &Ingest, payload_id: i64) -> Option<Value> {
    let conn = ingest.store().get().await.ok()?;
    let data: Option<Vec<u8>> = conn
        .interact(move |c| {
            use rusqlite::OptionalExtension as _;
            c.query_row(
                "SELECT data FROM payloads WHERE id = ?1",
                [payload_id],
                |r| r.get(0),
            )
            .optional()
        })
        .await
        .ok()?
        .ok()?;
    let raw = crate::store::decompress(&data?, MAX_PAYLOAD).ok()?;
    serde_json::from_slice(&raw).ok()
}

async fn event_detail(
    ingest: &Ingest,
    project_id: i64,
    issue_id: i64,
    e: &EventRow,
    all_frames: bool,
    page_href: &str,
) -> EventDetail {
    let payload = match e.payload_id {
        Some(id) => load_payload(ingest, id).await,
        None => None,
    };
    let sep = if page_href.contains('?') { '&' } else { '?' };
    EventDetail {
        short_id: e.event_id.chars().take(8).collect(),
        href: format!(
            "/projects/{project_id}/issues/{issue_id}/events/{}",
            e.event_id
        ),
        event_id: e.event_id.clone(),
        timestamp: e.timestamp.clone(),
        received_at: e.received_at.clone(),
        release: e.release.clone().unwrap_or_default(),
        environment: e.environment.clone().unwrap_or_default(),
        level_status: level_status(&e.level),
        level_label: i18n::t(level_key(&e.level)),
        user: e.user_hash.clone().unwrap_or_default(),
        view: payload.map(|p| view::build(&p, all_frames)),
        all_frames,
        frames_app_href: format!("{page_href}#stack"),
        frames_all_href: format!("{page_href}{sep}frames=all#stack"),
    }
}

/// A row of the events tab.
pub struct EventListRow {
    pub href: String,
    pub short_id: String,
    pub timestamp: String,
    pub release: String,
    pub environment: String,
    pub level_status: &'static str,
    pub level_label: &'static str,
    pub user: String,
    pub has_details: bool,
}

/// A tag key with its top values.
pub struct TagView {
    pub key: String,
    pub distinct: i64,
    pub values: Vec<TagValueView>,
    pub more: i64,
}

pub struct TagValueView {
    pub value: String,
    pub count: i64,
    pub pct: String,
    pub last_seen: String,
}

fn tag_views(tags: Vec<triage::TagSummary>) -> Vec<TagView> {
    tags.into_iter()
        .map(|t| {
            let shown = t.top.len() as i64;
            TagView {
                values: t
                    .top
                    .into_iter()
                    .map(|(value, count, last_seen)| TagValueView {
                        pct: i18n::fmt_pct2(count as f64 * 100.0 / t.total.max(1) as f64)
                            .replace(".00", "")
                            .replace(",00", ""),
                        value,
                        count,
                        last_seen,
                    })
                    .collect(),
                more: (t.distinct - shown).max(0),
                distinct: t.distinct,
                key: t.key,
            }
        })
        .collect()
}

/// One line of the activity log.
pub struct ActivityView {
    pub icon: &'static str,
    pub status: &'static str,
    pub text: String,
    pub time: String,
}

fn activity_views(issue: &IssueRow, log: Vec<triage::Activity>) -> Vec<ActivityView> {
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mut out: Vec<ActivityView> = log
        .into_iter()
        .map(|a| {
            let actor = a.actor.clone().unwrap_or_default();
            let (icon, status, text) = match a.kind.as_str() {
                "regressed" => (
                    "refresh",
                    "warning",
                    match s(&a.data, "release") {
                        r if r.is_empty() => i18n::t("activity.regressed").to_string(),
                        r => i18n::tf("activity.regressed_in", &[("release", &r)]),
                    },
                ),
                "resolved" => (
                    "check-circle",
                    "success",
                    i18n::tf("activity.resolved", &[("actor", &actor)]),
                ),
                "resolved_in_release" => (
                    "check-circle",
                    "success",
                    i18n::tf(
                        "activity.resolved_next",
                        &[("actor", &actor), ("release", &s(&a.data, "release"))],
                    ),
                ),
                "unresolved" => (
                    "circle-dashed",
                    "info",
                    i18n::tf("activity.unresolved", &[("actor", &actor)]),
                ),
                "ignored" => (
                    "eye-off",
                    "paused",
                    if let Some(n) = a.data.get("count").and_then(Value::as_i64) {
                        i18n::tnf("activity.ignored_count", n, &[("actor", &actor)])
                    } else if let Some(t) = a.data.get("until").and_then(Value::as_str) {
                        i18n::tf(
                            "activity.ignored_until",
                            &[("actor", &actor), ("time", &fmt_utc(t))],
                        )
                    } else {
                        i18n::tf("activity.ignored", &[("actor", &actor)])
                    },
                ),
                "unignored" => ("eye", "info", i18n::t("activity.unignored").to_string()),
                "merged" => {
                    let n = a
                        .data
                        .get("issues")
                        .and_then(Value::as_array)
                        .map_or(0, |v| v.len() as i64);
                    (
                        "link",
                        "info",
                        i18n::tnf("activity.merged", n, &[("actor", &actor)]),
                    )
                }
                _ => ("info", "none", a.kind.clone()),
            };
            ActivityView {
                icon,
                status,
                text,
                time: a.created_at,
            }
        })
        .collect();
    out.push(ActivityView {
        icon: "alert-circle",
        status: "error",
        text: match &issue.first_release {
            Some(r) => i18n::tf("activity.first_seen_in", &[("release", r)]),
            None => i18n::t("activity.first_seen").to_string(),
        },
        time: issue.first_seen.clone(),
    });
    out
}

/// One of the issue page's tabs.
pub struct TabLink {
    pub id: &'static str,
    pub label: String,
    pub href: String,
}

#[derive(Template)]
#[template(path = "issue.html")]
struct IssueTemplate {
    ctx: PageCtx,
    issue: IssueView,
    tab: &'static str,
    tabs: Vec<TabLink>,
    back_href: String,
    event: Option<EventDetail>,
    events: Vec<EventListRow>,
    paging: Paging,
    tags: Vec<TagView>,
    side_tags: Vec<TagView>,
    groupings: Vec<GroupingView>,
    activity: Vec<ActivityView>,
    has_release: bool,
}

#[derive(Deserialize, Default)]
struct IssueParams {
    #[serde(default)]
    tab: String,
    #[serde(default)]
    frames: String,
    #[serde(default)]
    page: Option<i64>,
}

fn event_list_row(project_id: i64, issue_id: i64, e: &EventRow) -> EventListRow {
    EventListRow {
        href: format!(
            "/projects/{project_id}/issues/{issue_id}/events/{}",
            e.event_id
        ),
        short_id: e.event_id.chars().take(8).collect(),
        timestamp: e.timestamp.clone(),
        release: e.release.clone().unwrap_or_default(),
        environment: e.environment.clone().unwrap_or_default(),
        level_status: level_status(&e.level),
        level_label: i18n::t(level_key(&e.level)),
        user: e
            .user_hash
            .as_deref()
            .map(|u| u.chars().take(12).collect())
            .unwrap_or_default(),
        has_details: e.payload_id.is_some(),
    }
}

async fn issue_page(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    user: CurrentUser,
    jar: SignedCookieJar,
    Path((id, issue_id)): Path<(i64, i64)>,
    UrlQuery(params): UrlQuery<IssueParams>,
) -> ApiResult<Response> {
    load_project(&core, id).await?;
    let issue = core
        .with_conn(move |c| triage::get(c, id, issue_id))
        .await?
        .ok_or(ApiError::NotFound)?;
    let tab = match params.tab.as_str() {
        "events" => "events",
        "tags" => "tags",
        "grouping" => "grouping",
        _ => "details",
    };
    let base = format!("/projects/{id}/issues/{issue_id}");
    let all_frames = params.frames == "all";
    let page = params.page.unwrap_or(1).max(1);
    let (latest, events, total, tags, hashes, log, has_release) = core
        .with_conn(move |c| {
            let latest = triage::latest_event(c, issue_id)?;
            let (events, total) = if tab == "events" {
                triage::events(c, issue_id, EVENTS_PAGE_SIZE, (page - 1) * EVENTS_PAGE_SIZE)?
            } else {
                let total: i64 = c.query_row(
                    "SELECT COUNT(*) FROM events WHERE issue_id = ?1",
                    [issue_id],
                    |r| r.get(0),
                )?;
                (Vec::new(), total)
            };
            let tags = triage::tags(c, issue_id, if tab == "tags" { 10 } else { 3 })?;
            let hashes = triage::hashes(c, issue_id)?;
            let log = triage::activity(c, issue_id)?;
            let has_release = triage::latest_release(c, id)?.is_some();
            Ok((latest, events, total, tags, hashes, log, has_release))
        })
        .await?;
    let event = match (&latest, tab) {
        (Some(e), "details") => {
            Some(event_detail(&ingest, id, issue_id, e, all_frames, &base).await)
        }
        _ => None,
    };
    let now = now();
    let tabs = vec![
        TabLink {
            id: "details",
            label: i18n::t("issue.tab_details").to_string(),
            href: base.clone(),
        },
        TabLink {
            id: "events",
            label: i18n::tf("issue.tab_events", &[("n", &i18n::fmt_int(total))]),
            href: format!("{base}?tab=events"),
        },
        TabLink {
            id: "tags",
            label: i18n::t("issue.tab_tags").to_string(),
            href: format!("{base}?tab=tags"),
        },
        TabLink {
            id: "grouping",
            label: i18n::t("issue.tab_grouping").to_string(),
            href: format!("{base}?tab=grouping"),
        },
    ];
    let paging = Paging::new(page, EVENTS_PAGE_SIZE, total, |p| {
        format!("{base}?tab=events&page={p}")
    });
    let tag_list = tag_views(tags);
    let side_tags: Vec<TagView> = if tab == "details" {
        tag_list
            .iter()
            .filter(|t| {
                matches!(
                    t.key.as_str(),
                    "release" | "environment" | "os" | "browser" | "device" | "runtime"
                )
            })
            .map(|t| TagView {
                key: t.key.clone(),
                distinct: t.distinct,
                values: t
                    .values
                    .iter()
                    .map(|v| TagValueView {
                        value: v.value.clone(),
                        count: v.count,
                        pct: v.pct.clone(),
                        last_seen: v.last_seen.clone(),
                    })
                    .collect(),
                more: t.more,
            })
            .collect()
    } else {
        Vec::new()
    };
    let title = issue.title.clone();
    let (ctx, jar) = PageCtx::build(&core, Some(&user), &jar, title, "projects", "").await;
    let page = IssueTemplate {
        ctx,
        issue: issue_view(id, &issue, &now),
        tab,
        tabs,
        back_href: format!("/projects/{id}/issues"),
        event,
        events: events
            .iter()
            .map(|e| event_list_row(id, issue_id, e))
            .collect(),
        paging,
        tags: tag_list,
        side_tags,
        groupings: hashes
            .iter()
            .map(|h| view::grouping(&h.hash, &h.grouping_config, &h.components))
            .collect(),
        activity: activity_views(&issue, log),
        has_release,
    };
    Ok((jar, HtmlTemplate(page)).into_response())
}

// ---------------------------------------------------------------------------
// One event
// ---------------------------------------------------------------------------

#[derive(Template)]
#[template(path = "event.html")]
struct EventTemplate {
    ctx: PageCtx,
    issue: IssueView,
    back_href: String,
    ev: EventDetail,
    older_href: String,
    newer_href: String,
}

#[derive(Deserialize, Default)]
struct EventParams {
    #[serde(default)]
    frames: String,
}

async fn event_page(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    user: CurrentUser,
    jar: SignedCookieJar,
    Path((id, issue_id, event_id)): Path<(i64, i64, String)>,
    UrlQuery(params): UrlQuery<EventParams>,
) -> ApiResult<Response> {
    load_project(&core, id).await?;
    let event_id = normalize::event_id(&event_id).ok_or(ApiError::NotFound)?;
    let (issue, row, (older, newer)) = core
        .with_conn(move |c| {
            let Some(issue) = triage::get(c, id, issue_id)? else {
                return Ok(None);
            };
            let Some(row) = triage::event(c, issue_id, &event_id)? else {
                return Ok(None);
            };
            let near = triage::neighbours(c, issue_id, &row)?;
            Ok(Some((issue, row, near)))
        })
        .await?
        .ok_or(ApiError::NotFound)?;
    let base = format!("/projects/{id}/issues/{issue_id}");
    let here = format!("{base}/events/{}", row.event_id);
    let event = event_detail(&ingest, id, issue_id, &row, params.frames == "all", &here).await;
    let older_href = older
        .map(|e| format!("{base}/events/{e}"))
        .unwrap_or_default();
    let newer_href = newer
        .map(|e| format!("{base}/events/{e}"))
        .unwrap_or_default();
    let title = i18n::tf("event.page_title", &[("id", &event.short_id)]);
    let (ctx, jar) = PageCtx::build(&core, Some(&user), &jar, title, "projects", "").await;
    let page = EventTemplate {
        ctx,
        issue: issue_view(id, &issue, &now()),
        back_href: base,
        ev: event,
        older_href,
        newer_href,
    };
    Ok((jar, HtmlTemplate(page)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_parse() {
        assert_eq!(Action::parse("resolve"), Some(Action::Resolve));
        assert_eq!(
            Action::parse("ignore_events_100"),
            Some(Action::Ignore(IgnoreFor::Events(100)))
        );
        assert_eq!(
            Action::parse("ignore_hours_24"),
            Some(Action::Ignore(IgnoreFor::Hours(24)))
        );
        assert_eq!(Action::parse("ignore_hours_0"), None);
        assert_eq!(Action::parse("ignore_events_x"), None);
        assert_eq!(Action::parse("explode"), None);
    }

    #[test]
    fn list_state_from_the_form() {
        let (s, q) = ListState::from_form(&ListForm {
            q: "is:resolved boom".into(),
            status: "ignored".into(),
            level: "error".into(),
            release: "1.4".into(),
            env: "production".into(),
            sort: "events".into(),
            page: Some(3),
            select: String::new(),
            shown_status: String::new(),
        });
        assert_eq!(s.status, Status::Resolved, "the search wins");
        assert!(s.status_in_q);
        assert_eq!(q.levels, [("error".to_string(), false)]);
        assert_eq!(q.tags.len(), 2);
        assert_eq!(
            s.qs(2),
            "q=is%3Aresolved+boom&level=error&release=1.4&env=production&sort=events&page=2"
        );
        assert_eq!(s.extra_filters(), 4);
        let (s, _) = ListState::from_form(&ListForm::default());
        assert_eq!(s.status, Status::Unresolved);
        assert_eq!(s.qs(1), "status=unresolved");
        assert!(!s.is_filtered());
    }

    #[test]
    fn charts_and_paging() {
        let (line, area) = chart_points(&[0, 5, 10], 80.0, 24.0);
        assert_eq!(line, "0.0,23.0 40.0,12.5 80.0,2.0");
        assert!(area.ends_with("80.0,24.0 0,24.0"));
        let p = Paging::new(2, 25, 60, |p| format!("?page={p}"));
        assert_eq!((p.first, p.last, p.pages), (26, 50, 3));
        assert_eq!(
            (p.prev_href.as_str(), p.next_href.as_str()),
            ("?page=1", "?page=3")
        );
        let p = Paging::new(9, 25, 0, |p| format!("?page={p}"));
        assert_eq!((p.page, p.first, p.last), (1, 0, 0));
        assert!(!p.shown());
    }
}
