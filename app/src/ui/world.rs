//! The world screen: the continuous map of a shard filling the window, the right drawer and
//! the keyboard controls, plus [`MapState`], what the top bar and status bar know of the map.
//!
//! On start the screen opens the world (`Map::world`) and the shard's cache, seeds the world
//! with the cached terrain, rooms without terrain and overviews, and runs a [`Feed`] between
//! the map viewer and the world (which writes what the world fetches back to the cache and
//! reports its loading progress to the status bar).
//! The camera comes back from the saved state (per server and shard), the route's room is
//! framed whole when the camera does not already show it; on a first visit (neither) the world
//! opens on the account's start room (one of its rooms, else the server's default). The room
//! in focus follows clicks, the room search (which frames the whole room and hands the
//! keyboard back to the map) and, once zoomed into the live level, the room under the centre;
//! it is kept in the route and the saved state.
//!
//! A mouse resting over the map names the room under it: a tooltip next to the pointer
//! ([`HoverTip`]: room, owner and level, reservation, status, portal destinations), the
//! status bar's room item, and an outline of the room on the map. Owners' and reservers'
//! badges are drawn on their rooms: the badges of the rooms in view, nearest first, go to the
//! viewer as they load ([`crate::badges`]), and only while the zoom draws them.

use std::rc::Rc;
use std::time::Duration;

use leptos::prelude::*;
use overseer::map::{DetailState, RoomCoord, RoomRect, WorldEvent, WorldKnowledge, WorldProgress};
use overseer::{Overseer, StartRoom};
use overseer_view::{Lod, Scope, ViewMode, Viewer};
use web_sys::KeyboardEvent;

use crate::feed::{Feed, Reach};
use crate::profiles::Server;
use crate::route::{Route, Shard, parse_room};
use crate::saved::{BOUNDS_MAX_AGE_MS, Bounds};
use crate::session;
use crate::state::AppState;
use crate::ui::drawer::Drawer;
use crate::ui::hover::{HoverTip, Hovered, MapRooms};
use crate::ui::icon::{Icon, IconKind};
use crate::ui::spawn_scoped;
use crate::ui::style::{self, concatcp};
use crate::ui::surface::{Hooks, HoverHook, Surface, SurfaceCanvas};

/// Keyboard pan step, in CSS pixels.
const PAN_STEP: f32 = 80.0;
/// Keyboard and button zoom factor.
const ZOOM_STEP: f32 = 1.5;
/// How long the camera must rest before it is saved.
const SAVE_DELAY: Duration = Duration::from_secs(1);
/// A map control's button: quiet over the map, its tooltip on the left.
const MAP_BUTTON: &str = concatcp!(style::ICON_BUTTON_GHOST, " relative ", style::TIP_LEFT);

/// Where the world of the screen is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    /// No world screen.
    Idle,
    /// Opening the world.
    Loading,
    /// The world is shown.
    Ready,
    /// The world could not be opened.
    Failed(String),
}

/// What the top bar, drawer and status bar know of the map; `Copy`, provided as context.
#[derive(Clone, Copy)]
pub struct MapState {
    /// The map's surface while the world screen is shown.
    pub surface: StoredValue<Option<Surface>, LocalStorage>,
    /// The running world.
    pub feed: RwSignal<Option<Feed>, LocalStorage>,
    /// Where the world is.
    pub phase: RwSignal<Phase>,
    /// Shards of the server (empty on single-world servers).
    pub shards: RwSignal<Vec<String>>,
    /// The shard shown.
    pub shard: RwSignal<Option<Shard>>,
    /// The rooms of the world, once open.
    pub bounds: RwSignal<Option<RoomRect>>,
    /// The view mode of every viewer.
    pub mode: RwSignal<ViewMode>,
    /// Level of detail of the map.
    pub lod: RwSignal<Option<Lod>>,
    /// The room under the map's centre.
    pub focused: RwSignal<Option<RoomCoord>>,
    /// The room in focus (drawer, breadcrumb, route).
    pub room: RwSignal<Option<RoomCoord>>,
    /// The selected tile.
    pub tile: RwSignal<Option<(RoomCoord, u8, u8)>>,
    /// Game tick of the latest live room update.
    pub game_time: RwSignal<Option<u64>>,
    /// CPU time of the map's last frame, in milliseconds.
    pub frame_ms: RwSignal<Option<f32>>,
    /// The map's graphics backend.
    pub backend: RwSignal<Option<&'static str>>,
    /// How much of the world's wanted terrain and overviews is known.
    pub progress: RwSignal<Option<WorldProgress>>,
    /// Counts world screens, so that a leaving one does not clear its successor's state.
    generation: StoredValue<u64>,
}

impl MapState {
    /// An empty map state.
    pub fn new() -> Self {
        Self {
            surface: StoredValue::new_local(None),
            feed: RwSignal::new_local(None),
            phase: RwSignal::new(Phase::Idle),
            shards: RwSignal::new(Vec::new()),
            shard: RwSignal::new(None),
            bounds: RwSignal::new(None),
            mode: RwSignal::new(ViewMode::TopDown),
            lod: RwSignal::new(None),
            focused: RwSignal::new(None),
            room: RwSignal::new(None),
            tile: RwSignal::new(None),
            game_time: RwSignal::new(None),
            frame_ms: RwSignal::new(None),
            backend: RwSignal::new(None),
            progress: RwSignal::new(None),
            generation: StoredValue::new(0),
        }
    }

    /// The state provided by [`crate::ui::App`].
    pub fn expect() -> Self {
        expect_context()
    }

    /// Reads what the running world knows of its rooms (none without a world); tracked: the
    /// caller reruns when the world or its knowledge changes.
    pub fn knowledge<T>(self, read: impl FnOnce(&WorldKnowledge) -> T) -> Option<T> {
        self.feed
            .with(|feed| feed.as_ref().map(|feed| read(feed.knowledge())))
    }

    /// Runs `change` on the map's viewer, if it is shown.
    pub fn with_viewer(self, change: impl FnOnce(&mut Viewer)) {
        if let Some(surface) = self.surface.try_get_value().flatten() {
            surface.with_viewer(change);
        }
    }

    /// Flies to `room`, framing the whole room, and focuses it.
    pub fn go_to(self, room: RoomCoord) {
        self.room.set(Some(room));
        self.with_viewer(|viewer| viewer.frame_room(room, true));
    }

    /// Hands the keyboard to the map (without a focus ring: see [`Surface::focus`]).
    pub fn focus_map(self) {
        if let Some(surface) = self.surface.try_get_value().flatten() {
            surface.focus();
        }
    }

    /// Shows the whole world and leaves the room.
    pub fn show_world(self) {
        self.room.set(None);
        self.tile.set(None);
        self.with_viewer(|viewer| viewer.fit(true));
    }

    /// Keeps the level of detail, the room under the centre and the room in focus in step
    /// with the camera after a frame. The camera focuses a room when it comes to it at Detail:
    /// zooming into Detail or panning to another room there. Not while a flight passes over
    /// rooms, and not again while it stays put: leaving the room for the whole world
    /// ([`Self::show_world`]) starts at Detail.
    fn follow_camera(self, viewer: &Viewer) {
        let lod = viewer.lod();
        let focused = viewer.focused_room();
        let (last_lod, last_focused) = (self.lod.get_untracked(), self.focused.get_untracked());
        if last_lod != Some(lod) {
            self.lod.set(Some(lod));
        }
        if last_focused != focused {
            self.focused.set(focused);
        }
        let arrived = last_lod != Some(lod) || last_focused != focused;
        if lod == Lod::Detail
            && arrived
            && !viewer.is_flying()
            && focused.is_some()
            && self.room.get_untracked() != focused
        {
            self.room.set(focused);
        }
    }

    /// Zooms around the centre by `factor` (> 1 zooms in).
    pub fn zoom(self, factor: f32) {
        self.with_viewer(|viewer| viewer.zoom_by(factor));
    }

    /// Changes the view mode of every viewer (room controls follow [`Self::mode`]).
    pub fn set_mode(self, mode: ViewMode) {
        self.mode.set(mode);
        self.with_viewer(|viewer| viewer.set_mode(mode));
    }

    /// Back to no world, unless another world screen took over since `generation`.
    fn clear(self, generation: u64) {
        if self.generation.try_get_value() != Some(generation) {
            return;
        }
        self.surface.try_set_value(None);
        self.feed.try_set(None);
        self.phase.try_set(Phase::Idle);
        self.bounds.try_set(None);
        self.lod.try_set(None);
        self.focused.try_set(None);
        self.tile.try_set(None);
        self.game_time.try_set(None);
        self.frame_ms.try_set(None);
        self.backend.try_set(None);
        self.progress.try_set(None);
    }
}

/// Opens the world `shard` of the server with base URL `server` (`overseer` connected to
/// it) seeded from its cache: the feed, ready to subscribe to, reporting its progress to
/// `map`.
///
/// The world's bounds come from the saved state while younger than [`BOUNDS_MAX_AGE_MS`]
/// (no request); otherwise the library measures them and they are saved.
///
/// # Errors
///
/// Returns a message when the world cannot be opened. The cache is best effort: without
/// `IndexedDB` every room is fetched.
pub async fn open_world(
    state: AppState,
    map_state: MapState,
    overseer: &Overseer,
    server: &str,
    shard: &Shard,
) -> Result<Feed, String> {
    let key = shard.to_string();
    let now = js_sys::Date::now();
    let known = state.saved.with_untracked(|saved| {
        saved
            .places
            .get(server)
            .and_then(|place| place.bounds.get(&key).copied())
            .filter(|bounds| now - bounds.at < BOUNDS_MAX_AGE_MS)
    });
    let map = overseer.map();
    let world = match known {
        Some(bounds) => map.world_with_bounds(shard.name(), bounds.rect),
        None => map.world(shard.name()).await,
    }
    .map_err(|error| session::describe(&error))?;
    if known.is_none() {
        let rect = world.bounds();
        state.update_saved(|saved| {
            saved
                .place_mut(server)
                .bounds
                .insert(key, Bounds { rect, at: now });
        });
    }
    let progress = move |progress| {
        map_state.progress.try_set(Some(progress));
    };
    #[cfg(target_arch = "wasm32")]
    if let Ok(cache) = crate::terrain_cache::TerrainCache::open(server, shard.name()).await {
        let mut world = world;
        if let Ok(loaded) = cache.load().await {
            for (room, terrain) in loaded.terrain {
                world.insert_terrain(room, terrain);
            }
            for room in loaded.no_terrain {
                world.insert_no_terrain(room);
            }
            for (room, stat) in loaded.stats {
                world.insert_stats(room, stat);
            }
            for (room, portals, scanned_at) in loaded.portals {
                world.insert_portals(room, portals, scanned_at);
            }
        }
        return Ok(Feed::start(
            world,
            move |event| cache.store(event),
            progress,
        ));
    }
    Ok(Feed::start(world, |_| {}, progress))
}

/// The account's start room ([`overseer::Account::start_room`]) on `shard`, for a first
/// visit's room. Best effort: `None` when it cannot be read.
async fn start_room(overseer: Overseer, shard: Shard) -> Option<StartRoom> {
    overseer
        .account()
        .start_room(shard.name())
        .await
        .ok()
        .flatten()
}

/// A first visit's room (`opening` holds none yet): the account's `start` room when the world
/// `bounds` hold it and no room was chosen meanwhile. Without it the whole world shows.
fn open_on_start_room(
    map: MapState,
    opening: StoredValue<Option<RoomCoord>>,
    start: Option<&StartRoom>,
    bounds: RoomRect,
) {
    if opening.get_value().is_some() {
        return;
    }
    if let Some(room) = start_room_in(start, bounds)
        && map.room.get_untracked().is_none()
    {
        opening.set_value(Some(room));
        map.room.set(Some(room));
    }
}

/// The room of the account's `start` room when the world `bounds` hold it.
fn start_room_in(start: Option<&StartRoom>, bounds: RoomRect) -> Option<RoomCoord> {
    start
        .and_then(|start| parse_room(&start.room))
        .filter(|room| bounds.contains(*room))
}

/// The world screen of `server`'s shard `shard`, signed in with `overseer`, focused on the
/// route's `room` (none: the saved camera, else the account's start room).
#[component]
#[expect(
    clippy::needless_pass_by_value,
    reason = "component properties are owned"
)]
pub fn WorldScreen(
    server: Server,
    shard: Shard,
    room: Option<RoomCoord>,
    overseer: Overseer,
) -> impl IntoView {
    let state = AppState::expect();
    let map = MapState::expect();
    let rooms = MapRooms::expect();
    rooms.clear();
    let attempt = RwSignal::new(0_u32);
    let url = server.url.clone();
    let shard_key = shard.to_string();
    let place = state
        .saved
        .with_untracked(|saved| saved.places.get(&url).cloned());
    let saved_camera = place
        .as_ref()
        .and_then(|place| place.cameras.get(&shard_key).copied());
    map.mode
        .set(place.as_ref().map(|place| place.mode).unwrap_or_default());
    map.shard.set(Some(shard.clone()));
    map.room.set(room);
    map.tile.set(None);
    let generation = map.generation.get_value() + 1;
    map.generation.set_value(generation);
    let camera_saver = CameraSaver::new(state, url.clone(), shard_key.clone());
    let user = overseer.user_id().map(str::to_owned);
    #[cfg(target_arch = "wasm32")]
    let badges = BadgeWants::new(map, &overseer, &server);
    // The viewer shows the world (its scope is set): before, its camera means nothing.
    let scoped = StoredValue::new(false);
    let surface = Surface::new(Hooks {
        ready: Rc::new(move |viewer: &mut Viewer, ()| {
            viewer.set_user(user.clone());
            viewer.set_mode(map.mode.get_untracked());
        }),
        frame: Rc::new(move |viewer: &mut Viewer, stats| {
            if scoped.try_get_value() != Some(true) {
                return;
            }
            map.follow_camera(viewer);
            map.frame_ms.set(Some(stats.cpu_ms));
            camera_saver.changed(viewer.camera());
        }),
        pick: Rc::new(move |viewer: &mut Viewer, pick| {
            // Setting the same room again would restart everything that follows the room
            // (the drawer's live state among them).
            if map.room.get_untracked() != Some(pick.room) {
                map.room.set(Some(pick.room));
            }
            let tile = if viewer.lod() == Lod::Detail {
                Some((pick.room, pick.x, pick.y))
            } else {
                map.tile
                    .get_untracked()
                    .filter(|(room, _, _)| *room == pick.room)
            };
            map.tile.set(tile);
            viewer.select(tile);
        }),
        hover: Some(hover_hook(rooms)),
        // The badges follow the view the feed was told about.
        #[cfg(target_arch = "wasm32")]
        interest: Some(Rc::new(move |viewer: &Viewer| badges.follow(viewer))),
        #[cfg(not(target_arch = "wasm32"))]
        interest: None,
    });
    map.surface.set_value(Some(surface));
    on_cleanup(move || {
        camera_saver.flush();
        #[cfg(target_arch = "wasm32")]
        badges.stop();
        if map.generation.try_get_value() == Some(generation) {
            rooms.clear();
        }
        map.clear(generation);
    });
    Effect::new(move |_| map.backend.set(surface.backend.get()));

    // Open the world (again on retry). A first visit (no room in the route, no saved camera)
    // opens on the account's start room: one of its rooms, else the server's default.
    let opening = StoredValue::new(room);
    let overseer_for_load = overseer.clone();
    let shard_for_load = shard.clone();
    let url_for_load = url.clone();
    Effect::new(move |_| {
        attempt.track();
        map.phase.set(Phase::Loading);
        map.feed.set(None);
        let overseer = overseer_for_load.clone();
        let shard = shard_for_load.clone();
        let url = url_for_load.clone();
        spawn_scoped(async move {
            // A first visit's start room is read while the world opens: neither waits for the
            // other.
            let first_visit = room.is_none() && saved_camera.is_none();
            let start = first_visit.then(|| start_room(overseer.clone(), shard.clone()));
            let (opened, start) = futures_util::future::join(
                open_world(state, map, &overseer, &url, &shard),
                async move {
                    match start {
                        Some(start) => start.await,
                        None => None,
                    }
                },
            )
            .await;
            match opened {
                Ok(feed) => {
                    if first_visit {
                        open_on_start_room(map, opening, start.as_ref(), feed.bounds());
                    }
                    map.bounds.set(Some(feed.bounds()));
                    map.feed.set(Some(feed));
                    map.phase.set(Phase::Ready);
                }
                Err(error) => map.phase.set(Phase::Failed(error)),
            }
        });
    });

    // Attach the viewer to the world once both exist.
    Effect::new(move |attached: Option<bool>| {
        let Some(feed) = map.feed.get() else {
            return false;
        };
        if !surface.ready.get() {
            return false;
        }
        if attached == Some(true) {
            return true;
        }
        surface.with_viewer(|viewer| {
            viewer.set_scope(Scope::World(feed.bounds()));
            if let Some(camera) = saved_camera {
                viewer.set_camera(camera);
                viewer.set_mode(map.mode.get_untracked());
            }
            // A restored camera already on the room keeps its view. The default camera shows
            // the whole world around its centre, which may be the room itself (`W5N5` of a
            // `W10N10..W0N0` world): frame the room then too.
            if let Some(room) = opening.get_value()
                && feed.bounds().contains(room)
                && (saved_camera.is_none() || viewer.focused_room() != Some(room))
            {
                viewer.frame_room(room, false);
            }
        });
        let subscription = feed.subscribe(Reach::World, move |event| {
            surface.apply(event);
            if let WorldEvent::Detail(_, DetailState::Live(snapshot)) = event
                && snapshot.game_time.is_some()
            {
                map.game_time.set(snapshot.game_time);
            }
        });
        surface.attach(Some(subscription));
        scoped.set_value(true);
        true
    });

    // The room in focus goes into the route and the saved state; a route edited by hand
    // (or the back button) flies to its room.
    let slug = state.slug(&url).unwrap_or_default();
    let shard_for_route = shard.clone();
    let url_for_route = url.clone();
    let shard_key_for_route = shard_key.clone();
    Effect::new(move |_| {
        let room = map.room.get();
        state.replace_route(Route::World {
            server: slug.clone(),
            shard: Some(shard_for_route.clone()),
            room,
        });
        state.update_saved(|saved| {
            let place = saved.place_mut(&url_for_route);
            place.shard = Some(shard_key_for_route.clone());
            match room {
                Some(room) => {
                    place.rooms.insert(shard_key_for_route.clone(), room);
                }
                None => {
                    place.rooms.remove(&shard_key_for_route);
                }
            }
        });
    });
    Effect::new(move |_| {
        if let Route::World {
            room: Some(room), ..
        } = state.route.get()
            && map.room.get_untracked() != Some(room)
            && map.feed.with(Option::is_some)
        {
            map.go_to(room);
        }
    });
    Effect::new(move |_| {
        let tile = map.tile.get();
        surface.with_viewer(|viewer| viewer.select(tile));
    });

    keyboard(map);

    let label = map_label(map);
    view! {
        <div class="flex flex-1 min-h-0 max-sm:flex-col">
            <div class="relative flex-1 min-w-0">
                <SurfaceCanvas
                    surface
                    testid="world-map"
                    label
                    description="Drag to pan (or orbit in 3D), scroll or pinch to zoom, click a room or tile to select it; point at a room to see its name, owner and portals. Arrow keys or WASD pan, plus and minus zoom, Escape zooms out, slash searches a room."
                />
                <HoverTip />
                <WorldOverlay surface attempt />
                <MapControls />
            </div>
            <Drawer server=server.clone() />
        </div>
    }
}

/// Zoom in, zoom out and the whole world, over the map's bottom right corner.
#[component]
fn MapControls() -> impl IntoView {
    let map = MapState::expect();
    view! {
        <div
            class="absolute right-3 bottom-3 z-5 flex flex-col p-0.5 bg-panel/92 border border-edge rounded-lg shadow-sm"
            role="group"
            aria-label="Zoom"
        >
            <button
                type="button"
                class=MAP_BUTTON
                aria-label="Zoom in"
                aria-keyshortcuts="+"
                data-tip="Zoom in (+)"
                on:click=move |_| map.zoom(ZOOM_STEP)
            >
                <Icon kind=IconKind::Plus />
            </button>
            <button
                type="button"
                class=MAP_BUTTON
                aria-label="Zoom out"
                aria-keyshortcuts="-"
                data-tip="Zoom out (−)"
                on:click=move |_| map.zoom(1.0 / ZOOM_STEP)
            >
                <Icon kind=IconKind::Minus />
            </button>
            <button
                type="button"
                class=MAP_BUTTON
                aria-label="Show the whole world"
                data-tip="Show the whole world"
                on:click=move |_| map.show_world()
            >
                <Icon kind=IconKind::Fit />
            </button>
        </div>
    }
}

/// Loading, failure and empty states over the map.
#[component]
fn WorldOverlay(surface: Surface, attempt: RwSignal<u32>) -> impl IntoView {
    let map = MapState::expect();
    move || {
        if let Some(error) = surface.error.get() {
            return view! {
                <div class=style::OVERLAY role="alert">
                    <span class=style::STATE_ICON_ERROR>
                        <Icon kind=IconKind::Alert class=style::STATE_ICON_SVG />
                    </span>
                    <p class="max-w-128">{format!("The map cannot be drawn: {error}")}</p>
                </div>
            }
            .into_any();
        }
        match map.phase.get() {
            Phase::Loading | Phase::Idle => view! {
                <div class=style::OVERLAY role="status">
                    <span class=style::SPINNER aria-hidden="true"></span>
                    <p class="max-w-128">"Loading the world…"</p>
                </div>
            }
            .into_any(),
            Phase::Failed(error) => view! {
                <div class=style::OVERLAY role="alert">
                    <span class=style::STATE_ICON_ERROR>
                        <Icon kind=IconKind::Alert class=style::STATE_ICON_SVG />
                    </span>
                    <p class="max-w-128">{format!("The world could not be opened: {error}")}</p>
                    <button
                        type="button"
                        class=style::BUTTON_PRIMARY
                        on:click=move |_| attempt.update(|n| *n += 1)
                    >
                        <Icon kind=IconKind::Refresh />
                        "Try again"
                    </button>
                </div>
            }
            .into_any(),
            Phase::Ready if map.bounds.get().is_some_and(|bounds| bounds.is_empty()) => view! {
                <div class=style::OVERLAY role="status">
                    <span class=style::STATE_ICON>
                        <Icon kind=IconKind::Globe class=style::STATE_ICON_SVG />
                    </span>
                    <p class="max-w-128">"This world has no rooms yet."</p>
                </div>
            }
            .into_any(),
            Phase::Ready => ().into_any(),
        }
    }
}

/// Arrow keys or WASD pan, `+`/`-` zoom, Escape zooms out, `/` focuses the room search;
/// ignored while typing, in menus, with modifiers, or when a handler already took the key.
fn keyboard(map: MapState) {
    let handle = window_event_listener(leptos::ev::keydown, move |event: KeyboardEvent| {
        if event.default_prevented()
            || event.ctrl_key()
            || event.meta_key()
            || event.alt_key()
            || typing(&event)
        {
            return;
        }
        let pan = |dx: f32, dy: f32| map.with_viewer(|viewer| viewer.pan_by(dx, dy));
        match event.key().as_str() {
            "ArrowLeft" | "a" | "A" => pan(-PAN_STEP, 0.0),
            "ArrowRight" | "d" | "D" => pan(PAN_STEP, 0.0),
            "ArrowUp" | "w" | "W" => pan(0.0, -PAN_STEP),
            "ArrowDown" | "s" | "S" => pan(0.0, PAN_STEP),
            "+" | "=" => map.zoom(ZOOM_STEP),
            "-" | "_" => map.zoom(1.0 / ZOOM_STEP),
            "Escape" => map.zoom(1.0 / (ZOOM_STEP * ZOOM_STEP)),
            "/" => {
                if let Some(search) = web_sys::window()
                    .and_then(|window| window.document())
                    .and_then(|document| document.get_element_by_id("room-search"))
                    .and_then(|element| {
                        wasm_bindgen::JsCast::dyn_into::<web_sys::HtmlElement>(element).ok()
                    })
                {
                    let _ = search.focus();
                }
            }
            _ => return,
        }
        event.prevent_default();
    });
    on_cleanup(move || handle.remove());
}

/// Whether the key goes to a text field, a select or a menu.
fn typing(event: &KeyboardEvent) -> bool {
    event
        .target()
        .and_then(|target| wasm_bindgen::JsCast::dyn_into::<web_sys::Element>(target).ok())
        .and_then(|element| {
            element
                .closest("input, textarea, select, [contenteditable], [role=menu]")
                .ok()
                .flatten()
        })
        .is_some()
}

/// Saves the camera of a shard once it rests for [`SAVE_DELAY`], and when the page hides.
#[derive(Clone, Copy)]
struct CameraSaver {
    state: AppState,
    url: StoredValue<String>,
    shard: StoredValue<String>,
    /// The camera to save, if it changed since the last save.
    pending: StoredValue<Option<overseer_view::CameraState>>,
    /// The camera last saved.
    saved: StoredValue<Option<overseer_view::CameraState>>,
    /// A save is scheduled.
    scheduled: StoredValue<bool>,
}

impl CameraSaver {
    /// A saver for the shard `shard` (route segment) of the server with base URL `url`.
    fn new(state: AppState, url: String, shard: String) -> Self {
        let saver = Self {
            state,
            url: StoredValue::new(url),
            shard: StoredValue::new(shard),
            pending: StoredValue::new(None),
            saved: StoredValue::new(None),
            scheduled: StoredValue::new(false),
        };
        let hide = window_event_listener(leptos::ev::pagehide, move |_| saver.flush());
        on_cleanup(move || hide.remove());
        saver
    }

    /// The camera is now `camera`.
    fn changed(self, camera: overseer_view::CameraState) {
        if self.saved.try_get_value().flatten() == Some(camera) {
            return;
        }
        self.pending.try_set_value(Some(camera));
        if self.scheduled.try_get_value() == Some(false) {
            self.scheduled.set_value(true);
            set_timeout(move || self.flush(), SAVE_DELAY);
        }
    }

    /// Saves the pending camera now.
    fn flush(self) {
        self.scheduled.try_set_value(false);
        let Some(Some(camera)) = self.pending.try_get_value() else {
            return;
        };
        self.pending.set_value(None);
        self.saved.set_value(Some(camera));
        let (url, shard) = (self.url.get_value(), self.shard.get_value());
        self.state.update_saved(|saved| {
            saved.place_mut(&url).cameras.insert(shard, camera);
        });
    }
}

/// The map canvas's accessible name: the shard, the room in focus and the selected tile.
fn map_label(map: MapState) -> Signal<String> {
    Signal::derive(move || {
        let shard = map.shard.get().map(|shard| shard.to_string());
        let room = map.room.get();
        let place = match (shard.as_deref(), room) {
            (Some("-") | None, Some(room)) => format!("World map, at {room}"),
            (Some("-") | None, None) => "World map".to_owned(),
            (Some(shard), Some(room)) => format!("Map of {shard}, at {room}"),
            (Some(shard), None) => format!("Map of {shard}"),
        };
        match map.tile.get() {
            Some((tile_room, x, y)) if Some(tile_room) == room => {
                format!("{place}, tile {x},{y} selected")
            }
            _ => place,
        }
    })
}

/// The world map's hover hook: the room and tile under the pointer go to [`MapRooms::hover`]
/// and the viewer outlines the room (a frame only when the room changes).
fn hover_hook(rooms: MapRooms) -> HoverHook {
    Rc::new(move |viewer: &mut Viewer, point| {
        let hovered = point.and_then(|(x, y)| {
            let (room, tile_x, tile_y) = viewer.room_at(x, y)?;
            Some(Hovered {
                room,
                tile: (tile_x, tile_y),
                point: (x, y),
            })
        });
        let Some(before) = rooms.hover.try_get_untracked() else {
            return false;
        };
        if before != hovered {
            rooms.hover.set(hovered);
        }
        let room = hovered.map(|hovered| hovered.room);
        if before.map(|before| before.room) == room {
            return false;
        }
        viewer.set_hover(room);
        true
    })
}

/// Asks the badge loader ([`crate::badges`]) for the badges of the owners and reservers in
/// view, nearest the centre first, whenever the map's interest goes to the feed (one camera
/// change drives both) and whenever what the world knows changes while badges are drawn; for
/// none while badges are too small to be drawn.
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy)]
struct BadgeWants {
    /// The rooms in view and whether badges are drawn, as the feed was last told.
    view: RwSignal<Option<(RoomRect, bool)>>,
    loader: StoredValue<crate::badges::Badges, LocalStorage>,
}

#[cfg(target_arch = "wasm32")]
impl BadgeWants {
    /// Badges of `server` (`overseer` connected to it) for the viewer of `map`, from what its
    /// world knows.
    fn new(map: MapState, overseer: &Overseer, server: &Server) -> Self {
        let surface = map.surface;
        let loader =
            crate::badges::Badges::new(overseer.clone(), &server.url, move |user, pixels| {
                if let Some(surface) = surface.try_get_value().flatten() {
                    surface.with_viewer(|viewer| {
                        viewer.set_badge(user, pixels, crate::badges::BADGE_SIZE);
                    });
                }
            });
        let wants = Self {
            view: RwSignal::new(None),
            loader: StoredValue::new_local(loader),
        };
        Effect::new(move |_| {
            let Some((rooms, visible)) = wants.view.get() else {
                return;
            };
            let wanted = if visible {
                map.knowledge(|knowledge| {
                    let stats = knowledge.stats_in(rooms);
                    crate::badges::wanted(stats.iter().map(|(room, stat)| (*room, &**stat)), rooms)
                })
                .unwrap_or_default()
            } else {
                Vec::new()
            };
            wants.loader.with_value(|loader| loader.want(wanted));
        });
        wants
    }

    /// The feed was told about the view of `viewer`: the badges follow it.
    fn follow(self, viewer: &Viewer) {
        let view = Some((viewer.visible_rooms(), viewer.badges_visible()));
        if self
            .view
            .try_get_untracked()
            .is_some_and(|before| before != view)
        {
            self.view.set(view);
        }
    }

    /// The screen left: no more badges.
    fn stop(self) {
        self.loader.try_with_value(crate::badges::Badges::stop);
    }
}

#[cfg(test)]
mod tests {
    use overseer::StartRoom;
    use overseer::map::{RoomCoord, RoomRect};

    use super::start_room_in;

    fn room(name: &str) -> RoomCoord {
        name.parse().unwrap()
    }

    fn start(room: &str) -> StartRoom {
        StartRoom {
            shard: None,
            room: room.to_owned(),
        }
    }

    #[test]
    fn the_start_room_must_lie_in_the_world() {
        // A centred world of 11 × 11 rooms.
        let bounds = RoomRect::new(room("W5N5"), room("E5S5"));
        assert_eq!(
            start_room_in(Some(&start("W3N2")), bounds),
            Some(room("W3N2"))
        );
        assert_eq!(
            start_room_in(Some(&start("e5s5")), bounds),
            Some(room("E5S5"))
        );
        assert_eq!(start_room_in(Some(&start("W6N0")), bounds), None);
        assert_eq!(start_room_in(Some(&start("sim")), bounds), None);
        assert_eq!(start_room_in(None, bounds), None);
    }
}
