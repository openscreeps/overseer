//! The tick speed of the shard on the map: [`TickUi`] shows the session's
//! [`overseer::Map::tick_speed`] of the map followed.
//!
//! [`TickUi::follow`] runs while a world screen or a room window shows a map of the live
//! session and holds the session's [`TickWatch`] of the map's shard; the session asks for and
//! measures the tick speed. Another session or shard follows the new one; no map drops the
//! watch.

use futures_util::future::{AbortHandle, abortable};
use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::Overseer;
use overseer::map::{TickSpeed, TickWatch};

use crate::route::Shard;
use crate::state::AppState;
use crate::ui::world::{MapState, Phase};

/// The map [`TickUi`] follows: the session ([`crate::state::Session::epoch`]) and the shard
/// (`None` on single-world servers).
type Followed = (u64, Option<String>);

/// The tick duration of the shard on the map; `Copy`, provided as context.
#[derive(Clone, Copy)]
pub struct TickUi {
    /// The latest tick speed of the followed map's [`TickWatch`].
    speed: RwSignal<Option<TickSpeed>>,
    /// The tick duration to show, when known.
    pub duration: Signal<Option<TickSpeed>>,
    /// The map followed.
    followed: StoredValue<Option<Followed>>,
    /// Stops reading the watch of the map followed (and drops the watch).
    task: StoredValue<Option<AbortHandle>>,
}

impl Default for TickUi {
    /// Nothing known yet.
    fn default() -> Self {
        let speed = RwSignal::new(None);
        Self {
            speed,
            duration: speed.into(),
            followed: StoredValue::new(None),
            task: StoredValue::new(None),
        }
    }
}

impl TickUi {
    /// The state provided by [`crate::ui::App`].
    pub fn expect() -> Self {
        expect_context()
    }

    /// Follows the map of the live session for as long as the page lives.
    pub fn follow(self, state: AppState, map: MapState) {
        Effect::new(move |_| {
            let shown = !matches!(map.phase.get(), Phase::Idle | Phase::Failed(_));
            let shard = map
                .shard
                .with(|shard| shard.as_ref().and_then(Shard::name).map(str::to_owned));
            let session = state.session.with(|session| {
                session
                    .as_ref()
                    .filter(|_| shown)
                    .map(|session| (session.epoch(), session.overseer.clone()))
            });
            self.start(session.map(|(epoch, overseer)| ((epoch, shard), overseer)));
        });
    }

    /// Follows `map` (`None`: no map), unless it does already: drops the watch of the previous
    /// map and reads the session's watch of the new one.
    fn start(self, map: Option<(Followed, Overseer)>) {
        let followed = map.as_ref().map(|(followed, _)| followed.clone());
        if self.followed.with_value(|current| *current == followed) {
            return;
        }
        self.followed.set_value(followed);
        if let Some(task) = self.task.try_update_value(Option::take).flatten() {
            task.abort();
        }
        self.speed.set(None);
        let Some(((_, shard), overseer)) = map else {
            return;
        };
        // `Err` only after the session closed: nothing to show.
        let Ok(watch) = overseer.map().tick_speed(shard.as_deref()) else {
            return;
        };
        let (future, handle) = abortable(self.read(watch));
        self.task.set_value(Some(handle));
        spawn_local(async move {
            // `Err` only says it was aborted.
            let _ = future.await;
        });
    }

    /// Shows each tick speed of `watch` until the session closes.
    async fn read(self, mut watch: TickWatch) {
        self.speed.try_set(watch.latest());
        while let Ok(speed) = watch.changed().await {
            self.speed.try_set(speed);
        }
    }
}
