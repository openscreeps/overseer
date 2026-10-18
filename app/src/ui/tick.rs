//! The tick speed of the shard on the map ([`crate::tick`]): [`TickUi`] is the one owner of the
//! tick duration the app shows.
//!
//! [`TickUi::follow`] runs while a world screen or a room window shows a map of the live
//! session. It asks the server for its tick duration every [`SERVER_POLL_MS`]
//! ([`overseer::Map::tick_duration`]), records each game tick of the live rooms
//! ([`MapState::game_time`]), and asks for the game time every [`TIME_POLL_MS`]
//! ([`overseer::Map::game_time`]) while neither gives a duration. Each request waits for the
//! session's request budget first. Another session, shard or map starts over; no map stops it.

use std::time::Duration;

use futures_util::future::{AbortHandle, abortable, join};
use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::Overseer;
use overseer::net::{Priority, rt};

use crate::route::Shard;
use crate::session;
use crate::state::AppState;
use crate::tick::{SERVER_POLL_MS, TIME_POLL_MS, TickDuration, TickSpeed};
use crate::ui::rate_limits::epoch_millis;
use crate::ui::world::{MapState, Phase};

/// The map [`TickUi`] follows: the session ([`crate::state::Session::epoch`]) and the shard
/// (`None` on single-world servers).
type Followed = (u64, Option<String>);

/// The tick duration of the shard on the map; `Copy`, provided as context.
#[derive(Clone, Copy)]
pub struct TickUi {
    /// What is known of the tick speed of the followed map.
    speed: RwSignal<TickSpeed>,
    /// The tick duration to show, when known ([`TickSpeed::duration`]; follows the clock).
    pub duration: Memo<Option<TickDuration>>,
    /// The map followed.
    followed: StoredValue<Option<Followed>>,
    /// Stops the requests of the map followed.
    task: StoredValue<Option<AbortHandle>>,
}

impl TickUi {
    /// Nothing known yet, with the clock `now` (milliseconds since the Unix epoch).
    pub fn new(now: Signal<u64>) -> Self {
        let speed = RwSignal::new(TickSpeed::default());
        Self {
            speed,
            duration: Memo::new(move |_| speed.with(|speed| speed.duration(now.get()))),
            followed: StoredValue::new(None),
            task: StoredValue::new(None),
        }
    }

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
        Effect::new(move |_| {
            if let Some(tick) = map.game_time.get() {
                self.speed
                    .update(|speed| speed.observe_live(tick, epoch_millis()));
            }
        });
    }

    /// Follows `map` (`None`: no map), unless it does already: forgets what it knew and asks
    /// the map's server.
    fn start(self, map: Option<(Followed, Overseer)>) {
        let followed = map.as_ref().map(|(followed, _)| followed.clone());
        if self.followed.with_value(|current| *current == followed) {
            return;
        }
        self.followed.set_value(followed);
        if let Some(task) = self.task.try_update_value(Option::take).flatten() {
            task.abort();
        }
        self.speed.set(TickSpeed::default());
        let Some(((_, shard), overseer)) = map else {
            return;
        };
        let (future, handle) = abortable(join(
            self.poll_server(overseer.clone(), shard.clone()),
            self.poll_game_time(overseer, shard),
        ));
        self.task.set_value(Some(handle));
        spawn_local(async move {
            // `Err` only says it was aborted.
            let _ = future.await;
        });
    }

    /// Asks the server for the tick duration of `shard` every [`SERVER_POLL_MS`]. A failed
    /// request keeps the last duration until it is stale.
    async fn poll_server(self, overseer: Overseer, shard: Option<String>) {
        loop {
            session::wait_for_budget(&overseer, Priority::Background, 0).await;
            if let Ok(ms) = overseer.map().tick_duration(shard.as_deref()).await {
                self.speed.update(|speed| speed.report(ms, epoch_millis()));
            }
            rt::sleep(Duration::from_millis(SERVER_POLL_MS)).await;
        }
    }

    /// Asks for the game time of `shard` every [`TIME_POLL_MS`] while
    /// [`TickSpeed::wants_game_time`]; the first time after one period, when the other sources
    /// had their chance.
    async fn poll_game_time(self, overseer: Overseer, shard: Option<String>) {
        loop {
            rt::sleep(Duration::from_millis(TIME_POLL_MS)).await;
            if !self
                .speed
                .with_untracked(|speed| speed.wants_game_time(epoch_millis()))
            {
                continue;
            }
            session::wait_for_budget(&overseer, Priority::Background, 0).await;
            if let Ok(tick) = overseer.map().game_time(shard.as_deref()).await {
                self.speed
                    .update(|speed| speed.observe_polled(tick, epoch_millis()));
            }
        }
    }
}
