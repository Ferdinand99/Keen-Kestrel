//! `/projects/{id}/issues/live`: the issue list as a server-sent event
//! stream. The page connects with its own query string, so every update is
//! rendered for exactly the view the user is looking at. The writer says
//! which project changed after each batch and each action; bursts are
//! merged, so a busy project re-renders at most about once a second per
//! open list.

use std::convert::Infallible;
use std::time::Duration;

use askama::Template;
use axum::extract::{Extension, Path, Query as UrlQuery, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use tokio::sync::broadcast::{self, error::RecvError, error::TryRecvError};
use vole_kit::auth::CurrentUser;
use vole_kit::http::ApiResult;
use vole_kit::i18n;
use vole_kit::state::Core;

use super::{ListForm, ListOnlyTemplate, ListState, load_list, load_project};
use crate::ingest::Ingest;

/// Quiet time that merges a burst of changes into one render.
const COALESCE: Duration = Duration::from_millis(1000);

/// Waits for a change to `project_id`, then swallows whatever else arrives
/// within [`COALESCE`]. `false` once the writer is gone.
async fn next_change(rx: &mut broadcast::Receiver<i64>, project_id: i64) -> bool {
    loop {
        match rx.recv().await {
            Ok(p) if p == project_id => break,
            Ok(_) => continue,
            Err(RecvError::Lagged(_)) => break,
            Err(RecvError::Closed) => return false,
        }
    }
    tokio::time::sleep(COALESCE).await;
    loop {
        match rx.try_recv() {
            Ok(_) | Err(TryRecvError::Lagged(_)) => continue,
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Closed) => return false,
        }
    }
}

struct Live {
    core: Core,
    rx: broadcast::Receiver<i64>,
    project_id: i64,
    form: ListForm,
    can_edit: bool,
    lang: i18n::Lang,
}

pub async fn stream(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    user: CurrentUser,
    Path(id): Path<i64>,
    UrlQuery(form): UrlQuery<ListForm>,
) -> ApiResult<Response> {
    load_project(&core, id).await?;
    let live = Live {
        core,
        rx: ingest.writer().subscribe(),
        project_id: id,
        form,
        can_edit: user.can_edit(),
        lang: i18n::current(),
    };
    let events = futures_util::stream::unfold(live, |mut live| async move {
        loop {
            if !next_change(&mut live.rx, live.project_id).await {
                return None;
            }
            let lang = live.lang;
            let (state, query) = ListState::from_form(&live.form);
            let loaded = i18n::scope(
                lang,
                load_list(&live.core, live.project_id, &state, query, live.can_edit),
            )
            .await;
            let Ok(mut data) = loaded else {
                continue;
            };
            data.select = false;
            let html = i18n::sync_scope(lang, || ListOnlyTemplate { data }.render());
            match html {
                Ok(html) => {
                    let event = Event::default().event("issues-updated").data(html);
                    return Some((Ok::<_, Infallible>(event), live));
                }
                Err(err) => tracing::warn!(%err, "can't render the live issue list"),
            }
        }
    });
    Ok(Sse::new(events)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response())
}
