//! The status bar: connection · shard, tick and tick duration · zoom level · hovered room ·
//! loading progress · rate limits lifted … CPU and Memory usage, frame time, graphics backend
//! and platform.
//!
//! An item shows only once its value is known (no "—" placeholders). While a map is shown its
//! tick and zoom-level slots keep their width even before they are known, and every item has a
//! fixed width or minimum width with tabular digits (the tick's sized for the server's shard
//! names, a 9-digit tick and the longest tick duration), so values arriving or changing never
//! shift their neighbours. The tick item's tooltip gives its full text and says where the tick
//! duration comes from ([`TickUi`]).
//!
//! The bar fits its items to its width ([`style::FIT_ROW`]): it shows them in priority order
//! and drops whole items, never part of one. The connection, the tick and the zoom level always
//! stay: they come first, and on a window too narrow for them the tick cuts its text with an
//! ellipsis. On phones the fixed widths go and the tick leaves out its shard (the top bar names
//! it). Then come the hovered room (never for a finger, which does not hover, nor on phones),
//! the loading progress (it cuts its text before it goes), the reset of the rate limits, the
//! lifted rate limits and the usage of a signed-in account ([`StatusUsage`]). The usage takes
//! the space left: it shows less as the space shrinks (bars and values, then percentages, then
//! the CPU alone). The frame time, the graphics backend and the platform go at the right end;
//! a narrower window drops them first, from the right, and then the items before them, from the
//! usage back to the hovered room. The bar keeps the bottom safe-area inset under its items.

use leptos::prelude::*;
use overseer::ConnectionStatus;
use overseer::map::TickSpeed;
use overseer_view::Lod;

use crate::platform::Platform;
use crate::route::Shard;
use crate::state::AppState;
use crate::tick::{self, COMPACT_CHARS};
use crate::ui::hover::{MapRooms, place_text};
use crate::ui::rate_limits::{RateLimitsUi, StatusLifted, StatusReset};
use crate::ui::style::{self, concatcp};
use crate::ui::tick::TickUi;
use crate::ui::topbar::status_class;
use crate::ui::usage::StatusUsage;
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
    /// The tick duration of the shard shown.
    tick: Option<TickSpeed>,
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
        // The shard only names the shard of a tick or a duration.
        let values: Vec<String> = facts
            .game_time
            .map(|time| format!("tick {time}"))
            .into_iter()
            .chain(facts.tick.map(|speed| tick::compact(speed.ms)))
            .collect();
        let place = (!values.is_empty()).then(|| {
            let shard = facts.shard.iter().cloned();
            shard.chain(values).collect::<Vec<_>>().join(SEPARATOR)
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
/// The separator of the parts of the tick slot.
const SEPARATOR: &str = " · ";

/// The minimum width of the tick slot, in `ch` (the width of a digit; with tabular digits every
/// digit takes it, the letters of `tick` and of shard names take less): `tick ` and
/// [`TICK_DIGITS`] digits, after `<shard> · ` for the longest of the server's `shards` and the
/// `shown` one, and before ` · ` and the longest tick duration. Constant for a server, so the
/// first tick or duration arriving never moves the items after it.
fn place_width(shards: &[String], shown: Option<&str>) -> usize {
    let separator = SEPARATOR.chars().count();
    let shard = shards
        .iter()
        .map(String::as_str)
        .chain(shown)
        .map(|name| name.chars().count())
        .max();
    "tick ".len()
        + TICK_DIGITS
        + separator
        + COMPACT_CHARS
        + shard.map_or(0, |length| length + separator)
}

/// The tooltip of the tick slot: what it shows, and where the tick duration comes from.
fn place_title(speed: Option<&TickSpeed>) -> String {
    match speed {
        Some(speed) => format!("Game tick and tick duration: {}", tick::sentence(speed)),
        None => "Game tick".to_owned(),
    }
}

/// The bar: a [`style::FIT_ROW`] one status line high, over the bottom safe-area inset.
const BAR: &str = concatcp!(
    style::FIT_ROW,
    " flex-none h-statusbar-safe px-3 pb-safe-b whitespace-nowrap text-sm tabular-nums \
    text-muted bg-panel border-t border-edge"
);
/// The items that always stay (connection, tick, zoom level): the bar's first item, which
/// shrinks on a window too narrow for it.
const CORE: &str = "inline-flex items-center gap-5 max-md:gap-3 min-w-0";
/// The tick item: as wide as its `--item-width` (constant for a server), narrower when the
/// window is too narrow for the items that always stay (it cuts its text with an ellipsis). On
/// phones only as wide as its text. A width, not a flex basis: the width of the bar's first
/// item comes from the widths of its items.
const PLACE: &str = "w-(--item-width) max-sm:w-auto min-w-0 truncate";
/// The loading progress: whole up to 12rem, else gone; with more space it grows to its full
/// text.
const PROGRESS: &str = "inline-flex items-center gap-2 grow basis-48 max-w-max min-w-0";

/// The status item of `slot` with `class` and `title`. The element stays while the slot is not
/// hidden and only its text follows the value: no re-render per value, and observers of the
/// element (the end-to-end tests) keep seeing it. `width`, when given, sets `--item-width` to
/// that many `ch` (for a class of the item to use). `lead`, when given and the text starts
/// with it, goes into a span of its own that phones do not show.
fn item(
    slot: Memo<Slot>,
    class: &'static str,
    title: impl Into<Signal<String>>,
    width: Option<Memo<usize>>,
    lead: Option<Memo<String>>,
) -> impl IntoView {
    let title = title.into();
    let shown = Memo::new(move |_| slot.with(|slot| *slot != Slot::Hidden));
    let reserved = Memo::new(move |_| slot.with(|slot| *slot == Slot::Reserved));
    let style = move || {
        width
            .map(|width| format!("--item-width: {}ch", width.get()))
            .unwrap_or_default()
    };
    let parts = move || {
        let text = slot.with(Slot::text);
        let lead = lead.map(|lead| lead.get()).unwrap_or_default();
        let (start, rest) = split_lead(&text, &lead);
        (start.to_owned(), rest.to_owned())
    };
    let lead_text = move || parts().0;
    let rest_text = move || parts().1;
    move || {
        shown.get().then(|| {
            view! {
                <span
                    class=class
                    style=style
                    title=title
                    aria-hidden=move || reserved.get().then_some("true")
                >
                    {lead.map(|_| view! { <span class="max-sm:hidden">{lead_text}</span> })}
                    {rest_text}
                </span>
            }
        })
    }
}

/// `text` as its `lead` and the rest when it starts with `lead`, else as nothing and all of it.
fn split_lead<'a>(text: &'a str, lead: &str) -> (&'a str, &'a str) {
    text.strip_prefix(lead)
        .map_or(("", text), |rest| (&text[..lead.len()], rest))
}

/// The status bar.
#[component]
pub fn StatusBar() -> impl IntoView {
    let state = AppState::expect();
    let map = MapState::expect();
    let rooms = MapRooms::expect();
    let limits = RateLimitsUi::expect();
    let tick = TickUi::expect();
    let items = Memo::new(move |_| {
        MapItems::of(&Facts {
            map: map.phase.get() != Phase::Idle,
            shard: map
                .shard
                .with(|shard| shard.as_ref().and_then(Shard::name).map(str::to_owned)),
            game_time: map.game_time.get(),
            tick: tick.duration.get(),
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
    // The full text first: the item cuts it on a narrow window.
    let place_title = Memo::new(move |_| {
        let about = tick.duration.with(|tick| place_title(tick.as_ref()));
        place.with(|place| match place {
            Slot::Shown(text) => format!("{text}\n{about}"),
            Slot::Hidden | Slot::Reserved => about,
        })
    });
    // The shard that starts the tick item's text: phones leave it out.
    let shard_lead = Memo::new(move |_| {
        map.shard.with(|shard| {
            shard
                .as_ref()
                .and_then(Shard::name)
                .map(|name| format!("{name}{SEPARATOR}"))
                .unwrap_or_default()
        })
    });
    // Known with the session (the server's shards) and the route (the shard shown), before
    // the first tick.
    let place_width = Memo::new(move |_| {
        map.shard.with(|shard| {
            let shown = shard.as_ref().and_then(Shard::name);
            map.shards.with(|shards| place_width(shards, shown))
        })
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
    // The full progress after what it is: the item cuts it on a narrow window.
    let title = Memo::new(move |_| {
        let about = map
            .progress
            .with(|progress| crate::feed::progress_title(progress.as_ref()));
        progress.with(|label| match label {
            Some(label) => format!("{about}\n{label}"),
            None => about,
        })
    });
    let progress = move || {
        loading.get().then(|| {
            view! {
                <span class=PROGRESS role="status" title=move || title.get()>
                    <span class=style::SPINNER_SMALL aria-hidden="true"></span>
                    <span class="truncate">{move || progress.get().unwrap_or_default()}</span>
                </span>
                <StatusReset />
            }
        })
    };
    view! {
        <footer class=BAR aria-label="Status">
            <span class=CORE>
                <span
                    class="inline-flex flex-none items-center gap-2 min-w-[9.5em] max-sm:min-w-0"
                    title=connection_title
                >
                    <span class=move || status_class(state.status.get()) aria-hidden="true"></span>
                    {move || connection_text(state.status.get())}
                </span>
                {item(place, PLACE, place_title, Some(place_width), Some(shard_lead))}
                {item(
                    level,
                    "flex-none min-w-[4.5em] max-sm:min-w-0 text-fg",
                    "Zoom level",
                    None,
                    None,
                )}
            </span>
            {item(
                hover,
                "inline-flex flex-none items-center min-w-[6.5em] text-fg max-sm:hidden \
                    pointer-coarse:hidden",
                "Room under the pointer",
                None,
                None,
            )}
            {progress}
            <StatusLifted />
            <StatusUsage />
            <span class=style::FIT_END aria-hidden="true"></span>
            {item(
                frame,
                "inline-flex flex-none items-center min-w-[7.5em]",
                "CPU time of the last frame",
                None,
                None,
            )}
            {item(backend, "inline-flex flex-none items-center", "Graphics backend", None, None)}
            <span class="inline-flex flex-none items-center">
                {move || {
                    if state.platform.with_value(Platform::is_tauri) { "Desktop" } else { "Web" }
                }}
            </span>
        </footer>
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use overseer::ConnectionStatus;
    use overseer_view::Lod;

    use super::{
        Facts, MapItems, Slot, connection_text, level_name, place_title, place_width, split_lead,
    };
    use overseer::map::{TickSource as Source, TickSpeed};

    /// A tick duration of `ms` from `source`.
    const fn duration(ms: f64, source: Source) -> TickSpeed {
        TickSpeed { ms, source }
    }

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
    fn the_tick_slot_adds_the_tick_duration() {
        let timed = Facts {
            map: true,
            shard: Some("shard3".to_owned()),
            game_time: Some(1234),
            tick: Some(duration(3104.0, Source::Server)),
            ..Facts::default()
        };
        assert_eq!(
            MapItems::of(&timed).place,
            Slot::Shown("shard3 · tick 1234 · 3.1 s/tick".to_owned())
        );
        // The server's duration can come before the first live room.
        let no_room = Facts {
            game_time: None,
            ..timed
        };
        assert_eq!(
            MapItems::of(&no_room).place,
            Slot::Shown("shard3 · 3.1 s/tick".to_owned())
        );
        // The tooltip says where the duration comes from.
        assert_eq!(place_title(None), "Game tick");
        assert_eq!(
            place_title(Some(&duration(3100.0, Source::Server))),
            "Game tick and tick duration: 3.10 s per tick (0.32 ticks per second), reported by \
             the server"
        );
        assert_eq!(
            place_title(Some(&duration(250.0, Source::Measured))),
            "Game tick and tick duration: 0.25 s per tick (4.00 ticks per second), measured from \
             game time"
        );
    }

    #[test]
    fn the_tick_slot_fits_the_longest_shard_a_nine_digit_tick_and_a_duration() {
        // Single world: `tick 123456789 · 250 ms/tick`.
        assert_eq!(
            place_width(&[], None),
            "tick 123456789 · 250 ms/tick".chars().count()
        );
        // screeps.com: `shard0 · tick 123456789 · 250 ms/tick`, whichever shard is shown.
        let shards = ["shard0", "shard1", "shard2", "shard3"].map(str::to_owned);
        let widest = "shard0 · tick 123456789 · 250 ms/tick".chars().count();
        assert_eq!(place_width(&shards, Some("shard2")), widest);
        assert_eq!(place_width(&shards, None), widest);
        // A longer name (a season's) widens it; a shown shard counts before the list arrives.
        assert_eq!(
            place_width(&[], Some("shardSeason")),
            "shardSeason · tick 123456789 · 250 ms/tick".chars().count()
        );
    }

    #[test]
    fn phones_leave_out_the_shard_that_leads_the_tick_slot() {
        assert_eq!(
            split_lead("shard3 · tick 1234", "shard3 · "),
            ("shard3 · ", "tick 1234")
        );
        // No shard, or a text that does not start with it: all of it stays.
        assert_eq!(split_lead("tick 1234", ""), ("", "tick 1234"));
        assert_eq!(split_lead("tick 1234", "shard3 · "), ("", "tick 1234"));
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
