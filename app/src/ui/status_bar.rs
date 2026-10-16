//! The status bar: connection · shard and tick · zoom level · hovered room · loading progress
//! · rate limits lifted … frame time, graphics backend and platform.
//!
//! An item shows only once its value is known (no "—" placeholders). While a map is shown its
//! tick and zoom-level slots keep their width even before they are known, and every item has a
//! fixed minimum width with tabular digits (the tick's sized for the server's shard names and a
//! 9-digit tick), so values arriving or changing never shift their neighbours.

use leptos::prelude::*;
use overseer::ConnectionStatus;
use overseer_view::Lod;

use crate::platform::Platform;
use crate::route::Shard;
use crate::state::AppState;
use crate::ui::hover::{MapRooms, place_text};
use crate::ui::rate_limits::{RateLimitsUi, StatusLifted, StatusReset};
use crate::ui::style;
use crate::ui::topbar::status_class;
use crate::ui::world::{MapState, Phase};

/// The name of a zoom level, from the whole world in to one room.
pub const fn level_name(lod: Lod) -> &'static str {
    match lod {
        Lod::Overview => "World",
        Lod::Terrain => "Sectors",
        Lod::Minimap => "Rooms",
        Lod::Detail => "Room",
    }
}

/// What the connection item says.
fn connection_text(status: Option<ConnectionStatus>) -> String {
    match status {
        None => "Not connected".to_owned(),
        Some(ConnectionStatus::Connecting) => "Connecting…".to_owned(),
        Some(ConnectionStatus::Connected) => "Connected".to_owned(),
        Some(ConnectionStatus::Reconnecting { delay, .. }) => {
            format!("Reconnecting in {} s", delay.as_secs())
        }
        Some(ConnectionStatus::Closed) => "Disconnected".to_owned(),
    }
}

/// A slot of the status bar.
#[derive(Clone, Debug, PartialEq)]
enum Slot {
    /// Not part of the bar.
    Hidden,
    /// Keeps its width, empty: the value is coming.
    Reserved,
    /// Shows the value.
    Shown(String),
}

impl Slot {
    /// `value` when known; else reserved while `reserve`, hidden otherwise.
    fn of(value: Option<String>, reserve: bool) -> Self {
        match value {
            Some(value) => Self::Shown(value),
            None if reserve => Self::Reserved,
            None => Self::Hidden,
        }
    }

    /// The text shown (empty unless [`Self::Shown`]).
    fn text(&self) -> String {
        match self {
            Self::Shown(text) => text.clone(),
            Self::Hidden | Self::Reserved => String::new(),
        }
    }
}

/// What the status bar knows.
#[derive(Clone, Debug, Default)]
struct Facts {
    /// A map (world or popped-out room) is on screen.
    map: bool,
    /// The shard shown, on sharded servers.
    shard: Option<String>,
    /// Game tick of the latest live room.
    game_time: Option<u64>,
    /// Zoom level of the world map.
    lod: Option<Lod>,
    /// CPU time of the map's last frame, in milliseconds.
    frame_ms: Option<f32>,
    /// Graphics backend.
    backend: Option<&'static str>,
    /// The room (and tile) under a hovering pointer ([`crate::ui::hover::place_text`]).
    hover: Option<String>,
}

/// The map items of the status bar, from what is known.
#[derive(Clone, Debug, PartialEq)]
struct MapItems {
    place: Slot,
    level: Slot,
    hover: Slot,
    frame: Slot,
    backend: Slot,
}

impl MapItems {
    fn of(facts: &Facts) -> Self {
        let place = facts.game_time.map(|time| match &facts.shard {
            Some(shard) => format!("{shard} · tick {time}"),
            None => format!("tick {time}"),
        });
        let on_map = |value: Option<String>| value.filter(|_| facts.map);
        Self {
            place: Slot::of(place, facts.map),
            level: Slot::of(facts.lod.map(|lod| level_name(lod).to_owned()), facts.map),
            hover: Slot::of(on_map(facts.hover.clone()), facts.map),
            frame: Slot::of(
                on_map(facts.frame_ms.map(|ms| format!("frame {ms:.2} ms"))),
                false,
            ),
            backend: Slot::of(on_map(facts.backend.map(str::to_owned)), false),
        }
    }
}

/// Digits the tick slot leaves room for: 9 (a long-running server's ticks have 8).
const TICK_DIGITS: usize = 9;

/// The minimum width of the tick slot, in `ch` (the width of a digit; with tabular digits every
/// digit takes it, the letters of `tick` and of shard names take less): `tick ` and
/// [`TICK_DIGITS`] digits, after `<shard> · ` for the longest of the server's `shards` and the
/// `shown` one. Constant for a server, so the first tick arriving never moves the items after it.
fn place_width(shards: &[String], shown: Option<&str>) -> usize {
    let shard = shards
        .iter()
        .map(String::as_str)
        .chain(shown)
        .map(|name| name.chars().count())
        .max();
    "tick ".len() + TICK_DIGITS + shard.map_or(0, |length| length + " · ".chars().count())
}

/// The status item of `slot` with `class` and `title`, at least `min_width` `ch` wide when
/// given. The element stays while the slot is not hidden and only its text follows the value:
/// no re-render per value, and observers of the element (the end-to-end tests) keep seeing it.
fn item(
    slot: Memo<Slot>,
    class: &'static str,
    title: &'static str,
    min_width: Option<Memo<usize>>,
) -> impl IntoView {
    let shown = Memo::new(move |_| slot.with(|slot| *slot != Slot::Hidden));
    let reserved = Memo::new(move |_| slot.with(|slot| *slot == Slot::Reserved));
    let style = move || {
        min_width
            .map(|width| format!("min-width: {}ch", width.get()))
            .unwrap_or_default()
    };
    move || {
        shown.get().then(|| {
            view! {
                <span
                    class=class
                    style=style
                    title=title
                    aria-hidden=move || reserved.get().then_some("true")
                >
                    {move || slot.with(Slot::text)}
                </span>
            }
        })
    }
}

/// The status bar.
#[component]
pub fn StatusBar() -> impl IntoView {
    let state = AppState::expect();
    let map = MapState::expect();
    let rooms = MapRooms::expect();
    let limits = RateLimitsUi::expect();
    let items = Memo::new(move |_| {
        MapItems::of(&Facts {
            map: map.phase.get() != Phase::Idle,
            shard: map.shard.get().and_then(|shard| match shard {
                Shard::Named(name) => Some(name),
                Shard::Single => None,
            }),
            game_time: map.game_time.get(),
            lod: map.lod.get(),
            frame_ms: map.frame_ms.get(),
            backend: map.backend.get(),
            hover: rooms
                .hover
                .get()
                .map(|hovered| place_text(&hovered, map.lod.get())),
        })
    });
    let place = Memo::new(move |_| items.with(|items| items.place.clone()));
    // Known with the session (the server's shards) and the route (the shard shown), before
    // the first tick.
    let place_width = Memo::new(move |_| {
        let shown = map.shard.get().and_then(|shard| match shard {
            Shard::Named(name) => Some(name),
            Shard::Single => None,
        });
        map.shards
            .with(|shards| place_width(shards, shown.as_deref()))
    });
    let level = Memo::new(move |_| items.with(|items| items.level.clone()));
    let hover = Memo::new(move |_| items.with(|items| items.hover.clone()));
    let frame = Memo::new(move |_| items.with(|items| items.frame.clone()));
    let backend = Memo::new(move |_| items.with(|items| items.backend.clone()));
    let connection_title = move || match state.status.get() {
        Some(ConnectionStatus::Reconnecting { attempt, .. }) => format!("Attempt {attempt}"),
        _ => String::new(),
    };
    let progress = Memo::new(move |_| {
        map.progress.with(|progress| {
            progress.as_ref().and_then(|progress| {
                // The clock only matters while paused: the time left updates every second.
                let now = if progress.stats_paused_until.is_some() {
                    limits.now.get()
                } else {
                    0
                };
                crate::feed::progress_label(progress, now)
            })
        })
    });
    let loading = Memo::new(move |_| progress.with(Option::is_some));
    let title = Memo::new(move |_| {
        map.progress
            .with(|progress| crate::feed::progress_title(progress.as_ref()))
    });
    let progress = move || {
        loading.get().then(|| {
            view! {
                <span
                    class="inline-flex flex-initial items-center gap-2 min-w-0"
                    role="status"
                    title=move || title.get()
                >
                    <span class=style::SPINNER_SMALL aria-hidden="true"></span>
                    <span class="truncate">{move || progress.get().unwrap_or_default()}</span>
                </span>
                <StatusReset />
            }
        })
    };
    view! {
        <footer
            class="flex flex-none items-center gap-5 max-md:gap-3 h-statusbar px-3 overflow-hidden \
                whitespace-nowrap text-sm tabular-nums text-muted bg-panel border-t border-edge"
            aria-label="Status"
        >
            <span
                class="inline-flex flex-none items-center gap-2 min-w-[9.5em]"
                title=connection_title
            >
                <span class=move || status_class(state.status.get()) aria-hidden="true"></span>
                {move || connection_text(state.status.get())}
            </span>
            {item(place, "inline-flex flex-none items-center gap-2", "Game tick", Some(place_width))}
            {item(
                level,
                "inline-flex flex-none items-center gap-2 min-w-[4.5em] text-fg",
                "Zoom level",
                None,
            )}
            {item(
                hover,
                "inline-flex flex-none items-center gap-2 min-w-[6.5em] text-fg",
                "Room under the pointer",
                None,
            )}
            {progress}
            <StatusLifted />
            <span class="inline-flex items-center gap-5 max-md:gap-3 ml-auto">
                {item(
                    frame,
                    "inline-flex flex-none items-center gap-2 min-w-[7.5em]",
                    "CPU time of the last frame",
                    None,
                )}
                {item(
                    backend,
                    "inline-flex flex-none items-center gap-2 max-xs:hidden",
                    "Graphics backend",
                    None,
                )}
                <span class="inline-flex flex-none items-center gap-2 max-md:hidden">
                    {move || {
                        if state.platform.with_value(Platform::is_tauri) { "Desktop" } else { "Web" }
                    }}
                </span>
            </span>
        </footer>
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use overseer::ConnectionStatus;
    use overseer_view::Lod;

    use super::{Facts, MapItems, Slot, connection_text, level_name, place_width};

    #[test]
    fn zoom_levels_are_named_from_the_world_in_to_a_room() {
        let names = [Lod::Overview, Lod::Terrain, Lod::Minimap, Lod::Detail].map(level_name);
        assert_eq!(names, ["World", "Sectors", "Rooms", "Room"]);
    }

    #[test]
    fn without_a_map_no_map_item_shows() {
        // Before connecting, on the server list: nothing but the connection and platform.
        let items = MapItems::of(&Facts::default());
        assert_eq!(
            items,
            MapItems {
                place: Slot::Hidden,
                level: Slot::Hidden,
                hover: Slot::Hidden,
                frame: Slot::Hidden,
                backend: Slot::Hidden,
            }
        );
        // Stale values without a map stay hidden too.
        let items = MapItems::of(&Facts {
            frame_ms: Some(1.0),
            backend: Some("WebGL2"),
            ..Facts::default()
        });
        assert_eq!((items.frame, items.backend), (Slot::Hidden, Slot::Hidden));
    }

    #[test]
    fn a_map_reserves_its_slots_until_their_values_arrive() {
        let loading = Facts {
            map: true,
            ..Facts::default()
        };
        let items = MapItems::of(&loading);
        assert_eq!(items.place, Slot::Reserved);
        assert_eq!(items.level, Slot::Reserved);
        assert_eq!(
            items.hover,
            Slot::Reserved,
            "nothing hovered: the slot keeps its width"
        );
        assert_eq!(
            (items.frame, items.backend),
            (Slot::Hidden, Slot::Hidden),
            "nothing drawn yet"
        );

        let drawn = Facts {
            lod: Some(Lod::Terrain),
            frame_ms: Some(0.234),
            backend: Some("WebGPU"),
            ..loading.clone()
        };
        let items = MapItems::of(&drawn);
        assert_eq!(items.place, Slot::Reserved, "no live room yet");
        assert_eq!(items.level, Slot::Shown("Sectors".to_owned()));
        assert_eq!(items.frame, Slot::Shown("frame 0.23 ms".to_owned()));
        assert_eq!(items.backend, Slot::Shown("WebGPU".to_owned()));
        let hovering = Facts {
            hover: Some("W3N4".to_owned()),
            ..drawn.clone()
        };
        assert_eq!(
            MapItems::of(&hovering).hover,
            Slot::Shown("W3N4".to_owned())
        );

        let live = Facts {
            game_time: Some(1234),
            lod: Some(Lod::Detail),
            ..drawn.clone()
        };
        assert_eq!(
            MapItems::of(&live).place,
            Slot::Shown("tick 1234".to_owned())
        );
        let sharded = Facts {
            shard: Some("shard3".to_owned()),
            ..live
        };
        assert_eq!(
            MapItems::of(&sharded).place,
            Slot::Shown("shard3 · tick 1234".to_owned())
        );
    }

    #[test]
    fn the_tick_slot_fits_the_longest_shard_and_a_nine_digit_tick() {
        // Single world: `tick 123456789`.
        assert_eq!(place_width(&[], None), "tick 123456789".chars().count());
        // screeps.com: `shard0 · tick 123456789`, whichever shard is shown.
        let shards = ["shard0", "shard1", "shard2", "shard3"].map(str::to_owned);
        let widest = "shard0 · tick 123456789".chars().count();
        assert_eq!(place_width(&shards, Some("shard2")), widest);
        assert_eq!(place_width(&shards, None), widest);
        // A longer name (a season's) widens it; a shown shard counts before the list arrives.
        assert_eq!(
            place_width(&[], Some("shardSeason")),
            "shardSeason · tick 123456789".chars().count()
        );
    }

    #[test]
    fn the_connection_is_named_without_placeholders() {
        assert_eq!(connection_text(None), "Not connected");
        assert_eq!(
            connection_text(Some(ConnectionStatus::Connected)),
            "Connected"
        );
        assert_eq!(
            connection_text(Some(ConnectionStatus::Reconnecting {
                attempt: 3,
                delay: Duration::from_secs(4),
            })),
            "Reconnecting in 4 s"
        );
    }
}
