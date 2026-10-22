//! Releases (docs/design.md §4.9, §6): a project's releases with their
//! counts and health, one release's page (health over time, the issues that
//! started or came back in it, the debug files its crashes used), and the
//! release-health rollup ([`health`]). [`cli`] answers sentry-cli's release
//! commands.

pub mod cli;
pub mod health;

use askama::Template;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum_extra::extract::cookie::SignedCookieJar;
use serde::Deserialize;
use vole_kit::auth::CurrentUser;
use vole_kit::http::{ApiError, ApiResult, HtmlTemplate};
use vole_kit::i18n::{self, Tr as _};
use vole_kit::page::PageCtx;
use vole_kit::state::Core;

use crate::db::projects::{self, Project};
use crate::db::releases::{self as db, Release, ReleaseDif, ReleaseIssue};
use crate::issues::{Paging, chart_points, level_key, level_status};
use crate::pages::ProjectView;
use health::Health;

/// Releases per page of the list.
pub const PAGE_SIZE: i64 = 25;
/// Days the release page's charts cover.
pub const CHART_DAYS: i64 = 30;

pub fn router() -> Router<Core> {
    Router::new()
        .route("/projects/{id}/releases", get(list_page))
        .route("/projects/{id}/releases/find", get(find))
        .route("/projects/{id}/releases/{release_id}", get(release_page))
}

/// The link to a project's release by its version (resolved by
/// [`find`]), for the issue and event pages.
pub fn href(project_id: i64, version: &str) -> String {
    if version.is_empty() {
        return String::new();
    }
    let q = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("version", version)
        .finish();
    format!("/projects/{project_id}/releases/find?{q}")
}

async fn load_project(core: &Core, id: i64) -> ApiResult<Project> {
    core.with_conn(move |c| projects::get(c, id))
        .await?
        .ok_or(ApiError::NotFound)
}

/// One release in the list.
pub struct ReleaseRow {
    pub href: String,
    pub version: String,
    pub first_seen: String,
    pub last_seen: String,
    pub finalized: bool,
    pub new_issues: i64,
    pub events: i64,
    pub crash_free_sessions: Option<f64>,
    pub crash_free_users: Option<f64>,
    pub adoption: Option<f64>,
    /// Status class of the crash-free sessions rate.
    pub health_status: &'static str,
}

/// The design system's status for a crash-free rate.
pub fn health_status(rate: Option<f64>) -> &'static str {
    match rate {
        None => "none",
        Some(r) if r >= 99.0 => "success",
        Some(r) if r >= 95.0 => "warning",
        Some(_) => "error",
    }
}

fn row(project_id: i64, r: &Release) -> ReleaseRow {
    let cfs = r.health.crash_free_sessions();
    ReleaseRow {
        href: format!("/projects/{project_id}/releases/{}", r.id),
        version: r.version.clone(),
        first_seen: r.first_seen.clone(),
        last_seen: r.last_seen.clone(),
        finalized: r.date_released.is_some(),
        new_issues: r.new_issues,
        events: r.events,
        crash_free_sessions: cfs,
        crash_free_users: r.health.crash_free_users(),
        adoption: r.adoption,
        health_status: health_status(cfs),
    }
}

#[derive(Template)]
#[template(path = "releases.html")]
struct ListTemplate {
    ctx: PageCtx,
    project: ProjectView,
    tab: &'static str,
    rows: Vec<ReleaseRow>,
    paging: Paging,
    any_health: bool,
}

#[derive(Deserialize, Default)]
struct ListParams {
    #[serde(default)]
    page: Option<i64>,
}

async fn list_page(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    Path(id): Path<i64>,
    Query(params): Query<ListParams>,
) -> ApiResult<Response> {
    let project = load_project(&core, id).await?;
    let page = params.page.unwrap_or(1).max(1);
    let now = crate::issues::now();
    let (releases, total, any_health) = core
        .with_conn(move |c| {
            let (rows, total) = db::list(c, id, PAGE_SIZE, (page - 1) * PAGE_SIZE, &now)?;
            Ok((rows, total, health::any(c, id)?))
        })
        .await?;
    let base = format!("/projects/{id}/releases");
    let paging = Paging::new(page, PAGE_SIZE, total, |p| format!("{base}?page={p}"));
    let title = i18n::tf("releases.page_title", &[("name", &project.name)]);
    let (ctx, jar) = PageCtx::build(&core, Some(&user), &jar, title, "projects", "").await;
    let page = ListTemplate {
        ctx,
        project: ProjectView::from(&project),
        tab: "releases",
        rows: releases.iter().map(|r| row(id, r)).collect(),
        paging,
        any_health,
    };
    Ok((jar, HtmlTemplate(page)).into_response())
}

#[derive(Deserialize, Default)]
struct FindParams {
    #[serde(default)]
    version: String,
}

/// `…/releases/find?version=`: to the release's page.
async fn find(
    State(core): State<Core>,
    _user: CurrentUser,
    Path(id): Path<i64>,
    Query(params): Query<FindParams>,
) -> ApiResult<Response> {
    let version = params.version;
    let found = core
        .with_conn(move |c| db::id_of(c, id, &version))
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Redirect::to(&format!("/projects/{id}/releases/{found}")).into_response())
}

/// An issue on the release page.
pub struct IssueLink {
    pub href: String,
    pub title: String,
    pub culprit: String,
    pub level_status: &'static str,
    pub level_label: &'static str,
    pub events: i64,
    pub last_seen: String,
}

fn issue_link(project_id: i64, i: &ReleaseIssue) -> IssueLink {
    IssueLink {
        href: format!("/projects/{project_id}/issues/{}", i.id),
        title: i.title.clone(),
        culprit: i.culprit.clone(),
        level_status: level_status(&i.level),
        level_label: i18n::t(level_key(&i.level)),
        events: i.times_seen,
        last_seen: i.last_seen.clone(),
    }
}

/// A line chart of one series.
pub struct Chart {
    pub line: String,
    pub area: String,
    pub label: String,
    pub max_label: String,
    pub min_label: String,
}

/// The release page's numbers.
pub struct Stats {
    pub crash_free_sessions: Option<f64>,
    pub crash_free_users: Option<f64>,
    pub health_status: &'static str,
    pub sessions: i64,
    pub crashed: i64,
    pub errored: i64,
    pub abnormal: i64,
    pub users: i64,
    pub crashed_users: i64,
    pub adoption: Option<f64>,
}

#[derive(Template)]
#[template(path = "release.html")]
struct ReleaseTemplate {
    ctx: PageCtx,
    project: ProjectView,
    back_href: String,
    release: Release,
    stats: Stats,
    sessions_chart: Option<Chart>,
    crash_free_chart: Option<Chart>,
    new_issues: Vec<IssueLink>,
    regressions: Vec<IssueLink>,
    difs: Vec<ReleaseDif>,
    difs_href: String,
    issues_href: String,
}

/// SVG points of `values` on a scale from `min` (bottom) to `max` (top) in a
/// `w`×`h` box.
pub fn scaled_points(values: &[f64], min: f64, max: f64, w: f64, h: f64) -> String {
    let n = values.len();
    let span = (max - min).max(f64::EPSILON);
    let step = if n > 1 { w / (n - 1) as f64 } else { 0.0 };
    values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = step * i as f64;
            let y = h - 1.0 - ((v - min) / span).clamp(0.0, 1.0) * (h - 3.0);
            format!("{x:.1},{y:.1}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Charts of the sessions per day and the crash-free sessions per day.
fn charts(days: &[(String, Health)]) -> (Option<Chart>, Option<Chart>) {
    let sessions: Vec<i64> = days.iter().map(|(_, h)| h.sessions).collect();
    if sessions.iter().all(|n| *n == 0) {
        return (None, None);
    }
    let max = sessions.iter().copied().max().unwrap_or(0);
    let (line, area) = chart_points(&sessions, 600.0, 120.0);
    let total: i64 = sessions.iter().sum();
    let sessions_chart = Chart {
        line,
        area,
        label: i18n::tf(
            "release.sessions_chart_label",
            &[
                ("n", &i18n::fmt_int(total)),
                ("days", &CHART_DAYS.to_string()),
            ],
        ),
        max_label: i18n::fmt_int(max),
        min_label: i18n::fmt_int(0),
    };
    // Crash-free per day, on a scale from the lowest day (at most 90 %) to
    // 100 %; days without sessions repeat the day before.
    let mut last = 100.0;
    let rates: Vec<f64> = days
        .iter()
        .map(|(_, h)| {
            if let Some(r) = h.crash_free_sessions() {
                last = r;
            }
            last
        })
        .collect();
    let floor = rates.iter().copied().fold(90.0_f64, f64::min).floor();
    let line = scaled_points(&rates, floor, 100.0, 600.0, 120.0);
    let worst = rates.iter().copied().fold(100.0_f64, f64::min);
    let crash_free_chart = Chart {
        area: String::new(),
        line,
        label: i18n::tf(
            "release.crash_free_chart_label",
            &[("pct", &i18n::fmt_pct2(worst))],
        ),
        max_label: i18n::fmt_pct2(100.0),
        min_label: i18n::fmt_pct2(floor),
    };
    (Some(sessions_chart), Some(crash_free_chart))
}

async fn release_page(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    Path((id, release_id)): Path<(i64, i64)>,
) -> ApiResult<Response> {
    let project = load_project(&core, id).await?;
    let now = crate::issues::now();
    let (release, days, new_issues, regressions, difs) = core
        .with_conn(move |c| {
            let Some(r) = db::get(c, id, release_id, &now)? else {
                return Ok(None);
            };
            let (from, to): (String, String) = c.query_row(
                "SELECT date(?1, ?2), date(?1)",
                rusqlite::params![now, format!("-{} days", CHART_DAYS - 1)],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let days = health::daily(c, r.id, &from, &to)?;
            let new_issues = db::new_issues(c, r.id, 20)?;
            let regressions = db::regressions(c, id, &r.version, 20)?;
            let difs = db::debug_files(c, id, &r.version)?;
            Ok(Some((r, days, new_issues, regressions, difs)))
        })
        .await?
        .ok_or(ApiError::NotFound)?;
    let (sessions_chart, crash_free_chart) = charts(&days);
    let h = release.health;
    let stats = Stats {
        crash_free_sessions: h.crash_free_sessions(),
        crash_free_users: h.crash_free_users(),
        health_status: health_status(h.crash_free_sessions()),
        sessions: h.sessions,
        crashed: h.crashed,
        errored: (h.errored - h.crashed - h.abnormal).max(0),
        abnormal: h.abnormal,
        users: h.users,
        crashed_users: h.crashed_users,
        adoption: release.adoption,
    };
    let title = release.version.clone();
    let (ctx, jar) = PageCtx::build(&core, Some(&user), &jar, title, "projects", "").await;
    let q = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("status", "all")
        .append_pair("release", &release.version)
        .finish();
    let page = ReleaseTemplate {
        ctx,
        project: ProjectView::from(&project),
        back_href: format!("/projects/{id}/releases"),
        stats,
        sessions_chart,
        crash_free_chart,
        new_issues: new_issues.iter().map(|i| issue_link(id, i)).collect(),
        regressions: regressions.iter().map(|i| issue_link(id, i)).collect(),
        difs,
        difs_href: format!("/projects/{id}/debug-files"),
        issues_href: format!("/projects/{id}/issues?{q}"),
        release,
    };
    Ok((jar, HtmlTemplate(page)).into_response())
}
