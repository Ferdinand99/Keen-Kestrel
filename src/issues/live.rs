//! `/projects/{id}/issues/live`: the issue list as a server-sent event
//! stream. The page connects with its own query string, so every update is
//! rendered for exactly the view the user is looking at. The writer says
//! which project changed after each batch and each action; bursts are
//! merged, so a busy project re-renders at most about once a second per
//! view, and lists open on the same view share one render (the kit's
//! `http::live`). At most [`MAX_STREAMS`] lists are open at once, and one
//! stays open for [`MAX_LIFETIME`] at most; the browser's `EventSource`
//! then reconnects, which checks the session (and the role) again.

use std::sync::LazyLock;
use std::time::Duration;

use askama::Template;
use axum::extract::{Extension, Path, Query as UrlQuery, State};
use axum::response::Response;
use vole_kit::auth::CurrentUser;
use vole_kit::http::live::{self, Changes, SharedRenders, StreamLimit};
use vole_kit::http::{ApiError, ApiResult};
use vole_kit::i18n::{self, Lang};
use vole_kit::state::Core;

use super::{ListForm, ListOnlyTemplate, ListState, load_list, load_project};
use crate::ingest::Ingest;

/// Quiet time that merges a burst of changes into one render.
const COALESCE: Duration = Duration::from_millis(1000);

/// Issue streams open at once.
const MAX_STREAMS: usize = 64;
/// How long one stream stays open.
const MAX_LIFETIME: Duration = Duration::from_secs(3600);

static STREAMS: StreamLimit = StreamLimit::new(MAX_STREAMS);

/// One view of a list: project, query string, whether the reader may edit
/// (bulk actions), language.
type ViewKey = (i64, String, bool, Lang);

/// Views rendered at most once per change, however many lists show them.
static RENDERS: LazyLock<SharedRenders<ViewKey>> = LazyLock::new(|| SharedRenders::new(256));

struct Live {
    core: Core,
    project_id: i64,
    form: ListForm,
    can_edit: bool,
    lang: Lang,
}

impl Live {
    fn key(&self) -> ViewKey {
        let (state, _) = ListState::from_form(&self.form);
        (
            self.project_id,
            state.qs(state.page),
            self.can_edit,
            self.lang,
        )
    }

    async fn render(&self) -> Result<String, ApiError> {
        let (state, query) = ListState::from_form(&self.form);
        let mut data = load_list(&self.core, self.project_id, &state, query, self.can_edit).await?;
        data.select = false;
        ListOnlyTemplate { data }.render().map_err(|err| {
            tracing::warn!(%err, "can't render the live issue list");
            ApiError::Internal(err.to_string())
        })
    }
}

pub async fn stream(
    State(core): State<Core>,
    Extension(ingest): Extension<Ingest>,
    user: CurrentUser,
    Path(id): Path<i64>,
    UrlQuery(form): UrlQuery<ListForm>,
) -> ApiResult<Response> {
    load_project(&core, id).await?;
    let Some(slot) = STREAMS.acquire() else {
        return Ok(live::busy());
    };
    let changes = Changes::new(ingest.writer().subscribe(), move |p: &i64| *p == id)
        .quiet(COALESCE)
        .lifetime(MAX_LIFETIME);
    let live = Live {
        core,
        project_id: id,
        form,
        can_edit: user.can_edit(),
        // The stream runs outside the request's language scope.
        lang: i18n::current(),
    };
    Ok(live::sse(futures_util::stream::unfold(
        (changes, live, slot),
        |(mut changes, live, slot)| async move {
            loop {
                let since = changes.next().await?;
                let render = RENDERS.get(live.key(), since, || live.render());
                if let Ok(html) = i18n::scope(live.lang, render).await {
                    let event = live::html_event("issues-updated", &html);
                    return Some((event, (changes, live, slot)));
                }
            }
        },
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_issue_streams_are_capped() {
        let held: Vec<_> = (0..MAX_STREAMS).filter_map(|_| STREAMS.acquire()).collect();
        assert_eq!(held.len(), MAX_STREAMS);
        assert!(STREAMS.acquire().is_none(), "the next one is turned away");
        drop(held);
        assert!(STREAMS.acquire().is_some());
        assert_eq!(MAX_LIFETIME, Duration::from_secs(3600));
    }
}
