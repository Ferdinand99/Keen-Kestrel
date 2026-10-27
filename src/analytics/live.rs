//! What the Web and App tabs' live streams (`…/web/live`, `…/app/live`)
//! wake for, on the kit's `http::live`: a project's new raw events (the
//! current visitors or users), its new rollups (the numbers and charts),
//! and a tick when neither came for a while (people leave, so the current
//! count drops on its own). Each kind's bursts are merged over a quiet
//! period.
//!
//! The streams are capped like the kit's other live pages: at most
//! [`MAX_STREAMS`] open at once ([`STREAMS`]), each ending after
//! [`MAX_LIFETIME`], when the browser's `EventSource` reconnects and the
//! route checks the session again.

use std::pin::Pin;
use std::time::Duration;

use futures_util::stream::{self, Stream, StreamExt as _};
use tokio::sync::broadcast;
use vole_kit::http::live::{Changes, StreamLimit};

use crate::writer::{WebChange, Writer};

/// Web and App streams open at once, all projects and users together.
pub const MAX_STREAMS: usize = 64;
/// How long one stream stays open.
pub const MAX_LIFETIME: Duration = Duration::from_secs(3600);

/// The cap on open Web and App streams.
pub static STREAMS: StreamLimit = StreamLimit::new(MAX_STREAMS);

/// Why a live stream wakes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// New raw events or signals: send the current visitors or users.
    Events,
    /// New rollups: send the numbers and charts.
    Rollup,
    /// Nothing for a while: send the current count again.
    Tick,
}

/// A project's analytics changes, for one live stream.
pub struct LiveChanges {
    changes: Pin<Box<dyn Stream<Item = Change> + Send>>,
    tick: Duration,
}

impl LiveChanges {
    /// Changes to `project_id`, each kind's bursts merged over `quiet`; a
    /// [`Change::Tick`] after `tick` without one. The stream ends after
    /// `lifetime`.
    pub fn new(
        writer: &Writer,
        project_id: i64,
        quiet: Duration,
        tick: Duration,
        lifetime: Duration,
    ) -> Self {
        Self::from_receivers(
            writer.subscribe_web(),
            writer.subscribe_web(),
            project_id,
            (quiet, tick, lifetime),
        )
    }

    /// [`Self::new`] with two receivers of the writer's analytics changes.
    fn from_receivers(
        events: broadcast::Receiver<WebChange>,
        rollups: broadcast::Receiver<WebChange>,
        project_id: i64,
        (quiet, tick, lifetime): (Duration, Duration, Duration),
    ) -> Self {
        let events = Changes::new(events, move |c: &WebChange| {
            *c == WebChange::Events(project_id)
        })
        .quiet(quiet)
        .lifetime(lifetime);
        let rollups = Changes::new(rollups, move |c: &WebChange| {
            *c == WebChange::Rollup(project_id)
        })
        .quiet(quiet)
        .lifetime(lifetime);
        // A receiver that fell behind counts as a change of its kind.
        let events = stream::unfold(events, |mut c| async move {
            c.next().await?;
            Some((Change::Events, c))
        });
        let rollups = stream::unfold(rollups, |mut c| async move {
            c.next().await?;
            Some((Change::Rollup, c))
        });
        LiveChanges {
            changes: Box::pin(stream::select(events, rollups)),
            tick,
        }
    }

    /// The next change; `None` once the writer is gone or the lifetime is
    /// over.
    pub async fn next(&mut self) -> Option<Change> {
        match tokio::time::timeout(self.tick, self.changes.next()).await {
            Ok(change) => change,
            Err(_) => Some(Change::Tick),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bursts_merge_and_ticks_come_when_nothing_happens() {
        let (tx, _) = broadcast::channel(64);
        let mut live = LiveChanges::from_receivers(
            tx.subscribe(),
            tx.subscribe(),
            7,
            (
                Duration::from_millis(20),
                Duration::from_millis(200),
                Duration::from_secs(60),
            ),
        );
        for _ in 0..5 {
            tx.send(WebChange::Events(7)).unwrap();
            tx.send(WebChange::Events(8)).unwrap();
        }
        assert_eq!(live.next().await, Some(Change::Events));
        tx.send(WebChange::Rollup(8)).unwrap();
        tx.send(WebChange::Rollup(7)).unwrap();
        assert_eq!(live.next().await, Some(Change::Rollup));
        assert_eq!(
            live.next().await,
            Some(Change::Tick),
            "the burst was one change"
        );
        drop(tx);
        assert_eq!(live.next().await, None);
    }

    #[tokio::test]
    async fn a_stream_ends_after_its_lifetime() {
        let (tx, _) = broadcast::channel::<WebChange>(8);
        let mut live = LiveChanges::from_receivers(
            tx.subscribe(),
            tx.subscribe(),
            7,
            (
                Duration::from_millis(10),
                Duration::from_millis(500),
                Duration::from_millis(80),
            ),
        );
        // Ticks are not changes: with the lifetime over, the stream is done
        // even though the writer is still there.
        assert_eq!(live.next().await, None);
    }

    #[test]
    fn the_streams_are_capped() {
        let held: Vec<_> = (0..MAX_STREAMS).filter_map(|_| STREAMS.acquire()).collect();
        assert_eq!(held.len(), MAX_STREAMS);
        assert!(STREAMS.acquire().is_none(), "the 65th is turned away");
        drop(held);
        assert!(STREAMS.acquire().is_some(), "places come back");
    }
}
