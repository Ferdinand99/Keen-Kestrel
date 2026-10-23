//! The project's Web tab (`/projects/{id}/web`, docs/design.md §5.3, §6):
//! visitors and pageviews over time, top pages and entry pages, referrers
//! and UTM, browsers, operating systems, devices and screen sizes, custom
//! events, a period (today, 7 or 30 days, or custom) and filters made by
//! clicking a value. Current visitors and, while the period includes today,
//! the numbers and the chart update live (server-sent events from
//! `…/web/live`). Before the first pageview the tab shows how to add k.js.
//! The overview's small summary ([`overview`]) and the switch in the
//! settings are here too.

use std::time::Duration;

use askama::Template;
use axum::extract::{Extension, Path, RawQuery, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use axum_extra::extract::cookie::SignedCookieJar;
use serde::Deserialize;
use vole_kit::auth::CurrentUser;
use vole_kit::components::LineChart;
use vole_kit::http::live;
use vole_kit::http::{ApiError, ApiResult, HtmlTemplate};
use vole_kit::i18n::{self, Tr as _};
use vole_kit::page::{PageCtx, flash};
use vole_kit::state::Core;

use super::live::{Change, LiveChanges};
use super::query::{self, Filters, Period, Point, Row, Totals};
use super::{Dim, day_number};
use crate::db::projects::{self as db, Project};
use crate::ingest::Ingest;
use crate::pages::ProjectView;

/// Values per list.
const TOP: i64 = 10;
/// Quiet time that merges a burst of changes into one update.
const COALESCE: Duration = Duration::from_secs(2);
/// Current visitors are sent again at least this often (people leave).
const TICK: Duration = Duration::from_secs(30);

pub fn router() -> Router<Core> {
    Router::new()
        .route("/projects/{id}/web", get(dashboard))
        .route("/projects/{id}/web/live", get(live))
        .route("/projects/{id}/web/settings", post(save_settings))
}

async fn load_project(core: &Core, id: i64) -> ApiResult<Project> {
    core.with_conn(move |c| db::get(c, id))
        .await?
        .ok_or(ApiError::NotFound)
}

pub fn today() -> i64 {
    day_number(jiff::Timestamp::now().as_second())
}

// ---------------------------------------------------------------------------
// What the page shows
// ---------------------------------------------------------------------------

/// The period and filters of a request.
#[derive(Debug, Clone)]
pub struct View {
    pub period: Period,
    /// `today`, `7d`, `30d`, `custom` …
    pub name: String,
    pub filters: Filters,
}

impl View {
    /// From the query string; a period that doesn't parse is the last 7 days.
    pub fn parse(query: &str, today: i64) -> View {
        View::try_parse(query, today).unwrap_or_else(|_| {
            let mut v = View::try_parse("period=7d", today).unwrap_or(View {
                period: Period::last(7, today),
                name: "7d".into(),
                filters: Filters::default(),
            });
            v.filters = View::filters_of(query);
            v
        })
    }

    fn filters_of(query: &str) -> Filters {
        let pairs: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();
        Filters::from_pairs(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
    }

    /// From the query string (`period`, `from`, `to` and filters); `Err`
    /// names what is wrong with the period.
    pub fn try_parse(query: &str, today: i64) -> Result<View, &'static str> {
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
        Ok(View {
            period,
            name,
            filters: View::filters_of(query),
        })
    }

    /// The query string for `filters` and this period (or `period`).
    fn qs_with(&self, filters: &Filters, period: Option<&str>) -> String {
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

/// One choice of period.
pub struct Choice {
    pub label: &'static str,
    pub href: String,
    pub current: bool,
}

/// An active filter, with the link that removes it.
pub struct FilterChip {
    pub label: String,
    pub href: String,
}

/// The numbers and the chart (also sent alone as live updates).
pub struct Summary {
    pub visitors: String,
    pub pageviews: String,
    pub per_visitor: String,
    pub events: String,
    pub has_events: bool,
    pub period_label: String,
    /// Visitors (line and area) and pageviews on one scale.
    pub chart: LineChart,
}

/// One value in a list, with a bar as wide as its share of the top value.
pub struct Bar {
    pub label: String,
    pub href: String,
    pub visitors: String,
    pub count: String,
    pub share: String,
    pub mono: bool,
}

/// One tab of a card.
pub struct Panel {
    pub id: &'static str,
    pub label: &'static str,
    pub count_label: &'static str,
    pub rows: Vec<Bar>,
    pub empty: &'static str,
}

pub struct Card {
    pub id: &'static str,
    pub title: &'static str,
    pub panels: Vec<Panel>,
}

/// How to add k.js to a site.
pub struct Setup {
    /// The `<script>` tag with an enabled key, if the project has one.
    pub snippet: Option<String>,
    pub stub: String,
    pub event_example: String,
    pub settings_href: String,
}

impl Setup {
    pub fn new(base: &str, key: Option<&str>, project_id: i64) -> Setup {
        Setup {
            snippet: key.map(|k| {
                format!("<script defer src=\"{base}/js/k.js\" data-key=\"{k}\"></script>")
            }),
            stub: "<script>window.kestrel = window.kestrel || function () { (kestrel.q = kestrel.q || []).push(arguments) }</script>".into(),
            event_example: "kestrel('event', 'Signup', { plan: 'pro' })".into(),
            settings_href: format!("/projects/{project_id}/settings#keys"),
        }
    }
}

/// A dimension's value as people read it.
pub fn value_label(dim: Dim, value: &str) -> String {
    match dim {
        Dim::Referrer if value.is_empty() => i18n::t("web.direct").to_string(),
        Dim::Device => match value {
            super::ua::DESKTOP => i18n::t("web.device_desktop").to_string(),
            super::ua::MOBILE => i18n::t("web.device_mobile").to_string(),
            super::ua::TABLET => i18n::t("web.device_tablet").to_string(),
            _ => i18n::t("common.unknown").to_string(),
        },
        Dim::Screen => screen_label(value),
        _ if value.is_empty() => i18n::t("common.unknown").to_string(),
        _ => value.to_string(),
    }
}

/// A screen bucket as a width range.
pub fn screen_label(code: &str) -> String {
    match code {
        "0-575" => "< 576 px".into(),
        "1920+" => "≥ 1920 px".into(),
        c => match c.split_once('-') {
            Some((a, b)) => format!("{a}–{b} px"),
            None => i18n::t("common.unknown").to_string(),
        },
    }
}

/// The name of a dimension (filters, the API's labels).
pub fn dim_label(dim: Dim) -> &'static str {
    i18n::t(match dim {
        Dim::Page => "web.dim_page",
        Dim::Entry => "web.dim_entry",
        Dim::Referrer => "web.dim_referrer",
        Dim::UtmSource => "web.dim_utm_source",
        Dim::UtmMedium => "web.dim_utm_medium",
        Dim::UtmCampaign => "web.dim_utm_campaign",
        Dim::Browser => "web.dim_browser",
        Dim::Os => "web.dim_os",
        Dim::Device => "web.dim_device",
        Dim::Screen => "web.dim_screen",
        Dim::Event => "web.dim_event",
    })
}

fn period_label(v: &View) -> String {
    match v.name.as_str() {
        "today" | "day" => i18n::t("web.period_today").to_string(),
        "yesterday" => i18n::t("web.period_yesterday").to_string(),
        "custom" => date_range(v.period),
        _ => i18n::fmt_last_days(v.period.days()),
    }
}

fn day_label(day: i64) -> String {
    super::day_string(day)
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

fn chart(p: Period, series: &[Point]) -> LineChart {
    let visitors: Vec<f64> = series.iter().map(|s| s.visitors as f64).collect();
    let pageviews: Vec<f64> = series.iter().map(|s| s.pageviews as f64).collect();
    let max = series
        .iter()
        .map(|s| s.visitors.max(s.pageviews))
        .max()
        .unwrap_or(0);
    let total = |values: &[f64]| i18n::fmt_int(values.iter().sum::<f64>() as i64);
    let label = i18n::tf(
        "web.chart_label",
        &[
            ("visitors", &total(&visitors)),
            ("pageviews", &total(&pageviews)),
            ("period", &date_range(p)),
        ],
    );
    let (start_label, end_label) = if p.hourly() {
        ("00:00".to_string(), "23:00".to_string())
    } else {
        (day_label(p.from), day_label(p.to))
    };
    LineChart::new(
        label,
        vec![
            (i18n::t("web.visitors").to_string(), visitors, String::new()),
            (
                i18n::t("web.pageviews").to_string(),
                pageviews,
                String::new(),
            ),
        ],
        true,
    )
    .scale(i18n::fmt_int(max), i18n::fmt_int(0))
    .axis(start_label, end_label)
}

fn summary(v: &View, totals: Totals, series: &[Point]) -> Summary {
    let per_visitor = if totals.visitors > 0 {
        i18n::fmt_dec(totals.pageviews as f64 / totals.visitors as f64, 1)
    } else {
        "–".into()
    };
    Summary {
        visitors: i18n::fmt_int(totals.visitors),
        pageviews: i18n::fmt_int(totals.pageviews),
        per_visitor,
        events: i18n::fmt_int(totals.events),
        has_events: totals.events > 0,
        period_label: period_label(v),
        chart: chart(v.period, series),
    }
}

/// What the page loads.
pub struct Data {
    pub totals: Totals,
    pub series: Vec<Point>,
    pub lists: Vec<(Dim, Vec<Row>)>,
    pub current: i64,
    /// With filters: the first day with raw events.
    pub raw_from: Option<i64>,
}

/// Loads the numbers for `v` (rollups, or raw events with filters) and,
/// with `lists`, the breakdowns.
pub async fn load(
    core: &Core,
    ingest: &Ingest,
    project_id: i64,
    v: &View,
    lists: bool,
) -> ApiResult<Data> {
    let (p, f) = (v.period, v.filters.clone());
    let store = ingest.store().get().await.map_err(ApiError::internal)?;
    let now = jiff::Timestamp::now().as_second();
    let filtered = !f.is_empty();
    let f2 = f.clone();
    let (current, raw) = store
        .interact(move |c| -> rusqlite::Result<_> {
            let current = query::current_visitors(c, project_id, now)?;
            if !filtered {
                return Ok((current, None));
            }
            let totals = query::raw_totals(c, project_id, p, &f2)?;
            let series = query::raw_series(c, project_id, p, &f2)?;
            let mut out = Vec::new();
            if lists {
                for dim in Dim::ALL {
                    out.push((dim, query::raw_breakdown(c, project_id, p, &f2, dim, TOP)?));
                }
            }
            let first = query::raw_first_day(c, project_id)?;
            Ok((current, Some((totals, series, out, first))))
        })
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)?;
    if let Some((totals, series, lists, raw_from)) = raw {
        return Ok(Data {
            totals,
            series,
            lists,
            current,
            raw_from,
        });
    }
    let (totals, series, lists) = core
        .with_conn(move |c| {
            let totals = query::totals(c, project_id, p)?;
            let series = query::series(c, project_id, p)?;
            let mut out = Vec::new();
            if lists {
                for dim in Dim::ALL {
                    out.push((dim, query::breakdown(c, project_id, p, dim, TOP)?));
                }
            }
            Ok((totals, series, out))
        })
        .await?;
    Ok(Data {
        totals,
        series,
        lists,
        current,
        raw_from: None,
    })
}

/// The top `limit` values of one dimension for `v` (the REST API and MCP).
pub async fn top(
    core: &Core,
    ingest: &Ingest,
    project_id: i64,
    v: &View,
    dim: Dim,
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

fn bars(project_id: i64, v: &View, dim: Dim, rows: &[Row]) -> Vec<Bar> {
    let top = rows.iter().map(|r| r.visitors).max().unwrap_or(0).max(1);
    rows.iter()
        .map(|r| {
            let filter = match dim {
                Dim::Entry => Some(Dim::Page),
                Dim::Event => None,
                d => Some(d),
            };
            let href = filter
                .filter(|d| v.filters.get(*d) != Some(r.value.as_str()))
                .map(|d| {
                    format!(
                        "/projects/{project_id}/web?{}",
                        v.qs_with(&v.filters.with(d, &r.value), None)
                    )
                })
                .unwrap_or_default();
            Bar {
                label: value_label(dim, &r.value),
                href,
                visitors: i18n::fmt_int(r.visitors),
                count: i18n::fmt_int(r.count),
                share: format!("{:.1}%", 100.0 * r.visitors as f64 / top as f64),
                mono: matches!(dim, Dim::Page | Dim::Entry),
            }
        })
        .collect()
}

fn cards(project_id: i64, v: &View, lists: &[(Dim, Vec<Row>)]) -> Vec<Card> {
    let rows = |dim: Dim| {
        lists
            .iter()
            .find(|(d, _)| *d == dim)
            .map(|(_, r)| bars(project_id, v, dim, r))
            .unwrap_or_default()
    };
    let panel = |id, label, count_label, dim| Panel {
        id,
        label: i18n::t(label),
        count_label: i18n::t(count_label),
        rows: rows(dim),
        empty: i18n::t("web.list_empty"),
    };
    vec![
        Card {
            id: "pages",
            title: i18n::t("web.card_pages"),
            panels: vec![
                panel("top-pages", "web.top_pages", "web.pageviews", Dim::Page),
                panel("entry-pages", "web.entry_pages", "web.entries", Dim::Entry),
            ],
        },
        Card {
            id: "sources",
            title: i18n::t("web.card_sources"),
            panels: vec![
                panel("referrers", "web.referrers", "web.pageviews", Dim::Referrer),
                panel(
                    "utm-source",
                    "web.utm_source",
                    "web.pageviews",
                    Dim::UtmSource,
                ),
                panel(
                    "utm-medium",
                    "web.utm_medium",
                    "web.pageviews",
                    Dim::UtmMedium,
                ),
                panel(
                    "utm-campaign",
                    "web.utm_campaign",
                    "web.pageviews",
                    Dim::UtmCampaign,
                ),
            ],
        },
        Card {
            id: "devices",
            title: i18n::t("web.card_devices"),
            panels: vec![
                panel("browsers", "web.browsers", "web.pageviews", Dim::Browser),
                panel("systems", "web.systems", "web.pageviews", Dim::Os),
                panel(
                    "device-classes",
                    "web.devices",
                    "web.pageviews",
                    Dim::Device,
                ),
                panel("screens", "web.screens", "web.pageviews", Dim::Screen),
            ],
        },
        Card {
            id: "events",
            title: i18n::t("web.card_events"),
            panels: vec![Panel {
                empty: i18n::t("web.events_empty"),
                ..panel(
                    "custom-events",
                    "web.custom_events",
                    "web.events",
                    Dim::Event,
                )
            }],
        },
    ]
}

#[derive(Template)]
#[template(path = "web.html")]
struct WebTemplate {
    ctx: PageCtx,
    project: ProjectView,
    tab: &'static str,
    has_data: bool,
    enabled: bool,
    setup: Setup,
    periods: Vec<Choice>,
    custom_current: bool,
    custom_label: String,
    custom_from: String,
    custom_to: String,
    max_day: String,
    hidden: Vec<(&'static str, String)>,
    filters: Vec<FilterChip>,
    clear_href: String,
    live_href: String,
    live: String,
    raw_note: String,
    s: Summary,
    cards: Vec<Card>,
}

#[derive(Template)]
#[template(path = "partials/web_summary.html")]
struct SummaryTemplate {
    s: Summary,
}

/// `GET /projects/{id}/web`.
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
    let v = View::parse(q.as_deref().unwrap_or(""), today);
    let (keys, enabled, has_rollups) = core
        .with_conn(move |c| {
            Ok((
                db::keys(c, id)?,
                db::web_analytics(c, id)?.unwrap_or(true),
                query::has_data(c, id)?,
            ))
        })
        .await?;
    let has_data = has_rollups || {
        let store = ingest.store().get().await.map_err(ApiError::internal)?;
        store
            .interact(move |c| query::has_raw(c, id))
            .await
            .map_err(ApiError::internal)?
            .map_err(ApiError::internal)?
    };
    let base = crate::dsn::public_base(core.base_url().await, &headers);
    let key = keys
        .iter()
        .find(|k| k.enabled)
        .map(|k| k.public_key.clone());
    let setup = Setup::new(&base, key.as_deref(), id);
    let data = if has_data {
        load(&core, &ingest, id, &v, true).await?
    } else {
        Data {
            totals: Totals::default(),
            series: Vec::new(),
            lists: Vec::new(),
            current: 0,
            raw_from: None,
        }
    };
    let href = |qs: String| format!("/projects/{id}/web?{qs}");
    let periods = [
        ("today", "web.period_today"),
        ("7d", "web.period_7d"),
        ("30d", "web.period_30d"),
    ]
    .into_iter()
    .map(|(name, label)| Choice {
        label: i18n::t(label),
        href: href(v.qs_with(&v.filters, Some(name))),
        current: v.name == name,
    })
    .collect();
    let filters = v
        .filters
        .values
        .iter()
        .map(|(dim, value)| FilterChip {
            label: format!("{}: {}", dim_label(*dim), value_label(*dim, value)),
            href: href(v.qs_with(&v.filters.without(*dim), None)),
        })
        .collect();
    let raw_note = match data.raw_from {
        Some(first) if !v.filters.is_empty() && first > v.period.from => {
            i18n::tf("web.raw_note", &[("day", &day_label(first))])
        }
        None if !v.filters.is_empty() && has_data => i18n::t("web.raw_note_none").to_string(),
        _ => String::new(),
    };
    let s = summary(&v, data.totals, &data.series);
    let title = i18n::tf("web.page_title", &[("name", &project.name)]);
    let (ctx, jar) = PageCtx::build(&core, Some(&user), &jar, title, "projects", "").await;
    let page = WebTemplate {
        ctx,
        project: ProjectView::from(&project),
        tab: "web",
        has_data,
        enabled,
        setup,
        periods,
        custom_current: !matches!(v.name.as_str(), "today" | "day" | "7d" | "30d"),
        custom_label: if v.name == "custom" {
            date_range(v.period)
        } else {
            i18n::t("web.period_custom").to_string()
        },
        custom_from: v.period.from_date(),
        custom_to: v.period.to_date(),
        max_day: super::day_string(today),
        hidden: v.filters.pairs(),
        filters,
        clear_href: href(v.qs_with(&Filters::default(), None)),
        live_href: format!("/projects/{id}/web/live?{}", v.qs()),
        live: i18n::fmt_int(data.current),
        raw_note,
        s,
        cards: cards(id, &v, &data.lists),
    };
    Ok((jar, HtmlTemplate(page)).into_response())
}

// ---------------------------------------------------------------------------
// Live updates
// ---------------------------------------------------------------------------

struct Live {
    core: Core,
    ingest: Ingest,
    project_id: i64,
    query: String,
    lang: i18n::Lang,
}

impl Live {
    async fn current(&self) -> Option<String> {
        let store = self.ingest.store().get().await.ok()?;
        let id = self.project_id;
        let now = jiff::Timestamp::now().as_second();
        let n = store
            .interact(move |c| query::current_visitors(c, id, now))
            .await
            .ok()?
            .ok()?;
        Some(i18n::sync_scope(self.lang, || i18n::fmt_int(n)))
    }

    async fn summary(&self) -> Option<String> {
        let today = today();
        let v = View::parse(&self.query, today);
        if !v.includes(today) {
            return None;
        }
        let data = i18n::scope(
            self.lang,
            load(&self.core, &self.ingest, self.project_id, &v, false),
        )
        .await
        .ok()?;
        i18n::sync_scope(self.lang, || {
            SummaryTemplate {
                s: summary(&v, data.totals, &data.series),
            }
            .render()
        })
        .ok()
    }
}

/// `GET /projects/{id}/web/live`: `web-live` events with the current
/// visitors, `web-updated` events with the numbers and the chart.
async fn live(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    _user: CurrentUser,
    Path(id): Path<i64>,
    RawQuery(q): RawQuery,
) -> ApiResult<Response> {
    load_project(&core, id).await?;
    let changes = LiveChanges::new(ingest.writer(), id, COALESCE, TICK);
    let state = Live {
        core,
        ingest,
        project_id: id,
        query: q.unwrap_or_default(),
        lang: i18n::current(),
    };
    Ok(live::sse(futures_util::stream::unfold(
        (changes, state),
        |(mut changes, st)| async move {
            loop {
                let event = match changes.next().await? {
                    Change::Rollup => match st.summary().await {
                        Some(html) => live::html_event("web-updated", &html),
                        None => continue,
                    },
                    Change::Events | Change::Tick => match st.current().await {
                        Some(n) => live::html_event("web-live", &n),
                        None => continue,
                    },
                };
                return Some((event, (changes, st)));
            }
        },
    )))
}

// ---------------------------------------------------------------------------
// Settings and the overview
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SettingsForm {
    #[serde(default)]
    enabled: bool,
}

/// `POST /projects/{id}/web/settings`: turns web analytics on or off.
async fn save_settings(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    Path(id): Path<i64>,
    Form(form): Form<SettingsForm>,
) -> ApiResult<Response> {
    user.require_editor()?;
    load_project(&core, id).await?;
    let on = form.enabled;
    core.with_conn(move |c| db::set_web_analytics(c, id, on))
        .await?;
    tracing::info!(user = %user.username, project = id, on, "web analytics switched");
    let jar = flash(jar, "success", i18n::t("web.settings_saved"));
    Ok((jar, Redirect::to(&format!("/projects/{id}/settings#web"))).into_response())
}

/// The overview's summary of the last 7 days.
pub struct Overview {
    pub href: String,
    pub visitors: String,
    pub pageviews: String,
    pub current: String,
}

/// The overview's web summary, when the project has web analytics.
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
    let v = View {
        period: Period::last(7, today()),
        name: "7d".into(),
        filters: Filters::default(),
    };
    let data = load(core, ingest, project_id, &v, false).await?;
    Ok(Some(Overview {
        href: format!("/projects/{project_id}/web"),
        visitors: i18n::fmt_int(data.totals.visitors),
        pageviews: i18n::fmt_int(data.totals.pageviews),
        current: i18n::fmt_int(data.current),
    }))
}
