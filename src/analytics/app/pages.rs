//! The project's App tab (`/projects/{id}/app`, docs/design.md §5.3, §6)
//! and the signal explorer (`/projects/{id}/app/signals`): daily, weekly
//! and monthly active users, sessions and signals over time, signal types,
//! version adoption, OS versions, device models, platforms and locales, a
//! filter bar (period, version, platform, signal type, test signals) and the
//! current users, live. Before the first signal the tab shows how to add
//! KestrelKit. The overview's app summary and the settings are here too.

use std::convert::Infallible;
use std::time::Duration;

use askama::Template;
use axum::extract::{Extension, Path, RawQuery, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use axum_extra::extract::cookie::SignedCookieJar;
use serde::Deserialize;
use tokio::sync::broadcast::{self, error::RecvError, error::TryRecvError};
use vole_kit::auth::CurrentUser;
use vole_kit::http::{ApiError, ApiResult, HtmlTemplate};
use vole_kit::i18n::{self, Tr as _};
use vole_kit::page::{PageCtx, flash};
use vole_kit::state::Core;

use super::AppDim;
use super::query::{self, Adoption, AppFilters, AppPoint, AppTotals, Payload};
use super::rollup::OTHER;
use crate::analytics::day_string;
use crate::analytics::pages::{Bar, Card, Choice, FilterChip, Panel, today};
use crate::analytics::query::{Period, Row};
use crate::db::projects::{self as db, Project};
use crate::ingest::Ingest;
use crate::pages::ProjectView;
use crate::writer::WebChange;

/// Values per list.
const TOP: i64 = 10;
/// Versions in the adoption chart.
const ADOPTION_TOP: i64 = 5;
/// Values per payload key in the explorer.
const PAYLOAD_TOP: usize = 10;
/// Signal types offered in the explorer's picker.
const TYPES_TOP: i64 = 100;
/// Choices in the filter bar's lists.
const CHOICES_TOP: i64 = 50;
/// Quiet time that merges a burst of changes into one update.
const COALESCE: Duration = Duration::from_secs(2);
/// Current users are sent again at least this often.
const TICK: Duration = Duration::from_secs(30);
/// The README with KestrelKit's setup.
pub const KESTRELKIT_README: &str = "https://github.com/Ferdinand99/KestrelKit#readme";
/// The Swift package's address.
pub const KESTRELKIT_URL: &str = "https://github.com/Ferdinand99/KestrelKit";

/// Line colours of the adoption chart (design-system tokens).
const COLORS: [&str; 5] = [
    "var(--brand)",
    "var(--pal-blue)",
    "var(--pal-green)",
    "var(--pal-orange)",
    "var(--pal-pink)",
];

pub fn router() -> Router<Core> {
    Router::new()
        .route("/projects/{id}/app", get(dashboard))
        .route("/projects/{id}/app/live", get(live))
        .route("/projects/{id}/app/signals", get(explorer))
        .route("/projects/{id}/app/settings", post(save_settings))
}

async fn load_project(core: &Core, id: i64) -> ApiResult<Project> {
    core.with_conn(move |c| db::get(c, id))
        .await?
        .ok_or(ApiError::NotFound)
}

// ---------------------------------------------------------------------------
// Period and filters
// ---------------------------------------------------------------------------

/// The period and filters of a request.
#[derive(Debug, Clone)]
pub struct AppView {
    pub period: Period,
    /// `today`, `7d`, `30d`, `custom` …
    pub name: String,
    pub filters: AppFilters,
}

impl AppView {
    /// From the query string; a period that doesn't parse is the last 7 days.
    pub fn parse(query: &str, today: i64) -> AppView {
        AppView::try_parse(query, today).unwrap_or_else(|_| AppView {
            period: Period::last(7, today),
            name: "7d".into(),
            filters: AppView::filters_of(query),
        })
    }

    fn filters_of(query: &str) -> AppFilters {
        let pairs: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();
        AppFilters::from_pairs(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
    }

    /// From the query string (`period`, `from`, `to` and filters); `Err`
    /// names what is wrong with the period.
    pub fn try_parse(query: &str, today: i64) -> Result<AppView, &'static str> {
        let pairs: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();
        let get = |k: &str| {
            pairs
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        let (period, name) = Period::parse(get("period"), get("from"), get("to"), today)?;
        Ok(AppView {
            period,
            name,
            filters: AppView::filters_of(query),
        })
    }

    /// The query string for `filters` and this period (or `period`).
    pub fn qs_with(&self, filters: &AppFilters, period: Option<&str>) -> String {
        let mut q = url::form_urlencoded::Serializer::new(String::new());
        match period {
            Some(p) => {
                q.append_pair("period", p);
            }
            None if self.name == "custom" => {
                q.append_pair("period", "custom");
                q.append_pair("from", &self.period.from_date());
                q.append_pair("to", &self.period.to_date());
            }
            None => {
                q.append_pair("period", &self.name);
            }
        }
        for (k, v) in filters.pairs() {
            q.append_pair(k, &v);
        }
        q.finish()
    }

    pub fn qs(&self) -> String {
        self.qs_with(&self.filters, None)
    }

    fn includes(&self, day: i64) -> bool {
        self.period.from <= day && day <= self.period.to
    }
}

// ---------------------------------------------------------------------------
// Labels and charts
// ---------------------------------------------------------------------------

fn day_label(day: i64) -> String {
    day_string(day)
        .parse::<jiff::civil::Date>()
        .map(i18n::fmt_day_month)
        .unwrap_or_default()
}

fn date_range(p: Period) -> String {
    if p.from == p.to {
        day_label(p.from)
    } else {
        format!("{} – {}", day_label(p.from), day_label(p.to))
    }
}

fn period_label(v: &AppView) -> String {
    match v.name.as_str() {
        "today" | "day" => i18n::t("web.period_today").to_string(),
        "yesterday" => i18n::t("web.period_yesterday").to_string(),
        "custom" => date_range(v.period),
        _ => i18n::tf(
            "web.period_last_days",
            &[("n", &v.period.days().to_string())],
        ),
    }
}

/// A dimension's name (filters, the API's labels).
pub fn dim_label(dim: AppDim) -> &'static str {
    i18n::t(match dim {
        AppDim::Type => "app.dim_type",
        AppDim::Version => "app.dim_version",
        AppDim::Build => "app.dim_build",
        AppDim::Release => "app.dim_release",
        AppDim::Platform => "app.dim_platform",
        AppDim::Os => "app.dim_os",
        AppDim::Model => "app.dim_model",
        AppDim::Locale => "app.dim_locale",
    })
}

/// A value as people read it.
pub fn value_label(value: &str) -> String {
    if value.is_empty() {
        i18n::t("common.unknown").to_string()
    } else if value == OTHER {
        i18n::t("app.other_values").to_string()
    } else {
        value.to_string()
    }
}

fn points(values: &[i64], max: i64) -> String {
    let v: Vec<f64> = values.iter().map(|n| *n as f64).collect();
    crate::releases::scaled_points(&v, 0.0, max.max(1) as f64, 600.0, 120.0)
}

/// One line of a chart.
pub struct Line {
    pub points: String,
    pub color: &'static str,
    pub label: String,
    /// Shown next to the legend (adoption: the latest share).
    pub value: String,
}

/// A chart of one or more lines on one scale; the first may have an area.
pub struct Chart {
    pub lines: Vec<Line>,
    pub area: String,
    pub label: String,
    pub max_label: String,
    pub min_label: String,
    pub start_label: String,
    pub end_label: String,
}

fn axis(p: Period) -> (String, String) {
    if p.hourly() {
        ("00:00".to_string(), "23:00".to_string())
    } else {
        (day_label(p.from), day_label(p.to))
    }
}

/// Users: daily (with an area), and weekly and monthly on days.
fn users_chart(p: Period, series: &[AppPoint]) -> Chart {
    let users: Vec<i64> = series.iter().map(|s| s.users).collect();
    let wau: Vec<i64> = series.iter().map(|s| s.wau.unwrap_or(0)).collect();
    let mau: Vec<i64> = series.iter().map(|s| s.mau.unwrap_or(0)).collect();
    let windows = !p.hourly();
    let max = users
        .iter()
        .chain(if windows { mau.iter() } else { [].iter() })
        .chain(if windows { wau.iter() } else { [].iter() })
        .copied()
        .max()
        .unwrap_or(0);
    let first = points(&users, max);
    let mut lines = vec![Line {
        points: first.clone(),
        color: "var(--brand)",
        label: i18n::t(if windows {
            "app.dau"
        } else {
            "app.users_per_hour"
        })
        .to_string(),
        value: String::new(),
    }];
    if windows {
        lines.push(Line {
            points: points(&wau, max),
            color: "var(--pal-blue)",
            label: i18n::t("app.wau").to_string(),
            value: String::new(),
        });
        lines.push(Line {
            points: points(&mau, max),
            color: "var(--muted)",
            label: i18n::t("app.mau").to_string(),
            value: String::new(),
        });
    }
    let (start_label, end_label) = axis(p);
    Chart {
        area: format!("{first} 600.0,120.0 0.0,120.0"),
        lines,
        label: i18n::tf(
            "app.users_chart_label",
            &[("max", &i18n::fmt_int(max)), ("period", &date_range(p))],
        ),
        max_label: i18n::fmt_int(max),
        min_label: i18n::fmt_int(0),
        start_label,
        end_label,
    }
}

/// Sessions (with an area) and signals.
fn activity_chart(p: Period, series: &[AppPoint]) -> Chart {
    let sessions: Vec<i64> = series.iter().map(|s| s.sessions).collect();
    let signals: Vec<i64> = series.iter().map(|s| s.signals).collect();
    let max = sessions.iter().chain(&signals).copied().max().unwrap_or(0);
    let first = points(&sessions, max);
    let (start_label, end_label) = axis(p);
    Chart {
        area: format!("{first} 600.0,120.0 0.0,120.0"),
        lines: vec![
            Line {
                points: first,
                color: "var(--brand)",
                label: i18n::t("app.sessions").to_string(),
                value: String::new(),
            },
            Line {
                points: points(&signals, max),
                color: "var(--muted)",
                label: i18n::t("app.signals").to_string(),
                value: String::new(),
            },
        ],
        label: i18n::tf(
            "app.activity_chart_label",
            &[
                ("sessions", &i18n::fmt_int(sessions.iter().sum())),
                ("signals", &i18n::fmt_int(signals.iter().sum())),
                ("period", &date_range(p)),
            ],
        ),
        max_label: i18n::fmt_int(max),
        min_label: i18n::fmt_int(0),
        start_label,
        end_label,
    }
}

/// Each top version's share of the daily users, 0–100 %.
fn adoption_chart(p: Period, a: &Adoption) -> Option<Chart> {
    if p.hourly() || a.versions.is_empty() {
        return None;
    }
    let lines = a
        .versions
        .iter()
        .zip(COLORS)
        .map(|((version, shares, _), color)| Line {
            points: crate::releases::scaled_points(shares, 0.0, 100.0, 600.0, 120.0),
            color,
            label: value_label(version),
            value: i18n::fmt_pct2(shares.last().copied().unwrap_or(0.0)),
        })
        .collect();
    let (start_label, end_label) = axis(p);
    Some(Chart {
        area: String::new(),
        lines,
        label: i18n::tf("app.adoption_chart_label", &[("period", &date_range(p))]),
        max_label: i18n::fmt_pct2(100.0),
        min_label: i18n::fmt_pct2(0.0),
        start_label,
        end_label,
    })
}

/// A type's signals (with an area) and users.
fn type_chart(p: Period, series: &[AppPoint]) -> Chart {
    let signals: Vec<i64> = series.iter().map(|s| s.signals).collect();
    let users: Vec<i64> = series.iter().map(|s| s.users).collect();
    let max = signals.iter().chain(&users).copied().max().unwrap_or(0);
    let first = points(&signals, max);
    let (start_label, end_label) = axis(p);
    Chart {
        area: format!("{first} 600.0,120.0 0.0,120.0"),
        lines: vec![
            Line {
                points: first,
                color: "var(--brand)",
                label: i18n::t("app.signals").to_string(),
                value: String::new(),
            },
            Line {
                points: points(&users, max),
                color: "var(--muted)",
                label: i18n::t("app.users").to_string(),
                value: String::new(),
            },
        ],
        label: i18n::tf(
            "app.type_chart_label",
            &[
                ("signals", &i18n::fmt_int(signals.iter().sum())),
                ("period", &date_range(p)),
            ],
        ),
        max_label: i18n::fmt_int(max),
        min_label: i18n::fmt_int(0),
        start_label,
        end_label,
    }
}

/// The numbers and charts (also sent alone as live updates).
pub struct Summary {
    pub dau: String,
    pub wau: String,
    pub mau: String,
    pub sessions: String,
    pub signals: String,
    pub period_label: String,
    /// "On 22 Oct" (the period's last day, which DAU/WAU/MAU are of).
    pub day_meta: String,
    pub users_chart: Chart,
    pub activity_chart: Chart,
    pub strict: bool,
}

fn summary(v: &AppView, t: AppTotals, series: &[AppPoint], strict: bool) -> Summary {
    Summary {
        dau: i18n::fmt_int(t.dau),
        wau: i18n::fmt_int(t.wau),
        mau: i18n::fmt_int(t.mau),
        sessions: i18n::fmt_int(t.sessions),
        signals: i18n::fmt_int(t.signals),
        period_label: period_label(v),
        day_meta: i18n::tf("app.on_day", &[("day", &day_label(v.period.to))]),
        users_chart: users_chart(v.period, series),
        activity_chart: activity_chart(v.period, series),
        strict,
    }
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// What the dashboard loads.
pub struct Data {
    pub totals: AppTotals,
    pub series: Vec<AppPoint>,
    pub lists: Vec<(AppDim, Vec<Row>)>,
    pub adoption: Option<Adoption>,
    pub current: i64,
    /// With filters: the first day with raw signals.
    pub raw_from: Option<i64>,
    /// Test signals the rollups leave out (without the test filter).
    pub test_hidden: i64,
}

impl Data {
    fn empty() -> Data {
        Data {
            totals: AppTotals::default(),
            series: Vec::new(),
            lists: Vec::new(),
            adoption: None,
            current: 0,
            raw_from: None,
            test_hidden: 0,
        }
    }
}

/// Loads the numbers for `v` (rollups, or raw signals with filters) and,
/// with `lists`, the breakdowns and version adoption.
pub async fn load(
    core: &Core,
    ingest: &Ingest,
    project_id: i64,
    v: &AppView,
    lists: bool,
) -> ApiResult<Data> {
    let (p, f) = (v.period, v.filters.clone());
    let store = ingest.store().get().await.map_err(ApiError::internal)?;
    let now = jiff::Timestamp::now().as_second();
    let filtered = !f.is_empty();
    let f2 = f.clone();
    let (current, test_hidden, raw) = store
        .interact(move |c| -> rusqlite::Result<_> {
            let current = query::current_users(c, project_id, now, f2.include_test)?;
            let hidden = if f2.include_test {
                0
            } else {
                query::test_signals(c, project_id, p)?
            };
            if !filtered {
                return Ok((current, hidden, None));
            }
            let raw = query::raw_summary(
                c,
                project_id,
                p,
                &f2,
                lists,
                TOP as usize,
                ADOPTION_TOP as usize,
            )?;
            let (totals, series, out, adoption) = (raw.totals, raw.series, raw.lists, raw.adoption);
            let first = query::raw_first_day(c, project_id)?;
            Ok((
                current,
                hidden,
                Some((totals, series, out, adoption, first)),
            ))
        })
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)?;
    if let Some((totals, series, lists, adoption, raw_from)) = raw {
        return Ok(Data {
            totals,
            series,
            lists,
            adoption,
            current,
            raw_from,
            test_hidden,
        });
    }
    let (totals, series, lists, adoption) = core
        .with_conn(move |c| {
            let totals = query::totals(c, project_id, p)?;
            let series = query::series(c, project_id, p)?;
            let mut out = Vec::new();
            let mut adoption = None;
            if lists {
                for dim in AppDim::ALL {
                    out.push((dim, query::breakdown(c, project_id, p, dim, TOP)?));
                }
                adoption = Some(query::adoption(c, project_id, p, ADOPTION_TOP)?);
            }
            Ok((totals, series, out, adoption))
        })
        .await?;
    Ok(Data {
        totals,
        series,
        lists,
        adoption,
        current,
        raw_from: None,
        test_hidden,
    })
}

/// The top `limit` values of one dimension for `v` (the REST API and MCP).
pub async fn top(
    core: &Core,
    ingest: &Ingest,
    project_id: i64,
    v: &AppView,
    dim: AppDim,
    limit: i64,
) -> ApiResult<Vec<Row>> {
    let (p, f) = (v.period, v.filters.clone());
    if f.is_empty() {
        return core
            .with_conn(move |c| query::breakdown(c, project_id, p, dim, limit))
            .await;
    }
    let store = ingest.store().get().await.map_err(ApiError::internal)?;
    store
        .interact(move |c| query::raw_breakdown(c, project_id, p, &f, dim, limit))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)
}

/// Version adoption for `v` (the REST API).
pub async fn adoption(
    core: &Core,
    ingest: &Ingest,
    project_id: i64,
    v: &AppView,
    limit: i64,
) -> ApiResult<Adoption> {
    let (p, f) = (v.period, v.filters.clone());
    if f.is_empty() {
        return core
            .with_conn(move |c| query::adoption(c, project_id, p, limit))
            .await;
    }
    let store = ingest.store().get().await.map_err(ApiError::internal)?;
    store
        .interact(move |c| query::raw_adoption(c, project_id, p, &f, limit))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)
}

/// A signal type's payload keys and values for `v` (explorer, API).
pub async fn payload(
    core: &Core,
    ingest: &Ingest,
    project_id: i64,
    v: &AppView,
    kind: &str,
    limit: usize,
) -> ApiResult<Payload> {
    let (p, mut f) = (v.period, v.filters.clone());
    f.kind = None;
    let kind = kind.to_string();
    if f.is_empty() {
        return core
            .with_conn(move |c| query::payload(c, project_id, p, &kind, limit))
            .await;
    }
    let store = ingest.store().get().await.map_err(ApiError::internal)?;
    store
        .interact(move |c| query::raw_payload(c, project_id, p, &f, &kind, limit))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)
}

/// A signal type's signals and users over time for `v`.
pub async fn type_series(
    core: &Core,
    ingest: &Ingest,
    project_id: i64,
    v: &AppView,
    kind: &str,
) -> ApiResult<Vec<AppPoint>> {
    let (p, mut f) = (v.period, v.filters.clone());
    f.kind = None;
    let kind = kind.to_string();
    if f.is_empty() && !p.hourly() {
        return core
            .with_conn(move |c| query::type_series(c, project_id, p, &kind))
            .await;
    }
    f.kind = Some(kind);
    let store = ingest.store().get().await.map_err(ApiError::internal)?;
    store
        .interact(move |c| query::raw_series(c, project_id, p, &f))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)
}

/// Whether the project has app analytics (rollups or raw signals).
pub async fn has_data(core: &Core, ingest: &Ingest, project_id: i64) -> ApiResult<bool> {
    if core
        .with_conn(move |c| query::has_data(c, project_id))
        .await?
    {
        return Ok(true);
    }
    let store = ingest.store().get().await.map_err(ApiError::internal)?;
    store
        .interact(move |c| query::has_raw(c, project_id))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)
}

// ---------------------------------------------------------------------------
// The page
// ---------------------------------------------------------------------------

fn bars(project_id: i64, v: &AppView, dim: AppDim, rows: &[Row]) -> Vec<Bar> {
    let top = rows.iter().map(|r| r.visitors).max().unwrap_or(0).max(1);
    rows.iter()
        .map(|r| {
            let href = match dim {
                AppDim::Type => {
                    let mut f = v.filters.clone();
                    f.kind = Some(r.value.clone());
                    format!("/projects/{project_id}/app/signals?{}", v.qs_with(&f, None))
                }
                AppDim::Version if v.filters.version.as_deref() != Some(r.value.as_str()) => {
                    let mut f = v.filters.clone();
                    f.version = Some(r.value.clone());
                    format!("/projects/{project_id}/app?{}", v.qs_with(&f, None))
                }
                AppDim::Platform if v.filters.platform.as_deref() != Some(r.value.as_str()) => {
                    let mut f = v.filters.clone();
                    f.platform = Some(r.value.clone());
                    format!("/projects/{project_id}/app?{}", v.qs_with(&f, None))
                }
                _ => String::new(),
            };
            Bar {
                label: value_label(&r.value),
                href,
                visitors: i18n::fmt_int(r.visitors),
                count: i18n::fmt_int(r.count),
                share: format!("{:.1}%", 100.0 * r.visitors as f64 / top as f64),
                mono: matches!(dim, AppDim::Type | AppDim::Model | AppDim::Release),
            }
        })
        .collect()
}

fn cards(project_id: i64, v: &AppView, lists: &[(AppDim, Vec<Row>)]) -> Vec<Card> {
    let rows = |dim: AppDim| {
        lists
            .iter()
            .find(|(d, _)| *d == dim)
            .map(|(_, r)| bars(project_id, v, dim, r))
            .unwrap_or_default()
    };
    let panel = |id, label, dim| Panel {
        id,
        label: i18n::t(label),
        count_label: i18n::t("app.signals"),
        rows: rows(dim),
        empty: i18n::t("web.list_empty"),
    };
    vec![
        Card {
            id: "types",
            title: i18n::t("app.card_types"),
            panels: vec![Panel {
                empty: i18n::t("app.types_empty"),
                ..panel("signal-types", "app.top_types", AppDim::Type)
            }],
        },
        Card {
            id: "versions",
            title: i18n::t("app.card_versions"),
            panels: vec![
                panel("versions", "app.versions", AppDim::Version),
                panel("builds", "app.builds", AppDim::Build),
            ],
        },
        Card {
            id: "devices",
            title: i18n::t("app.card_devices"),
            panels: vec![
                panel("os-versions", "app.os_versions", AppDim::Os),
                panel("models", "app.models", AppDim::Model),
                panel("platforms", "app.platforms", AppDim::Platform),
            ],
        },
        Card {
            id: "locales",
            title: i18n::t("app.card_locales"),
            panels: vec![panel("locales", "app.locales", AppDim::Locale)],
        },
    ]
}

/// One choice of a filter's list.
pub struct SelectOption {
    pub value: String,
    pub label: String,
    pub selected: bool,
}

/// A filter's list: every value with users in the period, and the current
/// value even when it has none.
fn options(rows: &[Row], current: Option<&str>) -> Vec<SelectOption> {
    let mut out: Vec<SelectOption> = rows
        .iter()
        .map(|r| SelectOption {
            value: r.value.clone(),
            label: value_label(&r.value),
            selected: current == Some(r.value.as_str()),
        })
        .collect();
    if let Some(c) = current
        && !out.iter().any(|o| o.value == c)
    {
        out.insert(
            0,
            SelectOption {
                value: c.to_string(),
                label: value_label(c),
                selected: true,
            },
        );
    }
    out
}

/// The filter bar's lists.
pub struct FilterBar {
    pub versions: Vec<SelectOption>,
    pub platforms: Vec<SelectOption>,
    pub types: Vec<SelectOption>,
    pub include_test: bool,
    /// The period as hidden fields (the filter form).
    pub hidden: Vec<(&'static str, String)>,
    /// The filters as hidden fields (the custom period's form).
    pub filter_hidden: Vec<(&'static str, String)>,
}

async fn filter_bar(core: &Core, project_id: i64, v: &AppView) -> ApiResult<FilterBar> {
    let p = v.period;
    let (versions, platforms, types) = core
        .with_conn(move |c| {
            Ok((
                query::breakdown(c, project_id, p, AppDim::Version, CHOICES_TOP)?,
                query::breakdown(c, project_id, p, AppDim::Platform, CHOICES_TOP)?,
                query::breakdown(c, project_id, p, AppDim::Type, TYPES_TOP)?,
            ))
        })
        .await?;
    let f = &v.filters;
    let mut hidden = vec![("period", v.name.clone())];
    if v.name == "custom" {
        hidden.push(("from", v.period.from_date()));
        hidden.push(("to", v.period.to_date()));
    }
    Ok(FilterBar {
        versions: options(&versions, f.version.as_deref()),
        platforms: options(&platforms, f.platform.as_deref()),
        types: options(&types, f.kind.as_deref()),
        include_test: f.include_test,
        hidden,
        filter_hidden: f.pairs(),
    })
}

/// Chips of the active filters, each with the link that removes it.
fn chips(href: &dyn Fn(&AppFilters) -> String, f: &AppFilters, explorer: bool) -> Vec<FilterChip> {
    let mut out = Vec::new();
    let mut add = |label: String, without: AppFilters| {
        out.push(FilterChip {
            label,
            href: href(&without),
        });
    };
    if let Some(v) = &f.version {
        add(
            format!("{}: {v}", dim_label(AppDim::Version)),
            AppFilters {
                version: None,
                ..f.clone()
            },
        );
    }
    if let Some(v) = &f.platform {
        add(
            format!("{}: {v}", dim_label(AppDim::Platform)),
            AppFilters {
                platform: None,
                ..f.clone()
            },
        );
    }
    if !explorer && let Some(v) = &f.kind {
        add(
            format!("{}: {v}", dim_label(AppDim::Type)),
            AppFilters {
                kind: None,
                ..f.clone()
            },
        );
    }
    if f.include_test {
        add(
            i18n::t("app.with_test").to_string(),
            AppFilters {
                include_test: false,
                ..f.clone()
            },
        );
    }
    out
}

/// How to add KestrelKit to an app.
pub struct Setup {
    pub package_url: &'static str,
    /// `Kestrel.configure(url:key:)` with this instance and an enabled key.
    pub configure: Option<String>,
    pub signal_example: String,
    pub readme: &'static str,
    pub settings_href: String,
}

impl Setup {
    pub fn new(base: &str, key: Option<&str>, project_id: i64) -> Setup {
        Setup {
            package_url: KESTRELKIT_URL,
            configure: key.map(|k| {
                format!(
                    "import KestrelKit\n\nKestrel.configure(url: URL(string: \"{base}\")!, key: \"{k}\")"
                )
            }),
            signal_example: "Kestrel.signal(\"Paywall.shown\", payload: [\"plan\": \"pro\"])"
                .into(),
            readme: KESTRELKIT_README,
            settings_href: format!("/projects/{project_id}/settings#keys"),
        }
    }
}

/// One version in the adoption card.
pub struct AdoptionRow {
    pub label: String,
    /// The share on the period's last day, as people read it.
    pub share: String,
    /// The same as a CSS width.
    pub width: String,
    /// Users over the period (counted per day).
    pub users: String,
    pub color: &'static str,
}

/// The adoption card: the chart (periods of days) and the versions.
pub struct AdoptionView {
    pub chart: Option<Chart>,
    pub rows: Vec<AdoptionRow>,
}

fn adoption_view(p: Period, a: Option<&Adoption>) -> Option<AdoptionView> {
    let a = a?;
    if a.versions.is_empty() {
        return None;
    }
    Some(AdoptionView {
        chart: adoption_chart(p, a),
        rows: a
            .versions
            .iter()
            .zip(COLORS)
            .map(|((v, shares, users), color)| {
                let last = shares.last().copied().unwrap_or(0.0);
                AdoptionRow {
                    label: value_label(v),
                    share: i18n::fmt_pct2(last),
                    width: format!("{last:.1}%"),
                    users: i18n::fmt_int(*users),
                    color,
                }
            })
            .collect(),
    })
}

#[derive(Template)]
#[template(path = "app.html")]
struct AppTemplate {
    ctx: PageCtx,
    project: ProjectView,
    tab: &'static str,
    sub: &'static str,
    has_data: bool,
    enabled: bool,
    setup: Setup,
    periods: Vec<Choice>,
    custom_current: bool,
    custom_label: String,
    custom_from: String,
    custom_to: String,
    max_day: String,
    bar: FilterBar,
    form_action: String,
    filters: Vec<FilterChip>,
    clear_href: String,
    dashboard_href: String,
    explorer_href: String,
    live_href: String,
    live: String,
    raw_note: String,
    test_note: String,
    test_href: String,
    s: Summary,
    adoption: Option<AdoptionView>,
    cards: Vec<Card>,
}

#[derive(Template)]
#[template(path = "partials/app_summary.html")]
struct SummaryTemplate {
    s: Summary,
}

/// What the dashboard and the explorer share: the period chips, the filter
/// bar, the notes.
struct Frame {
    periods: Vec<Choice>,
    custom_current: bool,
    custom_label: String,
    filters: Vec<FilterChip>,
    clear_href: String,
    raw_note: String,
    test_note: String,
    test_href: String,
}

fn frame(base: &str, v: &AppView, data: &Data, explorer: bool) -> Frame {
    let href = |f: &AppFilters, period: Option<&str>| format!("{base}?{}", v.qs_with(f, period));
    let periods = [
        ("today", "web.period_today"),
        ("7d", "web.period_7d"),
        ("30d", "web.period_30d"),
    ]
    .into_iter()
    .map(|(name, label)| Choice {
        label: i18n::t(label),
        href: href(&v.filters, Some(name)),
        current: v.name == name,
    })
    .collect();
    let raw_note = match data.raw_from {
        Some(first) if !v.filters.is_empty() && first > v.period.from => {
            i18n::tf("app.raw_note", &[("day", &day_label(first))])
        }
        None if !v.filters.is_empty() => i18n::t("app.raw_note_none").to_string(),
        _ => String::new(),
    };
    let test_note = if data.test_hidden > 0 {
        i18n::tnf(
            "app.test_note",
            data.test_hidden,
            &[("n", &i18n::fmt_int(data.test_hidden))],
        )
    } else {
        String::new()
    };
    let mut with_test = v.filters.clone();
    with_test.include_test = true;
    let mut clear = AppFilters::default();
    if explorer {
        clear.kind = v.filters.kind.clone();
    }
    Frame {
        periods,
        custom_current: !matches!(v.name.as_str(), "today" | "day" | "7d" | "30d"),
        custom_label: if v.name == "custom" {
            date_range(v.period)
        } else {
            i18n::t("web.period_custom").to_string()
        },
        filters: chips(&|f| href(f, None), &v.filters, explorer),
        clear_href: href(&clear, None),
        raw_note,
        test_note,
        test_href: href(&with_test, None),
    }
}

async fn project_setup(
    core: &Core,
    headers: &HeaderMap,
    id: i64,
) -> ApiResult<(Setup, bool, bool)> {
    let (keys, enabled, strict) = core
        .with_conn(move |c| {
            let (on, strict) = db::app_analytics(c, id)?.unwrap_or((true, false));
            Ok((db::keys(c, id)?, on, strict))
        })
        .await?;
    let base = crate::dsn::public_base(core.base_url().await, headers);
    let key = keys
        .iter()
        .find(|k| k.enabled)
        .map(|k| k.public_key.clone());
    Ok((Setup::new(&base, key.as_deref(), id), enabled, strict))
}

/// `GET /projects/{id}/app`.
async fn dashboard(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    user: CurrentUser,
    jar: SignedCookieJar,
    headers: HeaderMap,
    Path(id): Path<i64>,
    RawQuery(q): RawQuery,
) -> ApiResult<Response> {
    let project = load_project(&core, id).await?;
    let today = today();
    let v = AppView::parse(q.as_deref().unwrap_or(""), today);
    let (setup, enabled, strict) = project_setup(&core, &headers, id).await?;
    let has_data = has_data(&core, &ingest, id).await?;
    let data = if has_data {
        load(&core, &ingest, id, &v, true).await?
    } else {
        Data::empty()
    };
    let bar = filter_bar(&core, id, &v).await?;
    let base = format!("/projects/{id}/app");
    let fr = frame(&base, &v, &data, false);
    let s = summary(&v, data.totals, &data.series, strict);
    let title = i18n::tf("app.page_title", &[("name", &project.name)]);
    let (ctx, jar) = PageCtx::build(&core, Some(&user), &jar, title, "projects", "").await;
    let page = AppTemplate {
        ctx,
        project: ProjectView::from(&project),
        tab: "app",
        sub: "dashboard",
        has_data,
        enabled,
        setup,
        periods: fr.periods,
        custom_current: fr.custom_current,
        custom_label: fr.custom_label,
        custom_from: v.period.from_date(),
        custom_to: v.period.to_date(),
        max_day: day_string(today),
        bar,
        form_action: base.clone(),
        filters: fr.filters,
        clear_href: fr.clear_href,
        dashboard_href: format!("{base}?{}", v.qs()),
        explorer_href: format!("/projects/{id}/app/signals?{}", v.qs()),
        live_href: format!("/projects/{id}/app/live?{}", v.qs()),
        live: i18n::fmt_int(data.current),
        raw_note: fr.raw_note,
        test_note: fr.test_note,
        test_href: fr.test_href,
        s,
        adoption: adoption_view(v.period, data.adoption.as_ref()),
        cards: cards(id, &v, &data.lists),
    };
    Ok((jar, HtmlTemplate(page)).into_response())
}

// ---------------------------------------------------------------------------
// The signal explorer
// ---------------------------------------------------------------------------

/// One payload key with its values.
pub struct KeyView {
    pub key: String,
    pub rows: Vec<Bar>,
}

#[derive(Template)]
#[template(path = "app_explorer.html")]
struct ExplorerTemplate {
    ctx: PageCtx,
    project: ProjectView,
    tab: &'static str,
    sub: &'static str,
    has_data: bool,
    enabled: bool,
    periods: Vec<Choice>,
    custom_current: bool,
    custom_label: String,
    custom_from: String,
    custom_to: String,
    max_day: String,
    bar: FilterBar,
    form_action: String,
    filters: Vec<FilterChip>,
    clear_href: String,
    dashboard_href: String,
    explorer_href: String,
    raw_note: String,
    test_note: String,
    test_href: String,
    /// The chosen type, if any.
    kind: String,
    signals: String,
    users: String,
    period_label: String,
    chart: Option<Chart>,
    keys: Vec<KeyView>,
    /// Without a type: every type, to pick one.
    types: Vec<Bar>,
}

fn key_view(key: String, rows: Vec<Row>) -> KeyView {
    let top = rows.iter().map(|r| r.count).max().unwrap_or(0).max(1);
    KeyView {
        key,
        rows: rows
            .iter()
            .map(|r| Bar {
                label: value_label(&r.value),
                href: String::new(),
                visitors: String::new(),
                count: i18n::fmt_int(r.count),
                share: format!("{:.1}%", 100.0 * r.count as f64 / top as f64),
                mono: false,
            })
            .collect(),
    }
}

/// `GET /projects/{id}/app/signals?type=`.
async fn explorer(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    user: CurrentUser,
    jar: SignedCookieJar,
    headers: HeaderMap,
    Path(id): Path<i64>,
    RawQuery(q): RawQuery,
) -> ApiResult<Response> {
    let project = load_project(&core, id).await?;
    let today = today();
    let v = AppView::parse(q.as_deref().unwrap_or(""), today);
    let (_, enabled, _) = project_setup(&core, &headers, id).await?;
    let has_data = has_data(&core, &ingest, id).await?;
    let kind = v.filters.kind.clone().unwrap_or_default();
    // The type's own numbers; the frame's notes about the other filters.
    let mut others = v.clone();
    others.filters.kind = None;
    let data = if has_data && !kind.is_empty() {
        let mut d = Data::empty();
        let series = type_series(&core, &ingest, id, &v, &kind).await?;
        let store = ingest.store().get().await.map_err(ApiError::internal)?;
        let (p, f) = (v.period, others.filters.clone());
        (d.raw_from, d.test_hidden) = store
            .interact(move |c| -> rusqlite::Result<_> {
                Ok((
                    query::raw_first_day(c, id)?,
                    if f.include_test {
                        0
                    } else {
                        query::test_signals(c, id, p)?
                    },
                ))
            })
            .await
            .map_err(ApiError::internal)?
            .map_err(ApiError::internal)?;
        d.series = series;
        d
    } else {
        Data::empty()
    };
    let keys = if has_data && !kind.is_empty() {
        payload(&core, &ingest, id, &v, &kind, PAYLOAD_TOP)
            .await?
            .into_iter()
            .map(|(k, rows)| key_view(k, rows))
            .collect()
    } else {
        Vec::new()
    };
    let types = if has_data && kind.is_empty() {
        let rows = top(&core, &ingest, id, &others, AppDim::Type, TYPES_TOP).await?;
        bars(id, &v, AppDim::Type, &rows)
    } else {
        Vec::new()
    };
    let bar = filter_bar(&core, id, &v).await?;
    let base = format!("/projects/{id}/app/signals");
    let mut fr = frame(&base, &v, &data, true);
    if others.filters.is_empty() {
        fr.raw_note = String::new();
    }
    let (signals, users): (i64, i64) = data
        .series
        .iter()
        .fold((0, 0), |(s, u), p| (s + p.signals, u + p.users));
    let title = if kind.is_empty() {
        i18n::tf("app.explorer_title", &[("name", &project.name)])
    } else {
        format!("{kind} · {}", project.name)
    };
    let (ctx, jar) = PageCtx::build(&core, Some(&user), &jar, title, "projects", "").await;
    let page = ExplorerTemplate {
        ctx,
        project: ProjectView::from(&project),
        tab: "app",
        sub: "explorer",
        has_data,
        enabled,
        periods: fr.periods,
        custom_current: fr.custom_current,
        custom_label: fr.custom_label,
        custom_from: v.period.from_date(),
        custom_to: v.period.to_date(),
        max_day: day_string(today),
        bar,
        form_action: base.clone(),
        filters: fr.filters,
        clear_href: fr.clear_href,
        dashboard_href: format!("/projects/{id}/app?{}", others.qs()),
        explorer_href: format!("{base}?{}", others.qs()),
        raw_note: fr.raw_note,
        test_note: fr.test_note,
        test_href: fr.test_href,
        signals: i18n::fmt_int(signals),
        users: i18n::fmt_int(users),
        period_label: period_label(&v),
        chart: (!kind.is_empty() && has_data).then(|| type_chart(v.period, &data.series)),
        kind,
        keys,
        types,
    };
    Ok((jar, HtmlTemplate(page)).into_response())
}

// ---------------------------------------------------------------------------
// Live updates
// ---------------------------------------------------------------------------

enum Change {
    Events,
    Rollup,
    Tick,
}

async fn next_change(rx: &mut broadcast::Receiver<WebChange>, project_id: i64) -> Option<Change> {
    let mut rollup = loop {
        match rx.recv().await {
            Ok(WebChange::Events(p)) if p == project_id => break false,
            Ok(WebChange::Rollup(p)) if p == project_id => break true,
            Ok(_) => continue,
            Err(RecvError::Lagged(_)) => break true,
            Err(RecvError::Closed) => return None,
        }
    };
    tokio::time::sleep(COALESCE).await;
    loop {
        match rx.try_recv() {
            Ok(WebChange::Rollup(p)) if p == project_id => rollup = true,
            Ok(_) => continue,
            Err(TryRecvError::Lagged(_)) => rollup = true,
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Closed) => return None,
        }
    }
    Some(if rollup {
        Change::Rollup
    } else {
        Change::Events
    })
}

struct Live {
    core: Core,
    ingest: Ingest,
    rx: broadcast::Receiver<WebChange>,
    project_id: i64,
    query: String,
    lang: i18n::Lang,
}

impl Live {
    async fn current(&self) -> Option<String> {
        let store = self.ingest.store().get().await.ok()?;
        let id = self.project_id;
        let now = jiff::Timestamp::now().as_second();
        let test = AppView::parse(&self.query, today()).filters.include_test;
        let n = store
            .interact(move |c| query::current_users(c, id, now, test))
            .await
            .ok()?
            .ok()?;
        Some(i18n::sync_scope(self.lang, || i18n::fmt_int(n)))
    }

    async fn summary(&self) -> Option<String> {
        let today = today();
        let v = AppView::parse(&self.query, today);
        if !v.includes(today) {
            return None;
        }
        let id = self.project_id;
        let strict = self
            .core
            .with_conn(move |c| db::app_analytics(c, id))
            .await
            .ok()
            .flatten()
            .is_some_and(|(_, s)| s);
        let data = i18n::scope(self.lang, load(&self.core, &self.ingest, id, &v, false))
            .await
            .ok()?;
        i18n::sync_scope(self.lang, || {
            SummaryTemplate {
                s: summary(&v, data.totals, &data.series, strict),
            }
            .render()
        })
        .ok()
    }
}

/// `GET /projects/{id}/app/live`: `app-live` events with the current users,
/// `app-updated` events with the numbers and the charts.
async fn live(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    _user: CurrentUser,
    Path(id): Path<i64>,
    RawQuery(q): RawQuery,
) -> ApiResult<Response> {
    load_project(&core, id).await?;
    let state = Live {
        rx: ingest.writer().subscribe_web(),
        core,
        ingest,
        project_id: id,
        query: q.unwrap_or_default(),
        lang: i18n::current(),
    };
    let events = futures_util::stream::unfold(state, |mut st| async move {
        loop {
            let change = tokio::select! {
                c = next_change(&mut st.rx, st.project_id) => c?,
                () = tokio::time::sleep(TICK) => Change::Tick,
            };
            let event = match change {
                Change::Rollup => match st.summary().await {
                    Some(html) => Event::default().event("app-updated").data(html),
                    None => continue,
                },
                Change::Events | Change::Tick => match st.current().await {
                    Some(n) => Event::default().event("app-live").data(n),
                    None => continue,
                },
            };
            return Some((Ok::<_, Infallible>(event), st));
        }
    });
    Ok(Sse::new(events)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response())
}

// ---------------------------------------------------------------------------
// Settings and the overview
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SettingsForm {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    strict: bool,
}

/// `POST /projects/{id}/app/settings`: app analytics on or off, strict mode.
async fn save_settings(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    Path(id): Path<i64>,
    Form(form): Form<SettingsForm>,
) -> ApiResult<Response> {
    user.require_editor()?;
    load_project(&core, id).await?;
    let (on, strict) = (form.enabled, form.strict);
    core.with_conn(move |c| db::set_app_analytics(c, id, on, strict))
        .await?;
    tracing::info!(user = %user.username, project = id, on, strict, "app analytics switched");
    let jar = flash(jar, "success", i18n::t("app.settings_saved"));
    Ok((jar, Redirect::to(&format!("/projects/{id}/settings#app"))).into_response())
}

/// The overview's app summary.
pub struct Overview {
    pub href: String,
    pub dau: String,
    pub wau: String,
    pub mau: String,
    pub current: String,
}

/// The overview's app summary, when the project has app analytics.
pub async fn overview(
    core: &Core,
    ingest: &Ingest,
    project_id: i64,
) -> ApiResult<Option<Overview>> {
    let has = core
        .with_conn(move |c| query::has_data(c, project_id))
        .await?;
    if !has {
        return Ok(None);
    }
    let v = AppView {
        period: Period::last(7, today()),
        name: "7d".into(),
        filters: AppFilters::default(),
    };
    let data = load(core, ingest, project_id, &v, false).await?;
    Ok(Some(Overview {
        href: format!("/projects/{project_id}/app"),
        dau: i18n::fmt_int(data.totals.dau),
        wau: i18n::fmt_int(data.totals.wau),
        mau: i18n::fmt_int(data.totals.mau),
        current: i18n::fmt_int(data.current),
    }))
}
