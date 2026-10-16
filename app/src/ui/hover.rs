//! The room under a hovering pointer: the tooltip next to it on the map ([`HoverTip`], text
//! by [`tip`]) and the status bar's room item ([`place_text`]).
//!
//! The world screen learns the hovered point from its surface (at most once per frame, never
//! while dragging or touching), asks the viewer which room and tile are there
//! (`Viewer::room_at`), outlines that room (`Viewer::set_hover`) and keeps it in
//! [`MapRooms::hover`]; what the tooltip says comes from what the world knows of the room
//! (`Feed::knowledge`, followed by its revision): the same overview and portals as the
//! map's and the drawer's.

use leptos::prelude::*;
use overseer::map::{Portal, RoomCoord, RoomStat, RoomStatus};
use overseer_view::Lod;

use crate::portals::{destination_text, destinations};
use crate::ui::surface::Surface;
use crate::ui::world::{MapState, Phase};

/// Distance of the tooltip from the pointer, in CSS pixels.
const OFFSET: f32 = 16.0;
/// Room the tooltip needs to the right of and below the pointer before it flips, in CSS
/// pixels.
const ROOM: (f32, f32) = (260.0, 160.0);

/// The room and tile under a hovering pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hovered {
    /// The room.
    pub room: RoomCoord,
    /// The tile in it.
    pub tile: (u8, u8),
    /// Where the pointer is, in CSS pixels of the map.
    pub point: (f32, f32),
}

/// The hovered room; `Copy`, provided as context by [`crate::ui::App`] next to [`MapState`].
#[derive(Clone, Copy)]
pub struct MapRooms {
    /// The room and tile under a hovering pointer (the tooltip and the status bar).
    pub hover: RwSignal<Option<Hovered>>,
}

impl MapRooms {
    /// Nothing hovered.
    pub fn new() -> Self {
        Self {
            hover: RwSignal::new(None),
        }
    }

    /// The state provided by [`crate::ui::App`].
    pub fn expect() -> Self {
        expect_context()
    }

    /// Forgets the hovered room (another world).
    pub fn clear(self) {
        self.hover.try_set(None);
    }
}

/// What the tooltip says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tip {
    /// The room's name.
    pub room: String,
    /// The tile, when zoomed in far enough for tiles to matter (`25,17`).
    pub tile: Option<String>,
    /// Owner and level, reservation, status, safe mode, portal destinations.
    pub rows: Vec<String>,
}

/// The tooltip of `room` (and `tile`, when shown) with its overview `stat` and `portals` as
/// known, seen from `shard` (the shard shown) at `now` (milliseconds since the epoch).
pub fn tip(
    room: RoomCoord,
    tile: Option<(u8, u8)>,
    stat: Option<&RoomStat>,
    portals: Option<&[Portal]>,
    shard: Option<&str>,
    now: u64,
) -> Tip {
    let mut rows = Vec::new();
    if let Some(stat) = stat {
        let name =
            |username: &Option<String>, id: &str| username.clone().unwrap_or_else(|| id.to_owned());
        if let Some(owner) = &stat.owner {
            let name = name(&owner.username, &owner.id);
            rows.push(if owner.level > 0 {
                format!("{name} · RCL {}", owner.level)
            } else {
                name
            });
        }
        if let Some(reservation) = &stat.reservation {
            rows.push(format!(
                "Reserved by {}",
                name(&reservation.username, &reservation.id)
            ));
        }
        let until = stat
            .status_until
            .filter(|until| *until > now)
            .map(|until| format!(" · ends in {}", remaining(until - now)))
            .unwrap_or_default();
        match stat.status {
            RoomStatus::Normal => {}
            RoomStatus::Novice => rows.push(format!("Novice area{until}")),
            RoomStatus::Respawn => rows.push(format!("Respawn area{until}")),
            RoomStatus::Closed => rows.push(format!("Closed{until}")),
            RoomStatus::OutOfBorders => rows.push("Outside the world".to_owned()),
        }
        if stat.safe_mode {
            rows.push("Safe mode".to_owned());
        }
    }
    for found in destinations(portals.unwrap_or_default()) {
        rows.push(format!(
            "Portal → {}",
            destination_text(found.to.as_ref(), shard)
        ));
    }
    Tip {
        room: room.to_string(),
        tile: tile.map(|(x, y)| format!("{x},{y}")),
        rows,
    }
}

/// A duration in milliseconds, roughly: `3 d`, `5 h`, `12 min`, `under a minute`.
pub fn remaining(milliseconds: u64) -> String {
    const MINUTE: u64 = 60_000;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    match milliseconds {
        ms if ms >= DAY => format!("{} d", ms / DAY),
        ms if ms >= HOUR => format!("{} h", ms / HOUR),
        ms if ms >= MINUTE => format!("{} min", ms / MINUTE),
        _ => "under a minute".to_owned(),
    }
}

/// Whether tiles matter at `lod`: rooms are drawn large enough to tell them apart.
pub fn shows_tiles(lod: Option<Lod>) -> bool {
    lod.is_some_and(|lod| lod >= Lod::Minimap)
}

/// The status bar's room item for `hovered` at `lod`: `W3N4`, with the tile once tiles
/// matter (`W3N4 25,17`).
pub fn place_text(hovered: &Hovered, lod: Option<Lod>) -> String {
    if shows_tiles(lod) {
        format!("{} {},{}", hovered.room, hovered.tile.0, hovered.tile.1)
    } else {
        hovered.room.to_string()
    }
}

/// Where the tooltip goes for the pointer at `point` on a map of `size` CSS pixels: its
/// `left` and `top` and the CSS transform that flips it left of or above the pointer when
/// it would leave the map.
pub fn placement(point: (f32, f32), size: (f32, f32)) -> (f32, f32, &'static str) {
    let flip_x = point.0 + ROOM.0 > size.0 && point.0 > size.0 / 2.0;
    let flip_y = point.1 + ROOM.1 > size.1 && point.1 > size.1 / 2.0;
    let left = if flip_x {
        point.0 - OFFSET
    } else {
        point.0 + OFFSET
    };
    let top = if flip_y {
        point.1 - OFFSET
    } else {
        point.1 + OFFSET
    };
    let transform = match (flip_x, flip_y) {
        (false, false) => "none",
        (true, false) => "translateX(-100%)",
        (false, true) => "translateY(-100%)",
        (true, true) => "translate(-100%, -100%)",
    };
    (left, top, transform)
}

/// The tooltip of the hovered room, over the map.
#[component]
pub fn HoverTip() -> impl IntoView {
    let map = MapState::expect();
    let rooms = MapRooms::expect();
    let shown =
        Memo::new(move |_| map.phase.get() == Phase::Ready && rooms.hover.with(Option::is_some));
    let content = Memo::new(move |_| {
        let hovered = rooms.hover.get()?;
        let lod = map.lod.get();
        let shard = map.shard.get();
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the epoch time in milliseconds is a non-negative integer far below u64::MAX"
        )]
        let now = js_sys::Date::now() as u64;
        let (stat, portals) = map
            .knowledge(|knowledge| {
                (
                    knowledge.stat(hovered.room),
                    knowledge.portals(hovered.room),
                )
            })
            .unwrap_or_default();
        Some(tip(
            hovered.room,
            shows_tiles(lod).then_some(hovered.tile),
            stat.as_deref(),
            portals.as_deref(),
            shard.as_ref().and_then(|shard| shard.name()),
            now,
        ))
    });
    let style = move || {
        let hovered = rooms.hover.get()?;
        let size = map
            .surface
            .try_get_value()
            .flatten()
            .map_or((f32::MAX, f32::MAX), Surface::css_size);
        let (left, top, transform) = placement(hovered.point, size);
        Some(format!(
            "left: {left:.0}px; top: {top:.0}px; transform: {transform};"
        ))
    };
    move || {
        shown.get().then(|| {
            view! {
                <div
                    class="absolute z-20 max-w-72 px-3 py-2 text-xs leading-tight text-fg pointer-events-none bg-panel-3 border border-edge-strong rounded-sm shadow-sm"
                    role="tooltip"
                    data-testid="map-tooltip"
                    style=style
                >
                    {move || {
                        content
                            .get()
                            .map(|tip| {
                                let rows = tip
                                    .rows
                                    .into_iter()
                                    .map(|row| view! { <li>{row}</li> })
                                    .collect_view();
                                view! {
                                    <p class="flex items-baseline gap-2 text-sm font-medium tabular-nums">
                                        <span>{tip.room}</span>
                                        {tip.tile.map(|tile| view! { <span class="font-normal text-muted">{tile}</span> })}
                                    </p>
                                    <ul class="text-muted not-empty:mt-1">{rows}</ul>
                                }
                            })
                    }}
                </div>
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use overseer::map::{
        Portal, PortalDestination, RoomCoord, RoomOwner, RoomReservation, RoomStat, RoomStatus,
    };
    use overseer_view::Lod;

    use super::{Hovered, Tip, place_text, placement, remaining, tip};

    fn room(name: &str) -> RoomCoord {
        name.parse().unwrap()
    }

    const NOW: u64 = 1_800_000_000_000;

    fn stat() -> RoomStat {
        RoomStat {
            status: RoomStatus::Novice,
            status_until: Some(NOW + 3 * 24 * 3_600_000 + 5),
            owner: Some(RoomOwner {
                id: "u1".to_owned(),
                username: Some("Alice".to_owned()),
                level: 7,
                badge: None,
            }),
            reservation: None,
            safe_mode: true,
            mineral: None,
            mineral_density: None,
            fetched_at: NOW,
        }
    }

    fn portal(to: &str, shard: Option<&str>) -> Portal {
        Portal {
            room: room("W5N5"),
            x: 20,
            y: 20,
            destination: Some(PortalDestination {
                room: room(to),
                shard: shard.map(str::to_owned),
                x: None,
                y: None,
            }),
            expires_at: None,
            decay_time: None,
        }
    }

    #[test]
    fn the_tooltip_names_owner_status_and_portals() {
        let portals = [
            portal("W0N0", None),
            portal("W0N0", None),
            portal("E10S20", Some("shard2")),
            Portal::pending(room("W5N5"), 30, 30),
        ];
        assert_eq!(
            tip(
                room("W5N5"),
                Some((25, 17)),
                Some(&stat()),
                Some(&portals[..]),
                Some("shard3"),
                NOW
            ),
            Tip {
                room: "W5N5".to_owned(),
                tile: Some("25,17".to_owned()),
                rows: vec![
                    "Alice · RCL 7".to_owned(),
                    "Novice area · ends in 3 d".to_owned(),
                    "Safe mode".to_owned(),
                    "Portal → W0N0".to_owned(),
                    "Portal → shard2 E10S20".to_owned(),
                    // A destination not fetched yet.
                    "Portal → …".to_owned(),
                ],
            }
        );
    }

    #[test]
    fn reservations_and_unknown_rooms() {
        let reserved = RoomStat {
            status: RoomStatus::Normal,
            status_until: None,
            owner: None,
            reservation: Some(RoomReservation {
                id: "u2".to_owned(),
                username: None,
                badge: None,
            }),
            safe_mode: false,
            ..stat()
        };
        assert_eq!(
            tip(
                room("W1N1"),
                None,
                Some(&reserved),
                Some(&[][..]),
                None,
                NOW
            )
            .rows,
            ["Reserved by u2"]
        );
        // Nothing known yet: the name alone.
        assert_eq!(
            tip(room("W1N1"), None, None, None, None, NOW),
            Tip {
                room: "W1N1".to_owned(),
                tile: None,
                rows: Vec::new(),
            }
        );
        let outside = RoomStat {
            status: RoomStatus::OutOfBorders,
            owner: None,
            safe_mode: false,
            status_until: None,
            ..stat()
        };
        assert_eq!(
            tip(room("W1N1"), None, Some(&outside), None, None, NOW).rows,
            ["Outside the world"]
        );
        // An end in the past is not shown.
        let ended = RoomStat {
            owner: None,
            safe_mode: false,
            status: RoomStatus::Respawn,
            status_until: Some(NOW - 1),
            ..stat()
        };
        assert_eq!(
            tip(room("W1N1"), None, Some(&ended), None, None, NOW).rows,
            ["Respawn area"]
        );
    }

    #[test]
    fn durations_read_roughly() {
        assert_eq!(remaining(59_999), "under a minute");
        assert_eq!(remaining(60_000), "1 min");
        assert_eq!(remaining(3 * 3_600_000 + 1), "3 h");
        assert_eq!(remaining(2 * 86_400_000), "2 d");
    }

    #[test]
    fn the_status_item_shows_the_tile_once_tiles_matter() {
        let hovered = Hovered {
            room: room("W3N4"),
            tile: (25, 17),
            point: (1.0, 2.0),
        };
        assert_eq!(place_text(&hovered, Some(Lod::Overview)), "W3N4");
        assert_eq!(place_text(&hovered, Some(Lod::Terrain)), "W3N4");
        assert_eq!(place_text(&hovered, Some(Lod::Minimap)), "W3N4 25,17");
        assert_eq!(place_text(&hovered, Some(Lod::Detail)), "W3N4 25,17");
        assert_eq!(place_text(&hovered, None), "W3N4");
    }

    #[test]
    fn the_tooltip_flips_near_the_edges() {
        let size = (1000.0, 600.0);
        assert_eq!(placement((100.0, 100.0), size), (116.0, 116.0, "none"));
        assert_eq!(
            placement((900.0, 100.0), size),
            (884.0, 116.0, "translateX(-100%)")
        );
        assert_eq!(
            placement((100.0, 550.0), size),
            (116.0, 534.0, "translateY(-100%)")
        );
        assert_eq!(
            placement((900.0, 550.0), size),
            (884.0, 534.0, "translate(-100%, -100%)")
        );
    }
}
