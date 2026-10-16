//! Everything of the viewer but the GPU: scope, camera and its flights, the drawn world,
//! pointer gestures, level of detail, interest and picking.

use glam::{Vec2, Vec3};
use overseer::map::{Interest, Portal, RoomCoord, RoomRect};

use crate::camera::{self, Camera, room_at, room_centre};
use crate::mesh::{Instance, WALL_HEIGHT, linear};
use crate::overlay;
use crate::scene::Eye;
use crate::world::{self, Limits, World, tile_origin};
use crate::{
    CameraState, DETAIL_PX_PER_ROOM, DETAIL_ROOMS, FLIGHT_MS, Lod, MINIMAP_PX_PER_ROOM,
    MINIMAP_ROOMS, Pick, PointerButton, Scope, TERRAIN_PX_PER_ROOM, ViewMode,
};

/// Distance (CSS pixels) a press may move and still be a click.
const CLICK_SLOP: f32 = 5.0;
/// Zoom factor per wheel pixel (`exp(delta_y × WHEEL_ZOOM)`).
const WHEEL_ZOOM: f32 = 0.0015;
/// A layer fades in between its threshold and this multiple of it.
const FADE_SPAN: f32 = 1.5;
/// The detail layer fades in between its threshold and this multiple of it.
const DETAIL_FADE_SPAN: f32 = 1.25;
/// Room borders fade in between these room sizes (CSS pixels).
const BORDERS_PX: (f32, f32) = (4.0, 10.0);
/// Colour (sRGB) of the selection frame.
const SELECTION: u32 = 0xff_eb_3b;
/// Smallest side of the selection frame, in CSS pixels.
const SELECTION_MIN_PX: f32 = 14.0;

/// A pressed pointer.
#[derive(Clone, Copy, Debug)]
struct Pointer {
    id: i32,
    button: PointerButton,
    start: Vec2,
    last: Vec2,
}

/// An animated camera move (`look_at`, `fit`): the target glides and the zoom follows a
/// path in log space that rises in between for long distances (so that both ends stay in
/// sight).
#[derive(Clone, Copy, Debug)]
struct Flight {
    from: (Vec2, f32),
    to: (Vec2, f32),
    /// Extra log half height at the middle of the flight.
    lift: f32,
    start: f64,
}

impl Flight {
    fn new(from: (Vec2, f32), to: (Vec2, f32), start: f64) -> Self {
        let middle = f32::midpoint(from.1.ln(), to.1.ln());
        let distance = from.0.distance(to.0);
        let lift = if distance > 1.0 {
            ((distance * 0.6).ln() - middle).max(0.0)
        } else {
            0.0
        };
        Self {
            from,
            to,
            lift,
            start,
        }
    }

    /// Target and half height at `now`, and whether the flight is over.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a fraction in 0..=1 fits f32"
    )]
    fn at(&self, now: f64) -> (Vec2, f32, bool) {
        let progress = ((now - self.start) / FLIGHT_MS).clamp(0.0, 1.0) as f32;
        if progress >= 1.0 {
            return (self.to.0, self.to.1, true);
        }
        let eased = progress * progress * (3.0 - 2.0 * progress);
        let (from_log, to_log) = (self.from.1.ln(), self.to.1.ln());
        let log = from_log + (to_log - from_log) * eased + self.lift * 4.0 * eased * (1.0 - eased);
        (self.from.0.lerp(self.to.0, eased), log.exp(), false)
    }
}

/// Opacity of each layer at the current zoom, `0..=1`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fades {
    /// Terrain over the overview cells.
    pub terrain: f32,
    /// Minimap points.
    pub minimap: f32,
    /// Detail scenes over the minimaps.
    pub detail: f32,
    /// Room borders.
    pub borders: f32,
    /// The status and owner accent over full-resolution terrain: shown until detail
    /// scenes fade in, then out with them, so that terrain and objects read clearly.
    pub accent: f32,
}

impl Fades {
    /// The fades of a world view whose rooms are `px_per_room` CSS pixels wide.
    pub fn at(px_per_room: f32) -> Self {
        let ramp = |threshold: f32, span: f32| smoothstep(threshold, threshold * span, px_per_room);
        let detail = ramp(DETAIL_PX_PER_ROOM, DETAIL_FADE_SPAN);
        Self {
            terrain: ramp(TERRAIN_PX_PER_ROOM, FADE_SPAN),
            minimap: ramp(MINIMAP_PX_PER_ROOM, FADE_SPAN),
            detail,
            borders: smoothstep(BORDERS_PX.0, BORDERS_PX.1, px_per_room),
            accent: 1.0 - detail,
        }
    }

    /// Everything shown but the accent (room scope).
    const FULL: Self = Self {
        terrain: 1.0,
        minimap: 1.0,
        detail: 1.0,
        borders: 1.0,
        accent: 0.0,
    };
}

fn smoothstep(from: f32, to: f32, value: f32) -> f32 {
    let t = ((value - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The viewer without the GPU.
#[derive(Debug)]
pub struct View {
    pub camera: Camera,
    pub world: World,
    scope: Scope,
    /// Pressed pointers, in press order.
    pointers: Vec<Pointer>,
    /// The current gesture moved beyond [`CLICK_SLOP`] or used two pointers: no click.
    dragged: bool,
    /// The camera was placed (by the user or the app): resizing keeps the framing instead
    /// of fitting the scope.
    moved: bool,
    flight: Option<Flight>,
    selected: Option<(RoomCoord, u8, u8)>,
    /// Bumped whenever the interest may change without the camera moving: the hovered or
    /// selected room, or portals arriving.
    interest_revision: u64,
    /// Pan inertia and eased wheel zoom are on.
    smooth: bool,
    /// The latest positions of a panning pointer (time, CSS point), newest last.
    trail: [(f64, Vec2); TRAIL],
    /// Pan inertia after a released drag.
    glide: Option<Glide>,
    /// A wheel zoom easing in.
    zoom: Option<ZoomEase>,
}

/// Positions kept to measure a drag's release velocity.
const TRAIL: usize = 6;
/// A drag's velocity is measured over its last this many milliseconds.
const TRAIL_MS: f64 = 90.0;
/// Time constant of the inertia's decay (ms).
const GLIDE_DECAY_MS: f32 = 325.0;
/// Inertia stops below this speed (CSS pixels per millisecond).
const GLIDE_STOP: f32 = 0.02;
/// Releases slower than this (CSS pixels per millisecond) do not glide.
const GLIDE_START: f32 = 0.15;
/// Time constant of the wheel zoom's easing (ms).
const ZOOM_EASE_MS: f32 = 70.0;
/// The rest of an eased zoom below this (log factor) is applied at once.
const ZOOM_STOP: f32 = 1e-3;

/// Pan inertia: the view keeps moving after a drag, slowing down.
#[derive(Clone, Copy, Debug)]
struct Glide {
    /// CSS pixels per millisecond.
    velocity: Vec2,
    last: f64,
}

/// A wheel zoom easing in around a CSS point.
#[derive(Clone, Copy, Debug)]
struct ZoomEase {
    anchor: Vec2,
    /// Log of the zoom factor still to apply.
    remaining: f32,
    last: f64,
}

/// Milliseconds from `last` to `now`, as `f32` (never negative).
#[expect(
    clippy::cast_possible_truncation,
    reason = "frame intervals are small; f32 is plenty"
)]
fn elapsed(now: f64, last: f64) -> f32 {
    (now - last).max(0.0) as f32
}

/// The velocity (CSS pixels per millisecond) of a drag released at `now`, from its trail
/// (newest last); `None` when it was slow or had stopped before the release.
fn release_velocity(trail: &[(f64, Vec2); TRAIL], now: f64) -> Option<Vec2> {
    let (newest_time, newest) = trail[TRAIL - 1];
    if now - newest_time > 50.0 {
        return None;
    }
    let (oldest_time, oldest) = trail
        .iter()
        .copied()
        .find(|(time, _)| newest_time - time <= TRAIL_MS)?;
    let span = elapsed(newest_time, oldest_time);
    if span <= 0.0 {
        return None;
    }
    let velocity = (newest - oldest) / span;
    (velocity.length() >= GLIDE_START).then_some(velocity)
}

impl View {
    /// A view of the room `E0S0` for a device with texture `limits`.
    #[cfg_attr(
        not(any(target_arch = "wasm32", test)),
        expect(
            dead_code,
            reason = "only the wasm canvas constructor and the tests create a view"
        )
    )]
    pub fn new(limits: Limits) -> Self {
        let room = RoomCoord { x: 0, y: 0 };
        let bounds = RoomRect {
            min: room,
            max: room,
        };
        let mut view = Self {
            camera: Camera::default(),
            world: World::new(bounds, limits),
            scope: Scope::Room(room),
            pointers: Vec::new(),
            dragged: false,
            moved: false,
            flight: None,
            selected: None,
            interest_revision: 0,
            smooth: true,
            trail: [(f64::NEG_INFINITY, Vec2::ZERO); TRAIL],
            glide: None,
            zoom: None,
        };
        view.camera.set_bounds(bounds);
        view.camera.fit();
        view.changed();
        view
    }

    /// Shows another scope; the camera frames it (until placed).
    pub fn set_scope(&mut self, scope: Scope) {
        if scope == self.scope {
            return;
        }
        self.scope = scope;
        let bounds = scope.bounds();
        self.world.set_bounds(bounds);
        self.camera.set_bounds(bounds);
        self.camera.fit();
        self.moved = false;
        self.flight = None;
        self.changed();
    }

    /// The camera moved: tells the world where it looks.
    fn changed(&mut self) {
        self.world.set_focus(self.camera.target());
    }

    /// Sets the canvas size in CSS pixels; frames the scope until the camera is placed.
    pub fn resize(&mut self, width: f32, height: f32) {
        self.camera.resize(width, height);
        if !self.moved {
            self.camera.fit();
        }
        self.changed();
    }

    /// Switches the projection, keeping target and zoom.
    pub fn set_mode(&mut self, mode: ViewMode) {
        self.camera.mode = mode;
        self.changed();
    }

    /// The persistable camera.
    pub const fn camera_state(&self) -> CameraState {
        self.camera.state()
    }

    /// Restores a camera (clamped to the scope).
    pub fn set_camera_state(&mut self, state: CameraState) {
        self.flight = None;
        self.moved = true;
        self.camera.set_state(state);
        self.changed();
    }

    /// Centres a room (or one of its tiles), zooming in to at least the room's framing.
    pub fn look_at(&mut self, room: RoomCoord, tile: Option<(u8, u8)>, animate: bool, now: f64) {
        let target = tile.map_or_else(
            || room_centre(room),
            |(x, y)| tile_origin(room, x.min(49), y.min(49)) + Vec2::splat(0.5),
        );
        let half_height = self
            .camera
            .half_height()
            .min(self.camera.room_half_height());
        self.go(target, half_height, animate, now);
    }

    /// Centres a room and frames all of it (whatever the current zoom).
    pub fn frame_room(&mut self, room: RoomCoord, animate: bool, now: f64) {
        let half_height = self.camera.room_half_height();
        self.go(room_centre(room), half_height, animate, now);
    }

    /// Frames the whole scope.
    pub fn fit(&mut self, animate: bool, now: f64) {
        let (target, half_height) = self.camera.fit_view();
        self.go(target, half_height, animate, now);
    }

    fn go(&mut self, target: Vec2, half_height: f32, animate: bool, now: f64) {
        self.moved = true;
        if animate {
            let from = (self.camera.target(), self.camera.half_height());
            self.flight = Some(Flight::new(from, (target, half_height), now));
        } else {
            self.flight = None;
            self.camera.set_view(target, half_height);
            self.changed();
        }
    }

    /// Advances a flight, pan inertia and an eased zoom to `now`; returns whether any
    /// continues.
    pub fn advance(&mut self, now: f64) -> bool {
        let mut moving = false;
        if let Some(flight) = self.flight {
            let (target, half_height, done) = flight.at(now);
            self.camera.set_view(target, half_height);
            if done {
                self.flight = None;
            }
            self.changed();
            moving |= !done;
        }
        if let Some(mut glide) = self.glide.take() {
            let elapsed = elapsed(now, glide.last);
            let decay = (-elapsed / GLIDE_DECAY_MS).exp();
            // The distance covered while the velocity decays exponentially.
            let step = glide.velocity * GLIDE_DECAY_MS * (1.0 - decay);
            let centre = self.camera.size() / 2.0;
            self.camera.pan(centre, centre + step);
            glide.velocity *= decay;
            glide.last = now;
            self.changed();
            if glide.velocity.length() > GLIDE_STOP {
                self.glide = Some(glide);
                moving = true;
            }
        }
        if let Some(mut zoom) = self.zoom.take() {
            let elapsed = elapsed(now, zoom.last);
            let share = 1.0 - (-elapsed / ZOOM_EASE_MS).exp();
            let step = if zoom.remaining.abs() < ZOOM_STOP {
                zoom.remaining
            } else {
                zoom.remaining * share
            };
            self.camera.zoom(zoom.anchor, step.exp());
            zoom.remaining -= step;
            zoom.last = now;
            self.changed();
            if zoom.remaining != 0.0 {
                self.zoom = Some(zoom);
                moving = true;
            }
        }
        moving
    }

    /// Sets whether pan inertia and the wheel zoom ease (`false`: they act at once).
    pub fn set_smooth(&mut self, smooth: bool) {
        self.smooth = smooth;
        if !smooth {
            self.glide = None;
            if let Some(zoom) = self.zoom.take() {
                self.camera.zoom(zoom.anchor, zoom.remaining.exp());
                self.changed();
            }
        }
    }

    /// Moves the view by CSS pixels (positive: east and south).
    pub fn pan_by(&mut self, delta: Vec2) {
        self.interrupt();
        let centre = self.camera.size() / 2.0;
        self.camera.pan(centre + delta, centre);
        self.changed();
    }

    /// Zooms around the canvas centre; `factor > 1` zooms in.
    pub fn zoom_by(&mut self, factor: f32) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        self.interrupt();
        self.camera.zoom(self.camera.size() / 2.0, 1.0 / factor);
        self.changed();
    }

    /// The user takes over the camera.
    const fn interrupt(&mut self) {
        self.flight = None;
        self.glide = None;
        self.moved = true;
    }

    /// A pointer was pressed at a CSS point (it stops pan inertia). A `primary` press
    /// (`PointerEvent.isPrimary`: the first pointer of a gesture, every mouse press) or a
    /// press of a pointer still listed as pressed starts a new gesture: the pointers listed
    /// lost their releases and are forgotten, so they cannot turn later clicks into pinches.
    pub fn pointer_down(&mut self, id: i32, point: Vec2, button: PointerButton, primary: bool) {
        if primary || self.pointers.iter().any(|pointer| pointer.id == id) {
            self.pointers.clear();
        }
        self.glide = None;
        self.trail = [(f64::NEG_INFINITY, Vec2::ZERO); TRAIL];
        // A second pointer: pinch, never a click.
        self.dragged = !self.pointers.is_empty();
        self.pointers.push(Pointer {
            id,
            button,
            start: point,
            last: point,
        });
    }

    /// A pointer's gesture ended without a release (`pointercancel`, a lost pointer
    /// capture): it is forgotten, without a click or a glide.
    pub fn pointer_cancel(&mut self, id: i32) {
        self.pointers.retain(|pointer| pointer.id != id);
    }

    /// A pointer moved at `now` (ms); pans, orbits or pinches when pressed.
    pub fn pointer_move(&mut self, id: i32, point: Vec2, now: f64) {
        let Some(index) = self.pointers.iter().position(|pointer| pointer.id == id) else {
            return;
        };
        let pointer = self.pointers[index];
        if self.pointers.len() >= 2 {
            if index < 2 {
                let other = self.pointers[1 - index].last;
                self.pinch((pointer.last, point), other);
            }
        } else {
            if !self.dragged && pointer.start.distance(point) <= CLICK_SLOP {
                return;
            }
            self.dragged = true;
            self.interrupt();
            if self.pans(pointer.button) {
                self.camera.pan(pointer.last, point);
                self.trail.rotate_left(1);
                self.trail[TRAIL - 1] = (now, point);
            } else {
                self.camera.orbit(point - pointer.last);
            }
            self.changed();
        }
        self.pointers[index].last = point;
    }

    /// Whether dragging with `button` pans (else it orbits).
    fn pans(&self, button: PointerButton) -> bool {
        match self.camera.mode {
            ViewMode::TopDown => true,
            ViewMode::Perspective => button != PointerButton::Primary,
        }
    }

    /// Two-pointer gesture: one pointer moved from `moved.0` to `moved.1`, the other is at
    /// `other`. Pans by the midpoint and zooms by the distance ratio around it.
    fn pinch(&mut self, moved: (Vec2, Vec2), other: Vec2) {
        let before = (moved.0 + other) / 2.0;
        let after = (moved.1 + other) / 2.0;
        let (old, new) = (moved.0.distance(other), moved.1.distance(other));
        self.interrupt();
        self.camera.pan(before, after);
        if old > 1.0 && new > 1.0 {
            self.camera.zoom(after, old / new);
        }
        self.changed();
    }

    /// A pointer was released at `now` (ms); a press released without dragging is a click
    /// and returns what is under it (`None` outside the scope). A pan released while
    /// moving keeps gliding.
    pub fn pointer_up(&mut self, id: i32, point: Vec2, now: f64) -> Option<Pick> {
        let index = self.pointers.iter().position(|pointer| pointer.id == id)?;
        let pointer = self.pointers.remove(index);
        let click = self.pointers.is_empty() && !self.dragged;
        if self.smooth && self.pointers.is_empty() && self.dragged && self.pans(pointer.button) {
            self.glide = release_velocity(&self.trail, now).map(|velocity| Glide {
                velocity,
                last: now,
            });
        }
        click.then(|| self.pick(point)).flatten()
    }

    /// Zooms around a CSS point at `now` (ms); positive `delta_y` (scrolling down) zooms
    /// out. With smoothing the zoom eases in over a few frames (wheel steps add up, up to
    /// the zoom limits: the view eases into a limit and scrolling back acts at once).
    pub fn wheel(&mut self, point: Vec2, delta_y: f32, now: f64) {
        self.interrupt();
        let log = delta_y * WHEEL_ZOOM;
        if self.smooth {
            let remaining = self.zoom.map_or(0.0, |zoom| zoom.remaining);
            let last = self.zoom.map_or(now, |zoom| zoom.last);
            let (min, max) = self.camera.half_height_range();
            let current = self.camera.half_height().ln();
            self.zoom = Some(ZoomEase {
                anchor: point,
                remaining: (remaining + log).clamp(min.ln() - current, max.ln() - current),
                last,
            });
        } else {
            self.camera.zoom(point, log.exp());
            self.changed();
        }
    }

    /// Whether an animated camera move (`look_at`, `fit`, `frame_room`) is under way.
    pub const fn is_flying(&self) -> bool {
        self.flight.is_some()
    }

    /// The level of detail: from the room size in a world scope, always
    /// [`Lod::Detail`] in a room scope.
    pub fn lod(&self) -> Lod {
        match self.scope {
            Scope::Room(_) => Lod::Detail,
            Scope::World(_) => Lod::of(self.camera.px_per_room()),
        }
    }

    /// The layer opacities: from the room size in a world scope, everything in a room
    /// scope.
    pub fn fades(&self) -> Fades {
        match self.scope {
            Scope::Room(_) => Fades::FULL,
            Scope::World(_) => Fades::at(self.camera.px_per_room()),
        }
    }

    /// The room under the canvas centre (always the room of a room scope).
    pub fn focused_room(&self) -> Option<RoomCoord> {
        if let Scope::Room(room) = self.scope {
            return Some(room);
        }
        let room = room_at(self.camera.target());
        self.scope.bounds().contains(room).then_some(room)
    }

    /// The rooms in view, within the scope.
    pub fn visible_rooms(&self) -> RoomRect {
        let bounds = self.scope.bounds();
        let (min, max) = self.camera.visible_ground();
        let clamp = |room: RoomCoord| RoomCoord {
            x: room.x.clamp(bounds.min.x, bounds.max.x),
            y: room.y.clamp(bounds.min.y, bounds.max.y),
        };
        // A view edge exactly on a room border does not show the next room.
        let max = max - Vec2::splat(1e-3);
        RoomRect::new(clamp(room_at(min)), clamp(room_at(max.max(min))))
    }

    /// What the camera needs now: see the crate documentation.
    pub fn interest(&self) -> Interest {
        if let Scope::Room(room) = self.scope {
            return Interest {
                stats: None,
                terrain: vec![room],
                minimaps: vec![room],
                detail: vec![room],
                portals: Some(RoomRect::new(room, room)),
                portal_destinations: Vec::new(),
            };
        }
        let lod = self.lod();
        let visible = self.visible_rooms();
        let mut interest = Interest {
            stats: Some(visible),
            portals: Some(visible),
            portal_destinations: self
                .world
                .overlay
                .wanted_destinations(visible, self.selected.map(|(room, ..)| room)),
            ..Interest::default()
        };
        // Terrain for every room in view at every level of detail: rooms stay resident once
        // they arrive, so nothing caps the list but the view.
        let centre = self.camera.target();
        let mut rooms: Vec<(f32, RoomCoord)> = visible
            .iter()
            .map(|room| (room_centre(room).distance_squared(centre), room))
            .collect();
        rooms.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        interest.terrain = rooms.iter().map(|(_, room)| *room).collect();
        let nearest = |count: usize| interest.terrain.iter().take(count).copied();
        if lod >= Lod::Minimap {
            interest.minimaps = nearest(MINIMAP_ROOMS).collect();
        }
        if lod == Lod::Detail {
            let focused = self.focused_room();
            interest.detail.extend(focused);
            interest.detail.extend(
                nearest(DETAIL_ROOMS + 1)
                    .filter(|room| Some(*room) != focused)
                    .take(DETAIL_ROOMS - interest.detail.len()),
            );
        }
        interest
    }

    /// The room and tile under a CSS point (raised walls of detail scenes included);
    /// `None` outside the scope.
    pub fn room_at(&self, point: Vec2) -> Option<(RoomCoord, u8, u8)> {
        let (origin, direction) = self.camera.ray(point);
        let tile = camera::cast(origin, direction, WALL_HEIGHT, |x, y| {
            self.world.height((x, y))
        })?;
        Some(world::split(tile))
    }

    /// The tile under a CSS point (raised walls of detail scenes included) and its
    /// objects, topmost first; `None` outside the scope.
    pub fn pick(&self, point: Vec2) -> Option<Pick> {
        let (room, x, y) = self.room_at(point)?;
        let objects = self
            .world
            .detail(room)
            .map(|detail| detail.scene.objects_at((x, y)))
            .unwrap_or_default();
        Some(Pick {
            room,
            x,
            y,
            objects,
        })
    }

    /// Highlights a tile.
    pub fn select(&mut self, tile: Option<(RoomCoord, u8, u8)>) {
        if tile.map(|(room, ..)| room) != self.selected.map(|(room, ..)| room) {
            self.interest_revision += 1;
        }
        self.selected = tile;
    }

    /// Outlines a room (`None`: none).
    pub fn set_hover(&mut self, room: Option<RoomCoord>) {
        if self.world.overlay.hovered() != room {
            self.interest_revision += 1;
        }
        self.world.overlay.set_hover(room);
    }

    /// Sets a room's portals.
    pub fn set_portals(&mut self, room: RoomCoord, portals: &[Portal]) {
        if self.world.overlay.set_portals(room, portals) {
            self.interest_revision += 1;
        }
    }

    /// Bumped whenever [`Self::interest`] may change without the camera moving.
    pub const fn interest_revision(&self) -> u64 {
        self.interest_revision
    }

    /// What the overlay's labels and portal arcs are made for.
    pub fn overlay_frame(&self) -> overlay::Frame {
        overlay::Frame {
            bounds: self.scope.bounds(),
            visible: self.visible_rooms(),
            px_per_room: self.camera.px_per_room(),
            world: matches!(self.scope, Scope::World(_)),
            selected: self.selected.map(|(room, ..)| room),
        }
    }

    /// The opacities of the overlay's parts.
    pub fn overlay_fades(&self) -> overlay::Fades {
        match self.scope {
            Scope::Room(_) => overlay::Fades::ROOM,
            Scope::World(_) => overlay::Fades::at(self.camera.px_per_room(), self.fades().detail),
        }
    }

    /// Brings the overlay up to date with the camera.
    pub fn update_overlay(&mut self) {
        let frame = self.overlay_frame();
        self.world.update_overlay(&frame);
    }

    /// Where the camera looks from, for the detail scenes.
    pub fn eye(&self) -> Eye {
        Eye {
            position: self.camera.eye(),
            forward: self.camera.forward(),
            perspective: self.camera.mode == ViewMode::Perspective,
        }
    }

    /// The selection frame, if a tile of the scope is selected: at least
    /// [`SELECTION_MIN_PX`] wide, on top of the tile's terrain.
    pub fn selection(&self) -> Option<Instance> {
        let (room, x, y) = self.selected?;
        if !self.scope.bounds().contains(room) || x >= 50 || y >= 50 {
            return None;
        }
        let side = (SELECTION_MIN_PX * self.camera.tiles_per_px()).max(1.08);
        let centre = self.tile_top(room, x, y);
        Some(Instance {
            offset: centre.to_array(),
            scale: [side, 0.12, side],
            color: linear(SELECTION),
        })
    }

    /// The CSS point where the top centre of a room's tile is drawn (`None` behind the
    /// camera); a click there picks the tile.
    pub fn tile_point(&self, room: RoomCoord, x: u8, y: u8) -> Option<Vec2> {
        self.camera.project(self.tile_top(room, x, y))
    }

    /// The world point at the centre of a tile's top (its terrain in a detail scene).
    fn tile_top(&self, room: RoomCoord, x: u8, y: u8) -> Vec3 {
        let corner = tile_origin(room, x, y);
        let height = self
            .world
            .detail(room)
            .map_or(0.0, |detail| detail.scene.tile_height(x, y));
        Vec3::new(corner.x + 0.5, height, corner.y + 0.5)
    }
}
