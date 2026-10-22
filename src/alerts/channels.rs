//! `/settings/notifications`: the instance's notification channels (where
//! alerts go), with vole-kit's providers: the list, the add sheet (provider
//! picker, then the provider's form, rendered from its schema), the edit
//! page, test messages, pause and resume, delete. Editors and admins only.
//!
//! Secrets never leave the server: saved secrets render as empty inputs
//! with a "••••••" placeholder, an empty secret on save keeps the stored
//! value, and a changed destination needs the secrets typed again (the
//! kit's `merge_secrets_checked`). The provider form follows Vigilant Vole's
//! pages, which the kit doesn't share yet.

use askama::Template;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum_extra::extract::cookie::SignedCookieJar;
use serde::Deserialize;
use serde_json::{Value, json};
use vole_kit::auth::CurrentUser;
use vole_kit::http::{ApiError, ApiResult, HtmlTemplate};
use vole_kit::i18n::{self, Tr as _};
use vole_kit::notify::schema::{self, FieldError, FieldKind, ProviderSchema, SECRET_MASK};
use vole_kit::notify::{Category, PROVIDERS};
use vole_kit::page::{PageCtx, SettingsTab, TabAccess, flash};
use vole_kit::state::Core;

use super::Alerts;
use crate::db::alerts::{self as db, Channel};

/// The settings tab.
pub const TAB: SettingsTab = SettingsTab {
    id: "notifications",
    href: "/settings/notifications",
    label: "nav.notifications",
    icon: "bell",
    access: TabAccess::Editors,
};

pub fn router() -> Router<Core> {
    Router::new()
        .route("/settings/notifications", get(list_page).post(create))
        .route("/settings/notifications/providers", get(providers))
        .route("/settings/notifications/form", get(new_form))
        .route("/settings/notifications/test", post(test_unsaved))
        .route(
            "/settings/notifications/{id}",
            get(show).post(update).delete(remove),
        )
        .route("/settings/notifications/{id}/edit", get(edit_page))
        .route("/settings/notifications/{id}/test", post(test_saved))
        .route("/settings/notifications/{id}/active", post(toggle_active))
}

const NAME_MAX: usize = 100;

// ---------------------------------------------------------------------------
// View models
// ---------------------------------------------------------------------------

pub struct ChannelRow {
    pub id: i64,
    pub name: String,
    /// Provider kind, for the `provider-{kind}` tile.
    pub kind: String,
    pub kind_label: String,
    pub icon: &'static str,
    pub active: bool,
    pub rules_label: String,
}

impl ChannelRow {
    fn new(channel: Channel, rules: i64) -> Self {
        let schema = vole_kit::notify::provider(&channel.kind);
        ChannelRow {
            id: channel.id,
            kind_label: schema.map_or_else(|| channel.kind.clone(), |s| s.tr_label().to_string()),
            icon: schema.map_or("bell", |s| s.icon),
            kind: channel.kind,
            name: channel.name,
            active: channel.active,
            rules_label: rules_label(rules),
        }
    }
}

fn rules_label(n: i64) -> String {
    match n {
        0 => i18n::t("channels.no_rules").to_string(),
        n => i18n::tn("channels.rule_count", n),
    }
}

pub struct ProviderCard {
    pub kind: &'static str,
    pub label: &'static str,
    pub icon: &'static str,
    pub description: &'static str,
}

pub struct ProviderGroup {
    pub label: &'static str,
    pub id: String,
    pub providers: Vec<ProviderCard>,
}

pub struct OptionView {
    pub value: String,
    pub label: String,
    pub selected: bool,
}

pub struct FieldView {
    pub name: String,
    pub clear_name: String,
    pub label: &'static str,
    /// "text" | "url" | "password" | "number" | "select" | "bool" | "textarea" | "kv"
    pub input: &'static str,
    pub required: bool,
    /// A secret is stored and the input is empty: leaving it empty keeps it.
    pub saved: bool,
    pub placeholder: String,
    pub help: &'static str,
    pub value: String,
    pub checked: bool,
    pub options: Vec<OptionView>,
    pub rows: Vec<(String, String)>,
    pub error: String,
    pub show_when: String,
    pub autocomplete: &'static str,
}

impl FieldView {
    pub fn has_help(&self) -> bool {
        !self.help.is_empty() || self.saved
    }

    /// Ids for `aria-describedby` (help, error), space separated.
    pub fn describedby(&self) -> String {
        let mut ids = Vec::new();
        if self.has_help() {
            ids.push(format!("f-{}-help", self.name));
        }
        if !self.error.is_empty() {
            ids.push(format!("f-{}-error", self.name));
        }
        ids.join(" ")
    }
}

/// Everything the add and edit forms need.
pub struct ChannelForm {
    pub id: i64,
    pub kind: &'static str,
    pub label: &'static str,
    pub icon: &'static str,
    pub description: &'static str,
    pub docs_url: &'static str,
    pub name: String,
    pub name_error: String,
    pub active: bool,
    pub language: String,
    pub languages: Vec<OptionView>,
    pub rules_label: String,
    pub fields: Vec<FieldView>,
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

#[derive(Template)]
#[template(path = "settings_notifications.html")]
struct ListPage {
    ctx: PageCtx,
    rows: Vec<ChannelRow>,
    query: String,
    groups: Vec<ProviderGroup>,
}

#[derive(Template)]
#[template(path = "partials/notification_list.html")]
struct ListPartial {
    rows: Vec<ChannelRow>,
}

#[derive(Template)]
#[template(path = "partials/notification_picker.html")]
struct PickerPartial {
    query: String,
    groups: Vec<ProviderGroup>,
}

#[derive(Template)]
#[template(path = "partials/notification_provider_grid.html")]
struct GridPartial {
    query: String,
    groups: Vec<ProviderGroup>,
}

#[derive(Template)]
#[template(path = "partials/notification_form.html")]
struct FormPartial {
    form: ChannelForm,
}

#[derive(Template)]
#[template(path = "settings_notifications_edit.html")]
struct EditPage {
    ctx: PageCtx,
    form: ChannelForm,
    delete_url: String,
    delete_question: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn is_htmx(headers: &HeaderMap) -> bool {
    headers.contains_key("hx-request")
}

fn trigger(value: &Value) -> (HeaderName, HeaderValue) {
    let header = HeaderValue::from_str(&vole_kit::http::ascii_json(&value.to_string()))
        .unwrap_or_else(|_| HeaderValue::from_static("{}"));
    (HeaderName::from_static("hx-trigger"), header)
}

fn toast(message: &str, kind: &str) -> (HeaderName, HeaderValue) {
    (
        HeaderName::from_static("hx-trigger"),
        vole_kit::http::toast_trigger(message, kind, false),
    )
}

fn parse_pairs(body: &[u8]) -> Vec<(String, String)> {
    url::form_urlencoded::parse(body).into_owned().collect()
}

fn pair<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn pair_flag(pairs: &[(String, String)], key: &str) -> bool {
    matches!(pair(pairs, key), Some("true" | "on" | "1"))
}

/// A language code from a form, "" for the instance's.
fn language_of(pairs: &[(String, String)]) -> String {
    let code = pair(pairs, "language").unwrap_or_default().trim();
    if i18n::Lang::from_exact_code(code).is_some() {
        code.to_string()
    } else {
        String::new()
    }
}

fn language_options(selected: &str) -> Vec<OptionView> {
    let mut out = vec![OptionView {
        value: String::new(),
        label: i18n::tf(
            "channels.language_default",
            &[("name", i18n::site_default().native_name())],
        ),
        selected: selected.is_empty(),
    }];
    out.extend(
        i18n::language_choices()
            .into_iter()
            .map(|(code, label)| OptionView {
                selected: code == selected,
                value: code.to_string(),
                label,
            }),
    );
    out
}

fn schema_for(kind: &str) -> ApiResult<&'static ProviderSchema> {
    vole_kit::notify::provider(kind)
        .ok_or_else(|| ApiError::bad_request(i18n::t("notif.err_service")))
}

fn matches_query(p: &ProviderSchema, q: &str) -> bool {
    let q = q.trim().to_lowercase();
    q.is_empty()
        || p.label.to_lowercase().contains(&q)
        || p.kind.contains(&q)
        || p.description.to_lowercase().contains(&q)
        || p.tr_description().to_lowercase().contains(&q)
        || p.category.label().to_lowercase().contains(&q)
}

fn provider_groups(query: &str) -> Vec<ProviderGroup> {
    Category::ALL
        .iter()
        .filter_map(|cat| {
            let providers: Vec<ProviderCard> = PROVIDERS
                .iter()
                .filter(|p| p.category == *cat && matches_query(p, query))
                .map(|p| ProviderCard {
                    kind: p.kind,
                    label: p.tr_label(),
                    icon: p.icon,
                    description: p.tr_description(),
                })
                .collect();
            (!providers.is_empty()).then(|| ProviderGroup {
                label: cat.label(),
                id: format!("cat-{cat:?}").to_lowercase(),
                providers,
            })
        })
        .collect()
}

fn input_type(kind: FieldKind) -> &'static str {
    match kind {
        FieldKind::Text => "text",
        FieldKind::Url => "url",
        FieldKind::Secret => "password",
        FieldKind::Number => "number",
        FieldKind::Select(_) => "select",
        FieldKind::Bool => "bool",
        FieldKind::Textarea => "textarea",
        FieldKind::KeyValue => "kv",
    }
}

fn display_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn blank(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s.is_empty(),
        Some(Value::Object(o)) => o.is_empty(),
        _ => false,
    }
}

/// The provider's fields as the form shows them. `values`: what to show
/// (defaults when `fresh`, else what was submitted or the redacted stored
/// config, never raw stored secrets); `stored`: the saved config (`Null`
/// for a new channel), only to know whether a secret is saved.
pub fn field_views(
    schema: &ProviderSchema,
    values: &Value,
    stored: &Value,
    errors: &[FieldError],
    fresh: bool,
) -> Vec<FieldView> {
    schema
        .fields
        .iter()
        .map(|f| {
            let raw = values.get(f.key);
            let saved = f.secret && f.kind != FieldKind::KeyValue && !blank(stored.get(f.key));
            let default = f.default_value();
            let mut value = if fresh {
                default.to_string()
            } else {
                raw.map(display_value).unwrap_or_default()
            };
            if f.secret && value == SECRET_MASK {
                value.clear();
            }
            let options = match f.kind {
                FieldKind::Select(opts) => {
                    let chosen = if value.is_empty() {
                        default.as_ref()
                    } else {
                        value.as_str()
                    };
                    let chosen = if opts.iter().any(|(v, _)| *v == chosen) {
                        chosen
                    } else {
                        opts.first().map_or("", |o| o.0)
                    };
                    opts.iter()
                        .map(|(v, l)| OptionView {
                            value: (*v).to_string(),
                            label: f.tr_option(schema.kind, v, l).to_string(),
                            selected: *v == chosen,
                        })
                        .collect()
                }
                _ => Vec::new(),
            };
            let checked = if fresh {
                default == "true"
            } else {
                raw.is_some_and(|v| v.as_bool() == Some(true))
            };
            let mut rows: Vec<(String, String)> = match raw {
                Some(Value::Object(map)) if !fresh => map
                    .iter()
                    .map(|(k, v)| (k.clone(), display_value(v)))
                    .collect(),
                _ => Vec::new(),
            };
            if f.kind == FieldKind::KeyValue && rows.is_empty() {
                rows.push((String::new(), String::new()));
            }
            let show_when = if f.show_when.is_empty() {
                String::new()
            } else {
                format!("{}{}", schema::FIELD_PREFIX, f.show_when)
            };
            let placeholder = if saved {
                SECRET_MASK.to_string()
            } else {
                f.tr_placeholder(schema.kind).to_string()
            };
            FieldView {
                name: f.form_name(),
                clear_name: format!("{}__clear", f.form_name()),
                label: f.tr_label(schema.kind),
                input: input_type(f.kind),
                required: f.required,
                saved,
                placeholder,
                help: f.tr_help(schema.kind),
                value,
                checked,
                options,
                rows,
                error: errors
                    .iter()
                    .find(|(k, _)| k == f.key)
                    .map(|(_, m)| m.clone())
                    .unwrap_or_default(),
                show_when,
                autocomplete: if f.kind == FieldKind::Secret {
                    "new-password"
                } else {
                    "off"
                },
            }
        })
        .collect()
}

fn blank_form(schema: &'static ProviderSchema, language: &str) -> ChannelForm {
    ChannelForm {
        id: 0,
        kind: schema.kind,
        label: schema.tr_label(),
        icon: schema.icon,
        description: schema.tr_description(),
        docs_url: schema.docs_url,
        name: String::new(),
        name_error: String::new(),
        active: true,
        language: language.to_string(),
        languages: language_options(language),
        rules_label: String::new(),
        fields: field_views(schema, &Value::Null, &Value::Null, &[], true),
    }
}

fn validate_name(name: &str) -> Option<String> {
    if name.is_empty() {
        Some(i18n::t("notif.err_name").into())
    } else if name.chars().count() > NAME_MAX {
        Some(i18n::tf(
            "notif.err_name_long",
            &[("max", &NAME_MAX.to_string())],
        ))
    } else {
        None
    }
}

async fn list_rows(core: &Core) -> ApiResult<Vec<ChannelRow>> {
    let rows = core.with_conn(|c| db::channels_with_counts(c)).await?;
    Ok(rows
        .into_iter()
        .map(|(c, n)| ChannelRow::new(c, n))
        .collect())
}

async fn load_channel(core: &Core, id: i64) -> ApiResult<Channel> {
    core.with_conn(move |c| db::channel(c, id))
        .await?
        .ok_or(ApiError::NotFound)
}

/// The language the user reads (the request's), as a new channel's
/// default; "" when it is the instance's anyway.
fn user_language() -> String {
    let lang = i18n::current();
    if lang == i18n::site_default() {
        String::new()
    } else {
        lang.code().to_string()
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn list_page(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
) -> ApiResult<impl IntoResponse> {
    user.require_editor()?;
    let rows = list_rows(&core).await?;
    let (ctx, jar) = PageCtx::build(
        &core,
        Some(&user),
        &jar,
        i18n::t("nav.notifications"),
        "settings",
        "notifications",
    )
    .await;
    Ok((
        jar,
        HtmlTemplate(ListPage {
            ctx,
            rows,
            query: String::new(),
            groups: provider_groups(""),
        }),
    ))
}

#[derive(Deserialize)]
struct ProvidersQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    grid: bool,
}

async fn providers(user: CurrentUser, Query(q): Query<ProvidersQuery>) -> ApiResult<Response> {
    user.require_editor()?;
    let groups = provider_groups(&q.q);
    Ok(if q.grid {
        HtmlTemplate(GridPartial { query: q.q, groups }).into_response()
    } else {
        HtmlTemplate(PickerPartial { query: q.q, groups }).into_response()
    })
}

#[derive(Deserialize)]
struct KindQuery {
    #[serde(default)]
    kind: String,
}

async fn new_form(user: CurrentUser, Query(q): Query<KindQuery>) -> ApiResult<impl IntoResponse> {
    user.require_editor()?;
    let schema = schema_for(&q.kind)?;
    Ok(HtmlTemplate(FormPartial {
        form: blank_form(schema, &user_language()),
    }))
}

async fn create(
    State(core): State<Core>,
    user: CurrentUser,
    headers: HeaderMap,
    jar: SignedCookieJar,
    body: Bytes,
) -> ApiResult<Response> {
    user.require_editor()?;
    let pairs = parse_pairs(&body);
    let schema = schema_for(pair(&pairs, "kind").unwrap_or_default())?;
    let name = pair(&pairs, "name").unwrap_or_default().trim().to_string();
    let language = language_of(&pairs);
    let parsed = schema::parse_form(schema, &pairs);

    let name_error = validate_name(&name);
    let field_errors = schema::validate(schema, &parsed.config)
        .err()
        .unwrap_or_default();
    if name_error.is_some() || !field_errors.is_empty() {
        if !is_htmx(&headers) {
            let msg = name_error.unwrap_or_else(|| field_errors[0].1.clone());
            return Ok((
                flash(jar, "error", msg),
                Redirect::to("/settings/notifications"),
            )
                .into_response());
        }
        let mut form = blank_form(schema, &language);
        form.name = name;
        form.name_error = name_error.unwrap_or_default();
        // Echo what the user just typed (including secrets they typed now).
        form.fields = field_views(schema, &parsed.config, &Value::Null, &field_errors, false);
        return Ok(HtmlTemplate(FormPartial { form }).into_response());
    }

    let config = parsed.config.to_string();
    let kind = schema.kind;
    let saved_name = name.clone();
    core.with_conn(move |c| db::insert_channel(c, &saved_name, kind, &config, &language))
        .await?;
    tracing::info!(user = %user.username, kind, "notification channel created");

    let message = i18n::tf("notif.added", &[("name", &name)]);
    if !is_htmx(&headers) {
        return Ok((
            flash(jar, "success", message),
            Redirect::to("/settings/notifications"),
        )
            .into_response());
    }
    let rows = list_rows(&core).await?;
    let headers = [
        (
            HeaderName::from_static("hx-retarget"),
            HeaderValue::from_static("#channels"),
        ),
        (
            HeaderName::from_static("hx-reswap"),
            HeaderValue::from_static("outerHTML"),
        ),
        trigger(
            &json!({ "vv:toast": { "message": message, "kind": "success" }, "vv:close-sheet": true }),
        ),
    ];
    Ok((headers, HtmlTemplate(ListPartial { rows })).into_response())
}

async fn show(user: CurrentUser, Path(id): Path<i64>) -> ApiResult<Redirect> {
    user.require_editor()?;
    Ok(Redirect::to(&format!("/settings/notifications/{id}/edit")))
}

fn stored_config(channel: &Channel) -> Value {
    serde_json::from_str(&channel.config).unwrap_or_else(|_| json!({}))
}

fn edit_form(channel: &Channel, schema: &'static ProviderSchema, rules: i64) -> ChannelForm {
    let stored = stored_config(channel);
    let shown = schema::redact(Some(schema), &stored);
    ChannelForm {
        id: channel.id,
        name: channel.name.clone(),
        active: channel.active,
        rules_label: rules_label(rules),
        fields: field_views(schema, &shown, &stored, &[], false),
        ..blank_form(schema, &channel.language)
    }
}

async fn rules_using(core: &Core, id: i64) -> ApiResult<i64> {
    core.with_conn(move |c| {
        c.query_row(
            "SELECT COUNT(*) FROM alert_rule_channels WHERE channel_id = ?1",
            [id],
            |r| r.get(0),
        )
    })
    .await
}

async fn render_edit(
    core: &Core,
    user: &CurrentUser,
    jar: &SignedCookieJar,
    form: ChannelForm,
) -> (SignedCookieJar, HtmlTemplate<EditPage>) {
    let (ctx, jar) = PageCtx::build(
        core,
        Some(user),
        jar,
        form.name.clone(),
        "settings",
        "notifications",
    )
    .await;
    let delete_url = format!("/settings/notifications/{}", form.id);
    let delete_question = i18n::tf("channels.delete_q", &[("name", &form.name)]);
    (
        jar,
        HtmlTemplate(EditPage {
            ctx,
            form,
            delete_url,
            delete_question,
        }),
    )
}

async fn edit_page(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    user.require_editor()?;
    let channel = load_channel(&core, id).await?;
    let Some(schema) = vole_kit::notify::provider(&channel.kind) else {
        let jar = flash(
            jar,
            "error",
            i18n::tf("notif.cant_edit", &[("name", &channel.name)]),
        );
        return Ok((jar, Redirect::to("/settings/notifications")).into_response());
    };
    let rules = rules_using(&core, id).await?;
    let form = edit_form(&channel, schema, rules);
    Ok(render_edit(&core, &user, &jar, form).await.into_response())
}

/// Secret re-entry errors first, then validation errors for other fields.
fn combine_errors(
    secret: Option<Vec<FieldError>>,
    validation: Option<Vec<FieldError>>,
) -> Vec<FieldError> {
    let mut errors = secret.unwrap_or_default();
    for (key, msg) in validation.unwrap_or_default() {
        if !errors.iter().any(|(k, _)| *k == key) {
            errors.push((key, msg));
        }
    }
    errors
}

async fn update(
    State(core): State<Core>,
    user: CurrentUser,
    jar: SignedCookieJar,
    Path(id): Path<i64>,
    body: Bytes,
) -> ApiResult<Response> {
    user.require_editor()?;
    let channel = load_channel(&core, id).await?;
    let schema = schema_for(&channel.kind)?;
    let pairs = parse_pairs(&body);
    let name = pair(&pairs, "name").unwrap_or_default().trim().to_string();
    let language = language_of(&pairs);
    let parsed = schema::parse_form(schema, &pairs);
    let stored = stored_config(&channel);
    let mut config = parsed.config.clone();
    let secret_errors =
        schema::merge_secrets_checked(schema, &mut config, &stored, &parsed.clear).err();
    let active = pair_flag(&pairs, "active");

    let name_error = validate_name(&name);
    let field_errors = combine_errors(secret_errors, schema::validate(schema, &config).err());
    if name_error.is_some() || !field_errors.is_empty() {
        let rules = rules_using(&core, id).await?;
        let mut form = edit_form(&channel, schema, rules);
        form.name = name;
        form.name_error = name_error.unwrap_or_default();
        form.active = active;
        form.languages = language_options(&language);
        form.language = language;
        form.fields = field_views(schema, &parsed.config, &stored, &field_errors, false);
        let (jar, page) = render_edit(&core, &user, &jar, form).await;
        return Ok((StatusCode::UNPROCESSABLE_ENTITY, jar, page).into_response());
    }

    let config = config.to_string();
    let saved_name = name.clone();
    core.with_conn(move |c| db::update_channel(c, id, &saved_name, &config, active, &language))
        .await?;
    tracing::info!(user = %user.username, channel_id = id, "notification channel updated");
    let jar = flash(jar, "success", i18n::tf("notif.saved", &[("name", &name)]));
    Ok((jar, Redirect::to("/settings/notifications")).into_response())
}

async fn remove(
    State(core): State<Core>,
    user: CurrentUser,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    user.require_editor()?;
    let channel = load_channel(&core, id).await?;
    core.with_conn(move |c| db::delete_channel(c, id)).await?;
    tracing::info!(user = %user.username, channel_id = id, "notification channel deleted");
    let message = i18n::tf("notif.deleted", &[("name", &channel.name)]);
    let from_list = headers.get("hx-target").and_then(|v| v.to_str().ok()) == Some("channels");
    if from_list {
        let rows = list_rows(&core).await?;
        return Ok((
            [toast(&message, "success")],
            HtmlTemplate(ListPartial { rows }),
        )
            .into_response());
    }
    let redirect = (
        HeaderName::from_static("hx-redirect"),
        HeaderValue::from_static("/settings/notifications"),
    );
    Ok(([redirect, toast(&message, "success")], StatusCode::OK).into_response())
}

async fn toggle_active(
    State(core): State<Core>,
    user: CurrentUser,
    Path(id): Path<i64>,
    body: Bytes,
) -> ApiResult<Response> {
    user.require_editor()?;
    let active = pair_flag(&parse_pairs(&body), "active");
    let channel = load_channel(&core, id).await?;
    core.with_conn(move |c| db::set_channel_active(c, id, active))
        .await?;
    let message = if active {
        i18n::tf("notif.resumed", &[("name", &channel.name)])
    } else {
        i18n::tf("notif.paused", &[("name", &channel.name)])
    };
    Ok(([toast(&message, "success")], StatusCode::OK).into_response())
}

/// Sends a test and reports the result as a toast (always 200 so htmx shows
/// it).
async fn run_test(
    alerts: &Alerts,
    kind: &str,
    config: &Value,
    language: &str,
    label: &str,
) -> Response {
    match alerts.send_test(kind, config, language).await {
        Ok(()) => (
            [toast(
                &i18n::tf("notif.test_sent", &[("label", label)]),
                "success",
            )],
            StatusCode::OK,
        )
            .into_response(),
        Err(err) => {
            tracing::info!(kind, %err, "test notification failed");
            let message = i18n::tf("notif.test_failed", &[("err", &err)]);
            ([toast(&message, "error")], message).into_response()
        }
    }
}

fn first_error_toast(schema: &ProviderSchema, errors: &[FieldError]) -> Response {
    let (key, msg) = &errors[0];
    let label = schema
        .field(key)
        .map_or(i18n::t("notif.settings_label"), |f| f.tr_label(schema.kind));
    ([toast(&format!("{label}: {msg}"), "error")], StatusCode::OK).into_response()
}

async fn test_unsaved(
    Extension(alerts): Extension<Alerts>,
    user: CurrentUser,
    body: Bytes,
) -> ApiResult<Response> {
    user.require_editor()?;
    let pairs = parse_pairs(&body);
    let schema = schema_for(pair(&pairs, "kind").unwrap_or_default())?;
    let parsed = schema::parse_form(schema, &pairs);
    if let Err(errors) = schema::validate(schema, &parsed.config) {
        return Ok(first_error_toast(schema, &errors));
    }
    let language = language_of(&pairs);
    Ok(run_test(
        &alerts,
        schema.kind,
        &parsed.config,
        &language,
        schema.tr_label(),
    )
    .await)
}

/// Tests a saved channel. When the edit form is included (it carries
/// `kind`), its unsaved values are tested, with blank secrets taken from
/// storage (only while the destination stays the same).
async fn test_saved(
    State(core): State<Core>,
    Extension(alerts): Extension<Alerts>,
    user: CurrentUser,
    Path(id): Path<i64>,
    body: Bytes,
) -> ApiResult<Response> {
    user.require_editor()?;
    let channel = load_channel(&core, id).await?;
    let label = format!("“{}”", channel.name);
    let pairs = parse_pairs(&body);
    let Some(schema) = vole_kit::notify::provider(&channel.kind) else {
        let msg = i18n::tf("notif.cant_send", &[("label", &label)]);
        return Ok(([toast(&msg, "error")], StatusCode::OK).into_response());
    };
    let stored = stored_config(&channel);
    let (config, language) = if pair(&pairs, "kind").is_some() {
        let parsed = schema::parse_form(schema, &pairs);
        let mut config = parsed.config;
        let secret_errors =
            schema::merge_secrets_checked(schema, &mut config, &stored, &parsed.clear).err();
        let errors = combine_errors(secret_errors, schema::validate(schema, &config).err());
        if !errors.is_empty() {
            return Ok(first_error_toast(schema, &errors));
        }
        (config, language_of(&pairs))
    } else {
        (stored, channel.language.clone())
    };
    Ok(run_test(&alerts, schema.kind, &config, &language, &label).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vole_kit::notify::{discord, telegram, webhook};

    #[test]
    fn provider_search_filters_and_groups() {
        crate::product::install();
        let all = provider_groups("");
        assert_eq!(
            all.iter().map(|g| g.providers.len()).sum::<usize>(),
            PROVIDERS.len()
        );
        let chat = provider_groups("disc");
        assert_eq!(chat.len(), 1);
        assert_eq!(chat[0].providers[0].kind, "discord");
        assert!(provider_groups("zzz-nothing").is_empty());
    }

    #[test]
    fn saved_secrets_render_empty_with_the_mask() {
        crate::product::install();
        let stored = json!({"bot_token": "1:SECRET", "chat_id": "42", "silent": true});
        let shown = schema::redact(Some(&telegram::SCHEMA), &stored);
        let views = field_views(&telegram::SCHEMA, &shown, &stored, &[], false);
        let token = views.iter().find(|v| v.name == "c_bot_token").unwrap();
        assert!(token.value.is_empty());
        assert!(token.saved);
        assert_eq!(token.placeholder, SECRET_MASK);
        assert_eq!(token.input, "password");
        for v in &views {
            assert!(!v.value.contains("SECRET"));
        }
        let stored = json!({"url": "https://discord.com/api/webhooks/1/SECRET"});
        let shown = schema::redact(Some(&discord::SCHEMA), &stored);
        let views = field_views(&discord::SCHEMA, &shown, &stored, &[], false);
        assert!(views.iter().all(|v| !v.value.contains("SECRET")));
        let stored = json!({"url": "https://x", "headers": {"Authorization": "Bearer SECRET"}});
        let shown = schema::redact(Some(&webhook::SCHEMA), &stored);
        let views = field_views(&webhook::SCHEMA, &shown, &stored, &[], false);
        let headers = views.iter().find(|v| v.input == "kv").unwrap();
        assert_eq!(
            headers.rows,
            [("Authorization".to_string(), SECRET_MASK.to_string())]
        );
    }

    #[test]
    fn names_are_checked() {
        crate::product::install();
        assert!(validate_name("").is_some());
        assert!(validate_name(&"x".repeat(101)).is_some());
        assert!(validate_name("Ops").is_none());
    }
}
