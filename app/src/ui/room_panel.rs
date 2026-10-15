//! The room control: one room in its own viewer ([`Scope::Room`], [`RoomPreview`]), what is
//! known of it ([`RoomData`], kept live by [`RoomData::watch`]) and its facts ([`RoomInfo`]:
//! status, owner and controller level with progress, safe mode, reservation, energy for
//! spawning, sources, mineral, creeps and structures by owner, last update).
//!
//! The drawer shows the preview and facts on its Room tab; a popped-out room window shows the
//! whole [`RoomPanel`] (preview, facts and the room's objects). A live room that the server
//! refuses (the per-player limit of live rooms counts every window) shows the minimap with
//! the reason.

use std::rc::Rc;
use std::sync::Arc;

use leptos::prelude::*;
use overseer::RoomSnapshot;
use overseer::core::api::types::GameRoomObject;
use overseer::map::{DetailState, Interest, RoomCoord, RoomRect, RoomStat, RoomStatus, WorldEvent};
use overseer_view::{Scope, Viewer};

use crate::feed::{Feed, Reach, Subscription};
use crate::ui::icon::{Icon, IconKind};
use crate::ui::inspector::{Context, number, ticks};
use crate::ui::objects::{ObjectsPanel, Selection};
use crate::ui::surface::{Hooks, Surface, SurfaceCanvas};
use crate::ui::world::MapState;

/// What is known of the room in focus.
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

    /// The live snapshot, if the room is live (tracked).
    pub fn snapshot(self) -> Option<Arc<RoomSnapshot>> {
        self.detail.with(|detail| match detail {
            Some(DetailState::Live(snapshot)) => Some(Arc::clone(snapshot)),
            _ => None,
        })
    }

    /// Keeps the data at `room` of `feed` (both reactive): its overview and live state,
    /// asked of the world for as long as the calling component lives. Live updates also move
    /// the map's tick.
    pub fn watch(
        self,
        feed: impl Fn() -> Option<Feed> + 'static,
        room: Signal<Option<RoomCoord>>,
        map: MapState,
    ) {
        let subscription: StoredValue<Option<Subscription>, LocalStorage> =
            StoredValue::new_local(None);
        // The same room set again keeps the subscription (and the live state shown).
        let room = Memo::new(move |_| room.get());
        Effect::new(move |_| {
            let room = room.get();
            let feed = feed();
            subscription.set_value(None);
            self.stat.set(None);
            self.detail.set(None);
            let (Some(feed), Some(room)) = (feed, room) else {
                return;
            };
            let watched = feed.subscribe(Reach::Room(room), move |event| {
                self.deliver(map, room, event);
            });
            watched.set_interest(Interest {
                stats: Some(RoomRect::new(room, room)),
                terrain: Vec::new(),
                minimaps: Vec::new(),
                detail: vec![room],
            });
            subscription.set_value(Some(watched));
        });
    }

    /// Applies `event` when it concerns `room`.
    fn deliver(self, map: MapState, room: RoomCoord, event: &WorldEvent) {
        match event {
            WorldEvent::Stats(stats) => {
                if let Some((_, stat)) = stats.iter().find(|(at, _)| *at == room) {
                    self.stat.try_set(Some(Arc::clone(stat)));
                }
            }
            WorldEvent::Detail(at, state) if *at == room => {
                self.detail.try_set(Some(state.clone()));
                if let DetailState::Live(snapshot) = state
                    && snapshot.game_time.is_some()
                {
                    map.game_time.try_set(snapshot.game_time);
                }
            }
            WorldEvent::DetailGone(at) if *at == room => {
                self.detail.try_set(None);
            }
            _ => {}
        }
    }
}

/// A popped-out room: its viewer next to its facts and objects.
#[component]
pub fn RoomPanel(
    /// The world the room is in.
    feed: Feed,
    /// The room.
    #[prop(into)]
    room: Signal<Option<RoomCoord>>,
    /// What is known of the room.
    data: RoomData,
    /// The selection.
    selection: Selection,
    /// The signed-in user id.
    user: Option<String>,
) -> impl IntoView {
    let snapshot = Signal::derive(move || data.snapshot());
    view! {
        <div class="room-panel full">
            <RoomPreview feed room tile=selection.tile user=user.clone() data report=true />
            <div class="room-side">
                <RoomInfo room data user=user.clone() />
                <ObjectsPanel room snapshot selection user />
            </div>
        </div>
    }
}

/// The room's viewer.
#[component]
pub fn RoomPreview(
    /// The world the room is in.
    feed: Feed,
    /// The room shown.
    #[prop(into)]
    room: Signal<Option<RoomCoord>>,
    /// The selected tile (shared with the map).
    tile: RwSignal<Option<(RoomCoord, u8, u8)>>,
    /// The signed-in user id (own vs other colours).
    user: Option<String>,
    /// What is known of the room (the refusal notice).
    data: RoomData,
    /// Reports frame time and backend to the status bar (the preview is the window's view).
    #[prop(optional)]
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
    Effect::new(move |_| {
        let room = room.get();
        if !surface.ready.get() {
            return;
        }
        let Some(room) = room else {
            surface.attach(None);
            return;
        };
        surface.with_viewer(|viewer| viewer.set_scope(Scope::Room(room)));
        let subscription = feed.subscribe(Reach::Room(room), move |event| match event {
            WorldEvent::Stats(stats) => {
                if let Some((_, stat)) = stats.iter().find(|(at, _)| *at == room) {
                    surface.with_viewer(|viewer| viewer.set_stat(room, Some(stat)));
                }
            }
            WorldEvent::Terrain(at, _)
            | WorldEvent::NoTerrain(at)
            | WorldEvent::Minimap(at, _)
            | WorldEvent::MinimapGone(at)
            | WorldEvent::Detail(at, _)
            | WorldEvent::DetailGone(at)
                if *at == room =>
            {
                surface.apply(event);
            }
            _ => {}
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
                description="Drag to pan (or orbit in 3D), scroll or pinch to zoom, click a tile to select its object."
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

/// The controller as the facts show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControllerFacts {
    /// Owner's name.
    pub owner: Option<String>,
    /// Owned by the signed-in user.
    pub own: bool,
    /// Room control level.
    pub level: u64,
    /// `(progress, total)` towards the next level (none at level 8).
    pub progress: Option<(u64, u64)>,
    /// Ticks until it downgrades.
    pub downgrade_in: Option<u64>,
    /// Ticks of safe mode left.
    pub safe_mode_in: Option<u64>,
    /// Safe modes available.
    pub safe_modes: Option<u64>,
    /// Who reserves it, and for how many more ticks.
    pub reservation: Option<(String, Option<u64>)>,
}

/// Creeps and structures of one owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerCount {
    /// The owner's name.
    pub name: String,
    /// The signed-in user.
    pub own: bool,
    /// Creeps and power creeps.
    pub creeps: usize,
    /// Owned structures (construction sites apart).
    pub structures: usize,
    /// Construction sites.
    pub sites: usize,
}

/// What the live state says about the room.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// The controller.
    pub controller: Option<ControllerFacts>,
    /// Energy available for spawning and its capacity (spawns and extensions).
    pub energy: Option<(u64, u64)>,
    /// `(energy, capacity)` of each source, west to east.
    pub sources: Vec<(u64, u64)>,
    /// `(type, amount, density)` of the mineral.
    pub mineral: Option<(String, u64, u64)>,
    /// Per owner: the signed-in user first, then by name.
    pub owners: Vec<OwnerCount>,
}

/// The energy a spawn or extension holds and can hold.
fn spawn_energy(object: &GameRoomObject) -> Option<(u64, u64)> {
    let (store, capacity) = match object {
        GameRoomObject::Spawn(spawn) => (&spawn.store, &spawn.store_capacity_resource),
        GameRoomObject::Extension(extension) => {
            (&extension.store, &extension.store_capacity_resource)
        }
        _ => return None,
    };
    let energy = store
        .get("energy")
        .copied()
        .flatten()
        .and_then(|energy| u64::try_from(energy).ok())
        .unwrap_or_default();
    Some((
        energy,
        capacity
            .get("energy")
            .copied()
            .flatten()
            .unwrap_or_default(),
    ))
}

/// The facts of a room's `objects`.
pub fn facts(objects: &[GameRoomObject], context: &Context<'_>) -> Facts {
    let mut facts = Facts::default();
    let mut sources: Vec<(u64, u64, u64)> = Vec::new();
    for object in objects {
        if let Some((energy, capacity)) = spawn_energy(object) {
            let total = facts.energy.get_or_insert((0, 0));
            total.0 += energy;
            total.1 += capacity;
        }
        match object {
            GameRoomObject::Controller(controller) => {
                let user = controller.user.as_deref();
                facts.controller = Some(ControllerFacts {
                    owner: user.map(|user| context.name(user)),
                    own: context.is_me(user),
                    level: controller.level,
                    progress: controller
                        .progress
                        .zip(controller.progress_total)
                        .filter(|(_, total)| *total > 0),
                    downgrade_in: controller
                        .downgrade_time
                        .and_then(|tick| context.until(tick))
                        .filter(|_| user.is_some()),
                    safe_mode_in: controller.safe_mode.and_then(|tick| context.until(tick)),
                    safe_modes: controller.safe_mode_available.filter(|_| user.is_some()),
                    reservation: controller.reservation.as_ref().map(|reservation| {
                        (
                            context.name(&reservation.user),
                            context.until(reservation.end_time),
                        )
                    }),
                });
            }
            GameRoomObject::Source(source) => {
                sources.push((source.x, source.energy, source.energy_capacity));
            }
            GameRoomObject::Mineral(mineral) => {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "mineral amounts are small non-negative whole numbers"
                )]
                let amount = mineral.mineral_amount.max(0.0).round() as u64;
                facts.mineral = Some((mineral.mineral_type.clone(), amount, mineral.density));
            }
            _ => {}
        }
        let Some(user) = object.user() else {
            continue;
        };
        let counted = match object {
            GameRoomObject::Creep(_) | GameRoomObject::PowerCreep(_) => 0,
            GameRoomObject::ConstructionSite(_) => 2,
            GameRoomObject::Tombstone(_) | GameRoomObject::Ruin(_) | GameRoomObject::Other(_) => {
                continue;
            }
            _ => 1,
        };
        let name = context.name(user);
        let index = if let Some(index) = facts.owners.iter().position(|owner| owner.name == name) {
            index
        } else {
            facts.owners.push(OwnerCount {
                name,
                own: context.is_me(Some(user)),
                creeps: 0,
                structures: 0,
                sites: 0,
            });
            facts.owners.len() - 1
        };
        let owner = &mut facts.owners[index];
        match counted {
            0 => owner.creeps += 1,
            1 => owner.structures += 1,
            _ => owner.sites += 1,
        }
    }
    sources.sort_unstable();
    facts.sources = sources
        .into_iter()
        .map(|(_, energy, capacity)| (energy, capacity))
        .collect();
    facts
        .owners
        .sort_by(|a, b| b.own.cmp(&a.own).then_with(|| a.name.cmp(&b.name)));
    facts
}

/// `n things` with the plural.
fn counted(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

/// The room's name, live state and facts.
#[component]
pub fn RoomInfo(
    /// The room.
    #[prop(into)]
    room: Signal<Option<RoomCoord>>,
    /// What is known of it.
    data: RoomData,
    /// The signed-in user id.
    user: Option<String>,
    /// Pops the room out (the drawer).
    #[prop(optional)]
    pop_out: Option<Callback<()>>,
    /// Shown between the header and the facts (the drawer's room preview).
    #[prop(optional)]
    children: Option<Children>,
) -> impl IntoView {
    let user = StoredValue::new(user);
    let title = move || room.get().map(|room| room.to_string()).unwrap_or_default();
    let live = move || {
        data.detail.with(|detail| match detail {
            Some(DetailState::Live(_)) => ("dot ok", "Live".to_owned()),
            Some(DetailState::Pending) => ("dot busy", "Connecting…".to_owned()),
            Some(DetailState::Refused(_)) => ("dot off", "Minimap only".to_owned()),
            None => ("dot off", "Not live".to_owned()),
        })
    };
    let pop_out = pop_out.map(|pop_out| {
        view! {
            <button
                type="button"
                class="icon-button sm"
                aria-label="Pop out room"
                data-tip="Pop out room"
                data-tip-pos="bottom-end"
                on:click=move |_| pop_out.run(())
            >
                <Icon kind=IconKind::PopOut />
            </button>
        }
    });
    let overview = move || {
        let Some(stat) = data.stat.get() else {
            return view! { <p class="muted" role="status">"Loading the room's overview…"</p> }
                .into_any();
        };
        let live = data
            .detail
            .with(|detail| matches!(detail, Some(DetailState::Live(_))));
        overview_rows(&stat, live).into_any()
    };
    let live_facts = move || {
        let snapshot = data.snapshot()?;
        let me = user.get_value();
        let context = Context {
            game_time: snapshot.game_time,
            me: me.as_deref(),
            users: &snapshot.users,
        };
        Some(facts_rows(
            facts(&snapshot.objects, &context),
            snapshot.game_time,
        ))
    };
    view! {
        <section class="room-info" aria-labelledby="room-info-title">
            <header class="room-head">
                <h2 id="room-info-title">
                    <span class="room-name">{title}</span>
                    <span class="live" role="status">
                        <span class=move || live().0 aria-hidden="true"></span>
                        {move || live().1}
                    </span>
                </h2>
                {pop_out}
            </header>
            {children.map(|children| children())}
            <dl class="facts">
                {overview}
                {live_facts}
            </dl>
        </section>
    }
}

/// The overview's rows: status and owner, and while the room is not live, its reservation,
/// safe mode and mineral (the live rows tell more).
fn overview_rows(stat: &RoomStat, live: bool) -> impl IntoView + use<> {
    let owner = stat.owner.as_ref().map_or_else(
        || "Nobody".to_owned(),
        |owner| {
            format!(
                "{} · level {}",
                owner.username.clone().unwrap_or_else(|| owner.id.clone()),
                owner.level
            )
        },
    );
    let reservation = stat
        .reservation
        .as_ref()
        .filter(|_| !live)
        .map(|reservation| {
            reservation
                .username
                .clone()
                .unwrap_or_else(|| reservation.id.clone())
        });
    let mineral = stat.mineral.as_ref().filter(|_| !live).map(|mineral| {
        stat.mineral_density.map_or_else(
            || mineral.clone(),
            |density| format!("{mineral} · density {density}"),
        )
    });
    view! {
        <dt>"Status"</dt>
        <dd>{status_text(stat)}</dd>
        <dt>"Owner"</dt>
        <dd>{owner}</dd>
        {reservation.map(|name| view! { <dt>"Reserved by"</dt><dd>{name}</dd> })}
        {(stat.safe_mode && !live).then(|| view! { <dt>"Safe mode"</dt><dd>"Active"</dd> })}
        {mineral.map(|mineral| view! { <dt>"Mineral"</dt><dd>{mineral}</dd> })}
    }
}

/// The live state's rows.
fn facts_rows(facts: Facts, game_time: Option<u64>) -> impl IntoView {
    let controller = facts.controller.map(|controller| {
        let progress = controller.progress.map(|(progress, total)| {
            view! {
                <dt>"Upgrade"</dt>
                <dd>
                    <span class="meter progress">
                        <progress
                            max=total.to_string()
                            value=progress.min(total).to_string()
                            aria-label=format!("Progress to level {}", controller.level + 1)
                        ></progress>
                        <span class="meter-text">
                            {format!("{} / {}", number(progress), number(total))}
                        </span>
                    </span>
                </dd>
            }
        });
        let safe_mode = match (controller.safe_mode_in, controller.safe_modes) {
            (Some(left), _) => Some(format!("Active, ends in {}", ticks(left))),
            (None, Some(available)) => Some(format!("Off · {available} available")),
            (None, None) => None,
        };
        view! {
            {progress}
            {controller.downgrade_in.map(|left| view! {
                <dt>"Downgrade"</dt>
                <dd>{format!("in {}", ticks(left))}</dd>
            })}
            {safe_mode.map(|text| view! { <dt>"Safe mode"</dt><dd>{text}</dd> })}
            {controller.reservation.map(|(name, left)| {
                let text = match left {
                    Some(left) => format!("{name}, {} left", ticks(left)),
                    None => name,
                };
                view! { <dt>"Reservation"</dt><dd>{text}</dd> }
            })}
        }
    });
    let energy = facts.energy.map(|(energy, capacity)| {
        view! {
            <dt>"Energy"</dt>
            <dd>
                <span class="meter energy">
                    <progress
                        max=capacity.max(1).to_string()
                        value=energy.min(capacity).to_string()
                        aria-label="Energy available for spawning"
                    ></progress>
                    <span class="meter-text">{format!("{} / {}", number(energy), number(capacity))}</span>
                </span>
            </dd>
        }
    });
    let sources = (!facts.sources.is_empty()).then(|| {
        let count = facts.sources.len();
        let items = facts
            .sources
            .into_iter()
            .map(|(energy, capacity)| {
                view! {
                    <span class="meter energy">
                        <progress
                            max=capacity.max(1).to_string()
                            value=energy.min(capacity).to_string()
                            aria-label="Source energy"
                        ></progress>
                        <span class="meter-text">{format!("{} / {}", number(energy), number(capacity))}</span>
                    </span>
                }
            })
            .collect_view();
        view! {
            <dt>{if count == 1 { "Source" } else { "Sources" }}</dt>
            <dd class="stack">{items}</dd>
        }
    });
    let mineral = facts.mineral.map(|(kind, amount, density)| {
        view! {
            <dt>"Mineral"</dt>
            <dd>{format!("{kind} · {} · density {density}", number(amount))}</dd>
        }
    });
    let owners = (!facts.owners.is_empty()).then(|| {
        let rows = facts
            .owners
            .into_iter()
            .map(|owner| {
                let mut parts = vec![
                    counted(owner.creeps, "creep", "creeps"),
                    counted(owner.structures, "structure", "structures"),
                ];
                if owner.sites > 0 {
                    parts.push(counted(owner.sites, "site", "sites"));
                }
                view! {
                    <li class:own=owner.own>
                        <span class="owner">{owner.name}</span>
                        <span class="muted">{parts.join(" · ")}</span>
                    </li>
                }
            })
            .collect_view();
        view! {
            <dt>"Objects"</dt>
            <dd><ul class="owner-counts">{rows}</ul></dd>
        }
    });
    let tick = game_time.map(|time| {
        view! {
            <dt>"Updated"</dt>
            <dd class="numeric">{format!("tick {time}")}</dd>
        }
    });
    view! { {controller} {energy} {sources} {mineral} {owners} {tick} }
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use overseer::core::api::types::GameRoomObject;
    use overseer::core::socket::types::RoomUser;
    use serde_json::json;

    use super::{ControllerFacts, OwnerCount, facts};
    use crate::ui::inspector::Context;

    #[test]
    fn facts_of_a_live_room() {
        let objects: Vec<GameRoomObject> = serde_json::from_value(json!([
            {"_id": "k", "type": "controller", "room": "W1N1", "x": 21, "y": 39, "user": "u1",
             "level": 2, "progress": 100, "progressTotal": 45000, "downgradeTime": 20100,
             "safeMode": 150, "safeModeAvailable": 1},
            {"_id": "s2", "type": "source", "room": "W1N1", "x": 40, "y": 5, "energy": 3000,
             "energyCapacity": 3000, "ticksToRegeneration": 300},
            {"_id": "s1", "type": "source", "room": "W1N1", "x": 11, "y": 44, "energy": 1000,
             "energyCapacity": 3000, "ticksToRegeneration": 300},
            {"_id": "m", "type": "mineral", "room": "W1N1", "x": 6, "y": 45, "density": 3,
             "mineralAmount": 35000.0, "mineralType": "O"},
            {"_id": "sp", "type": "spawn", "room": "W1N1", "x": 18, "y": 36, "name": "Spawn1",
             "user": "u1", "hits": 5000, "hitsMax": 5000, "notifyWhenAttacked": true,
             "store": {"energy": 250}, "storeCapacityResource": {"energy": 300}},
            {"_id": "ex", "type": "extension", "room": "W1N1", "x": 19, "y": 36, "user": "u1",
             "hits": 1000, "hitsMax": 1000, "notifyWhenAttacked": true,
             "store": {"energy": 50}, "storeCapacityResource": {"energy": 50}},
            {"_id": "c1", "type": "creep", "room": "W1N1", "x": 7, "y": 43, "name": "w1",
             "user": "u1", "hits": 100, "hitsMax": 100, "store": {}, "body": []},
            {"_id": "c2", "type": "creep", "room": "W1N1", "x": 8, "y": 43, "name": "w2",
             "user": "u1", "hits": 100, "hitsMax": 100, "store": {}, "body": []},
            {"_id": "i", "type": "creep", "room": "W1N1", "x": 30, "y": 30, "name": "inv",
             "user": "2", "hits": 100, "hitsMax": 100, "store": {}, "body": []},
            {"_id": "cs", "type": "constructionSite", "room": "W1N1", "x": 2, "y": 2,
             "user": "u1", "structureType": "road", "progress": 0, "progressTotal": 300},
            {"_id": "t", "type": "tombstone", "room": "W1N1", "x": 5, "y": 5, "user": "u1",
             "deathTime": 1, "decayTime": 200, "store": {}},
            {"_id": "rd", "type": "road", "room": "W1N1", "x": 3, "y": 3, "hits": 5000,
             "hitsMax": 5000, "nextDecayTime": 1, "notifyWhenAttacked": true}
        ]))
        .unwrap();
        let users: BTreeMap<String, RoomUser> = [(
            "u1".to_owned(),
            serde_json::from_value(json!({"_id": "u1", "username": "smoke"})).unwrap(),
        )]
        .into_iter()
        .collect();
        let context = Context {
            game_time: Some(100),
            me: Some("u1"),
            users: &users,
        };
        let facts = facts(&objects, &context);
        assert_eq!(
            facts.controller,
            Some(ControllerFacts {
                owner: Some("smoke".to_owned()),
                own: true,
                level: 2,
                progress: Some((100, 45000)),
                downgrade_in: Some(20000),
                safe_mode_in: Some(50),
                safe_modes: Some(1),
                reservation: None,
            })
        );
        assert_eq!(facts.energy, Some((300, 350)));
        assert_eq!(facts.sources, [(1000, 3000), (3000, 3000)], "west to east");
        assert_eq!(facts.mineral, Some(("O".to_owned(), 35000, 3)));
        assert_eq!(
            facts.owners,
            [
                OwnerCount {
                    name: "smoke".to_owned(),
                    own: true,
                    creeps: 2,
                    structures: 3,
                    sites: 1
                },
                OwnerCount {
                    name: "Invader".to_owned(),
                    own: false,
                    creeps: 1,
                    structures: 0,
                    sites: 0
                },
            ],
            "tombstones are not counted; the controller is a structure"
        );
    }

    #[test]
    fn an_empty_room_has_no_facts() {
        let users = BTreeMap::new();
        let context = Context {
            game_time: None,
            me: None,
            users: &users,
        };
        assert_eq!(facts(&[], &context), super::Facts::default());
    }
}
