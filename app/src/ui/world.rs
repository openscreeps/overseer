//! The world screen: the continuous map of a shard filling the window, the right drawer and
//! the keyboard controls, plus [`MapState`], what the top bar and status bar know of the map.
//!
//! On start the screen opens the world (`Map::world`) and the shard's cache, seeds the world
//! with the cached terrain, rooms without terrain and overviews, and runs a [`Feed`] between
//! the map viewer and the world (which writes what the world fetches back to the cache and
//! reports its loading progress to the status bar).
//! The camera comes back from the saved state (per server and shard), the route's room is
//! looked at when the camera does not already show it; on a first visit (neither) the world
//! opens on the account's start room (one of its rooms, else the server's default). The room
//! in focus follows clicks, the room search and, once zoomed into the live level, the room
//! under the centre; it is kept in the route and the saved state.

use std::rc::Rc;
use std::time::Duration;

use leptos::prelude::*;
use overseer::map::{DetailState, RoomCoord, RoomRect, WorldEvent, WorldProgress};
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
use crate::ui::spawn_scoped;
use crate::ui::surface::{Hooks, Surface, SurfaceCanvas};

/// Keyboard pan step, in CSS pixels.
const PAN_STEP: f32 = 80.0;
/// Keyboard and button zoom factor.
const ZOOM_STEP: f32 = 1.5;
/// How long the camera must rest before it is saved.
const SAVE_DELAY: Duration = Duration::from_secs(1);

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

    /// Runs `change` on the map's viewer, if it is shown.
    pub fn with_viewer(self, change: impl FnOnce(&mut Viewer)) {
        if let Some(surface) = self.surface.try_get_value().flatten() {
            surface.with_viewer(change);
        }
    }

    /// Flies to `room` and focuses it.
    pub fn go_to(self, room: RoomCoord) {
        self.room.set(Some(room));
        self.with_viewer(|viewer| viewer.look_at(room, None, true));
    }

    /// Shows the whole world and leaves the room.
    pub fn show_world(self) {
        self.room.set(None);
        self.tile.set(None);
        self.with_viewer(|viewer| viewer.fit(true));
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
        }
        return Ok(Feed::start(
            world,
            move |event| cache.store(event),
            progress,
        ));
    }
    Ok(Feed::start(world, |_| {}, progress))
}

/// A first visit's room (`opening` holds none yet): the account's start room
/// ([`overseer::Account::start_room`]) on `shard` when the world `bounds` hold it and no room
/// was chosen meanwhile. Best effort: without it the whole world shows.
async fn open_on_start_room(
    map: MapState,
    opening: StoredValue<Option<RoomCoord>>,
    overseer: &Overseer,
    shard: &Shard,
    bounds: RoomRect,
) {
    if opening.get_value().is_some() {
        return;
    }
    let Ok(start) = overseer.account().start_room(shard.name()).await else {
        return;
    };
    if let Some(room) = start_room_in(start.as_ref(), bounds)
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
            let lod = viewer.lod();
            if map.lod.get_untracked() != Some(lod) {
                map.lod.set(Some(lod));
            }
            let focused = viewer.focused_room();
            if map.focused.get_untracked() != focused {
                map.focused.set(focused);
            }
            if lod == Lod::Detail && focused.is_some() && map.room.get_untracked() != focused {
                map.room.set(focused);
            }
            map.frame_ms.set(Some(stats.cpu_ms));
            camera_saver.changed(viewer.camera());
        }),
        pick: Rc::new(move |viewer: &mut Viewer, pick| {
            map.room.set(Some(pick.room));
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
    });
    map.surface.set_value(Some(surface));
    on_cleanup(move || {
        camera_saver.flush();
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
            match open_world(state, map, &overseer, &url, &shard).await {
                Ok(feed) => {
                    if room.is_none() && saved_camera.is_none() {
                        open_on_start_room(map, opening, &overseer, &shard, feed.bounds()).await;
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
            if let Some(room) = opening.get_value()
                && feed.bounds().contains(room)
                && viewer.focused_room() != Some(room)
            {
                viewer.look_at(room, None, false);
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

    let label = Signal::derive(move || {
        let shard = map.shard.get().map(|shard| shard.to_string());
        match (shard.as_deref(), map.room.get()) {
            (Some("-") | None, Some(room)) => format!("World map, at {room}"),
            (Some("-") | None, None) => "World map".to_owned(),
            (Some(shard), Some(room)) => format!("Map of {shard}, at {room}"),
            (Some(shard), None) => format!("Map of {shard}"),
        }
    });
    view! {
        <div class="world">
            <div class="stage">
                <SurfaceCanvas
                    surface
                    testid="world-map"
                    label
                    description="Drag to pan (or orbit in 3D), scroll or pinch to zoom, click a room or tile to select it. Arrow keys or WASD pan, plus and minus zoom, Escape zooms out, slash searches a room."
                />
                <WorldOverlay surface attempt />
                <div class="map-controls" role="group" aria-label="Zoom">
                    <button type="button" aria-label="Zoom in" on:click=move |_| map.zoom(ZOOM_STEP)>
                        "+"
                    </button>
                    <button
                        type="button"
                        aria-label="Zoom out"
                        on:click=move |_| map.zoom(1.0 / ZOOM_STEP)
                    >
                        "−"
                    </button>
                    <button type="button" aria-label="Show the whole world" on:click=move |_| map.show_world()>
                        "⤢"
                    </button>
                </div>
            </div>
            <Drawer server=server.clone() />
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
                <div class="overlay" role="alert">
                    <p>{format!("The map cannot be drawn: {error}")}</p>
                </div>
            }
            .into_any();
        }
        match map.phase.get() {
            Phase::Loading | Phase::Idle => view! {
                <div class="overlay" role="status">
                    <span class="spinner" aria-hidden="true"></span>
                    <p>"Loading the world…"</p>
                </div>
            }
            .into_any(),
            Phase::Failed(error) => view! {
                <div class="overlay" role="alert">
                    <p>{format!("The world could not be opened: {error}")}</p>
                    <button type="button" class="primary" on:click=move |_| attempt.update(|n| *n += 1)>
                        "Try again"
                    </button>
                </div>
            }
            .into_any(),
            Phase::Ready if map.bounds.get().is_some_and(|bounds| bounds.is_empty()) => view! {
                <div class="overlay" role="status">
                    <p>"This world has no rooms yet."</p>
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
