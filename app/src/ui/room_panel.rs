//! The room control: one room in its own viewer ([`Scope::Room`]) with its overview, live
//! state and the selected tile's objects. The drawer shows it small next to the map, a
//! popped-out window shows it alone; both feed it from a [`Feed`] (the map's, or the
//! window's own).
//!
//! The control subscribes to its room only; a live room that the server refuses (the
//! per-player limit of live rooms counts every window) shows the minimap with the reason.

use std::rc::Rc;
use std::sync::Arc;

use leptos::prelude::*;
use overseer::RoomSnapshot;
use overseer::core::api::types::GameRoomObject;
use overseer::map::{DetailState, RoomCoord, RoomStat, RoomStatus, WorldEvent};
use overseer_view::{Scope, Viewer};

use crate::feed::{Feed, Reach};
use crate::ui::surface::{Hooks, Surface, SurfaceCanvas};
use crate::ui::world::MapState;

/// What the control knows of its room.
#[derive(Clone, Copy)]
pub struct RoomData {
    /// The overview (owner, level, status).
    pub stat: RwSignal<Option<Arc<RoomStat>>, LocalStorage>,
    /// The live state.
    pub detail: RwSignal<Option<DetailState>, LocalStorage>,
}

impl RoomData {
    /// Nothing known yet.
    pub fn new() -> Self {
        Self {
            stat: RwSignal::new_local(None),
            detail: RwSignal::new_local(None),
        }
    }

    /// The live snapshot, if the room is live.
    fn snapshot(self) -> Option<Arc<RoomSnapshot>> {
        self.detail.with(|detail| match detail {
            Some(DetailState::Live(snapshot)) => Some(Arc::clone(snapshot)),
            _ => None,
        })
    }
}

/// The room control: info, the room's viewer and the selected tile's objects.
#[component]
pub fn RoomPanel(
    /// The world the room is in.
    feed: Feed,
    /// The room shown; `None` shows the empty state.
    #[prop(into)]
    room: Signal<Option<RoomCoord>>,
    /// The selected tile (shared with the map in the drawer).
    tile: RwSignal<Option<(RoomCoord, u8, u8)>>,
    /// The signed-in user id (own vs other colours).
    user: Option<String>,
    /// Laid out to fill a window (pop-out) instead of the drawer.
    #[prop(optional)]
    full: bool,
) -> impl IntoView {
    let data = RoomData::new();
    let has_room = move || room.with(Option::is_some);
    // The feed is not `Send`; the children of `Show` must be.
    let feed = StoredValue::new_local(feed);
    view! {
        <Show
            when=has_room
            fallback=|| {
                view! {
                    <p class="empty-state">
                        "Zoom into a room or click one on the map to see it here."
                    </p>
                }
            }
        >
            <div class=if full { "room-panel full" } else { "room-panel" }>
                <RoomControl feed=feed.get_value() room tile user=user.clone() data report=full />
                <div class="room-side">
                    <RoomInfo room data />
                    <Inspector room tile data />
                </div>
            </div>
        </Show>
    }
}

/// The room's viewer.
#[component]
fn RoomControl(
    feed: Feed,
    room: Signal<Option<RoomCoord>>,
    tile: RwSignal<Option<(RoomCoord, u8, u8)>>,
    user: Option<String>,
    data: RoomData,
    /// Reports frame time and backend to the status bar (the control is the window's view).
    report: bool,
) -> impl IntoView {
    let map = MapState::expect();
    let surface = Surface::new(Hooks {
        ready: Rc::new(move |viewer: &mut Viewer, ()| {
            viewer.set_user(user.clone());
            viewer.set_mode(map.mode.get_untracked());
        }),
        frame: Rc::new(
            move |viewer: &mut Viewer, stats: overseer_view::FrameStats| {
                if report {
                    map.frame_ms.set(Some(stats.cpu_ms));
                    let lod = Some(viewer.lod());
                    if map.lod.get_untracked() != lod {
                        map.lod.set(lod);
                    }
                }
            },
        ),
        pick: Rc::new(move |viewer: &mut Viewer, pick| {
            let selected = Some((pick.room, pick.x, pick.y));
            tile.set(selected);
            viewer.select(selected);
        }),
    });
    // Follow the room: a new scope and a subscription to that room alone.
    Effect::new(move |_| {
        let room = room.get();
        if !surface.ready.get() {
            return;
        }
        data.stat.set(None);
        data.detail.set(None);
        let Some(room) = room else {
            surface.attach(None);
            return;
        };
        surface.with_viewer(|viewer| viewer.set_scope(Scope::Room(room)));
        let subscription = feed.subscribe(Reach::Room(room), move |event| {
            deliver(surface, data, map, room, event);
        });
        surface.attach(Some(subscription));
    });
    if report {
        Effect::new(move |_| map.backend.set(surface.backend.get()));
    }
    Effect::new(move |_| {
        let mode = map.mode.get();
        surface.with_viewer(|viewer| viewer.set_mode(mode));
    });
    Effect::new(move |_| {
        let tile = tile.get();
        surface.with_viewer(|viewer| viewer.select(tile));
    });
    let label = Signal::derive(move || {
        room.get()
            .map_or_else(|| "Room".to_owned(), |room| format!("Room {room}"))
    });
    let refused = move || {
        data.detail.with(|detail| match detail {
            Some(DetailState::Refused(reason)) => Some(reason.clone()),
            _ => None,
        })
    };
    view! {
        <div class="room-control">
            <SurfaceCanvas
                surface
                testid="room-control"
                label
                description="Drag to pan (or orbit in 3D), scroll or pinch to zoom, click a tile to list its objects."
            />
            {move || {
                surface
                    .error
                    .get()
                    .map(|error| {
                        view! {
                            <div class="overlay" role="alert">
                                <p>{format!("The room cannot be drawn: {error}")}</p>
                            </div>
                        }
                    })
            }}
            {move || {
                refused()
                    .map(|reason| {
                        view! {
                            <p class="room-notice" role="status">
                                {format!(
                                    "Live view unavailable ({reason}): the server allows few live rooms per player, across all windows. Showing the minimap."
                                )}
                            </p>
                        }
                    })
            }}
        </div>
    }
}

/// Applies `event` when it concerns `room`, and keeps `data` (and the map's tick) up to date.
fn deliver(surface: Surface, data: RoomData, map: MapState, room: RoomCoord, event: &WorldEvent) {
    match event {
        WorldEvent::Stats(stats) => {
            if let Some((_, stat)) = stats.iter().find(|(at, _)| *at == room) {
                surface.with_viewer(|viewer| viewer.set_stat(room, Some(stat)));
                data.stat.try_set(Some(Arc::clone(stat)));
            }
        }
        WorldEvent::Terrain(at, _)
        | WorldEvent::NoTerrain(at)
        | WorldEvent::Minimap(at, _)
        | WorldEvent::MinimapGone(at)
            if *at == room =>
        {
            surface.apply(event);
        }
        WorldEvent::Detail(at, state) if *at == room => {
            surface.apply(event);
            data.detail.try_set(Some(state.clone()));
            if let DetailState::Live(snapshot) = state
                && snapshot.game_time.is_some()
            {
                map.game_time.try_set(snapshot.game_time);
            }
        }
        WorldEvent::DetailGone(at) if *at == room => {
            surface.apply(event);
            data.detail.try_set(None);
        }
        _ => {}
    }
}

/// Owner, level, reservation, status and live state of the room.
#[component]
fn RoomInfo(room: Signal<Option<RoomCoord>>, data: RoomData) -> impl IntoView {
    let title = move || room.get().map(|room| room.to_string()).unwrap_or_default();
    let rows = move || {
        let Some(stat) = data.stat.get() else {
            return view! { <p class="muted" role="status">"Loading the room's overview…"</p> }
                .into_any();
        };
        let owner = stat.owner.as_ref().map_or_else(
            || "Nobody".to_owned(),
            |owner| {
                format!(
                    "{} (level {})",
                    owner.username.clone().unwrap_or_else(|| owner.id.clone()),
                    owner.level
                )
            },
        );
        let reservation = stat.reservation.as_ref().map(|reservation| {
            reservation
                .username
                .clone()
                .unwrap_or_else(|| reservation.id.clone())
        });
        let mineral = stat.mineral.as_ref().map(|mineral| {
            stat.mineral_density.map_or_else(
                || mineral.clone(),
                |density| format!("{mineral} (density {density})"),
            )
        });
        view! {
            <dl class="stats">
                <dt>"Status"</dt>
                <dd>{status_text(&stat)}</dd>
                <dt>"Owner"</dt>
                <dd>{owner}</dd>
                {reservation
                    .map(|name| view! { <dt>"Reserved by"</dt><dd>{name}</dd> })}
                {stat.safe_mode.then(|| view! { <dt>"Safe mode"</dt><dd>"Active"</dd> })}
                {mineral.map(|mineral| view! { <dt>"Mineral"</dt><dd>{mineral}</dd> })}
            </dl>
        }
        .into_any()
    };
    let live = move || {
        data.detail.with(|detail| match detail {
            Some(DetailState::Live(_)) => ("dot ok", "Live".to_owned()),
            Some(DetailState::Pending) => ("dot busy", "Connecting…".to_owned()),
            Some(DetailState::Refused(_)) => ("dot off", "Minimap only".to_owned()),
            None => ("dot off", "Not live".to_owned()),
        })
    };
    view! {
        <section class="room-info" aria-labelledby="room-info-title">
            <h2 id="room-info-title">
                {title}
                <span class="live" role="status">
                    <span class=move || live().0 aria-hidden="true"></span>
                    {move || live().1}
                </span>
            </h2>
            {rows}
        </section>
    }
}

/// The room's status in words.
fn status_text(stat: &RoomStat) -> String {
    let until = stat
        .status_until
        .map(|until| format!(" until {}", date(until)))
        .unwrap_or_default();
    match stat.status {
        RoomStatus::Normal => "Open".to_owned(),
        RoomStatus::Novice => format!("Novice area{until}"),
        RoomStatus::Respawn => format!("Respawn area{until}"),
        RoomStatus::Closed => format!("Closed{until}"),
        RoomStatus::OutOfBorders => "Out of borders".to_owned(),
    }
}

/// A date from milliseconds since the epoch, in the user's locale.
#[expect(clippy::cast_precision_loss, reason = "dates are far below 2^53 ms")]
fn date(milliseconds: u64) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(milliseconds as f64));
    String::from(date.to_locale_date_string("default", &wasm_bindgen::JsValue::UNDEFINED))
}

/// The objects on the selected tile of the room.
#[component]
fn Inspector(
    room: Signal<Option<RoomCoord>>,
    tile: RwSignal<Option<(RoomCoord, u8, u8)>>,
    data: RoomData,
) -> impl IntoView {
    let content = move || {
        let selected = tile.get().filter(|(at, _, _)| Some(*at) == room.get());
        let Some((_, x, y)) = selected else {
            return view! { <p class="muted">"Click a tile of the live room to list its objects."</p> }
                .into_any();
        };
        let Some(snapshot) = data.snapshot() else {
            return view! { <p class="muted">"The room is not live: its objects are unknown."</p> }
                .into_any();
        };
        let objects: Vec<&GameRoomObject> = snapshot
            .objects
            .iter()
            .filter(|object| object.x() == u64::from(x) && object.y() == u64::from(y))
            .collect();
        let count = match objects.len() {
            0 => "No objects".to_owned(),
            1 => "1 object".to_owned(),
            n => format!("{n} objects"),
        };
        let items = objects
            .into_iter()
            .map(|object| {
                let owner = object.user().map(|user| {
                    snapshot
                        .users
                        .get(user)
                        .map_or_else(|| user.to_owned(), |known| known.username.clone())
                });
                let json = serde_json::to_string_pretty(object).unwrap_or_default();
                let title = format!("{} at {x},{y}", object.kind());
                view! {
                    <li class="object">
                        <strong>{title}</strong>
                        <span class="muted">
                            {owner.map_or_else(|| "no owner".to_owned(), |owner| format!("owner {owner}"))}
                        </span>
                        <details>
                            <summary>{format!("JSON of {}", object.id())}</summary>
                            <pre>{json}</pre>
                        </details>
                    </li>
                }
            })
            .collect_view();
        view! {
            <h3>{format!("Tile {x},{y}")}</h3>
            <p role="status">{count}</p>
            <ul class="object-list">{items}</ul>
        }
        .into_any()
    };
    view! {
        <section class="inspector" aria-label="Selected tile">
            {content}
        </section>
    }
}
