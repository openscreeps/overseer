//! Everything of the viewer but the GPU: scope, camera and its flights, the drawn world,
//! pointer gestures, level of detail, interest and picking.

use glam::{Vec2, Vec3};
use overseer::map::{Interest, RoomCoord, RoomRect};

use crate::camera::{self, Camera, room_at, room_centre};
use crate::mesh::{Instance, WALL_HEIGHT, linear};
use crate::world::{self, World, tile_origin};
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
}

impl Fades {
    /// The fades of a world view whose rooms are `px_per_room` CSS pixels wide.
    pub fn at(px_per_room: f32) -> Self {
        let ramp = |threshold: f32, span: f32| smoothstep(threshold, threshold * span, px_per_room);
        Self {
            terrain: ramp(TERRAIN_PX_PER_ROOM, FADE_SPAN),
            minimap: ramp(MINIMAP_PX_PER_ROOM, FADE_SPAN),
            detail: ramp(DETAIL_PX_PER_ROOM, DETAIL_FADE_SPAN),
            borders: smoothstep(BORDERS_PX.0, BORDERS_PX.1, px_per_room),
        }
    }

    /// Everything shown (room scope).
    const FULL: Self = Self {
        terrain: 1.0,
        minimap: 1.0,
        detail: 1.0,
        borders: 1.0,
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
}

impl View {
    /// A view of the room `E0S0` with a terrain atlas of `atlas_per_row²` slots.
    #[cfg_attr(
        not(any(target_arch = "wasm32", test)),
        expect(
            dead_code,
            reason = "only the wasm canvas constructor and the tests create a view"
        )
    )]
    pub fn new(atlas_per_row: u32) -> Self {
        let room = RoomCoord { x: 0, y: 0 };
        let bounds = RoomRect {
            min: room,
            max: room,
        };
        let mut view = Self {
            camera: Camera::default(),
            world: World::new(bounds, atlas_per_row),
            scope: Scope::Room(room),
            pointers: Vec::new(),
            dragged: false,
            moved: false,
            flight: None,
            selected: None,
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

    /// The camera moved: tells the world what is in view.
    fn changed(&mut self) {
        let visible = self.visible_rooms();
        self.world.set_focus(visible, self.camera.target());
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

    /// Advances a flight to `now`; returns whether it continues.
    pub fn advance(&mut self, now: f64) -> bool {
        let Some(flight) = self.flight else {
            return false;
        };
        let (target, half_height, done) = flight.at(now);
        self.camera.set_view(target, half_height);
        if done {
            self.flight = None;
        }
        self.changed();
        !done
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
        self.moved = true;
    }

    /// A pointer was pressed at a CSS point.
    pub fn pointer_down(&mut self, id: i32, point: Vec2, button: PointerButton) {
        self.pointers.retain(|pointer| pointer.id != id);
        // A second pointer: pinch, never a click.
        self.dragged = !self.pointers.is_empty();
        self.pointers.push(Pointer {
            id,
            button,
            start: point,
            last: point,
        });
    }

    /// A pointer moved; pans, orbits or pinches when pressed.
    pub fn pointer_move(&mut self, id: i32, point: Vec2) {
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
            let pans = match self.camera.mode {
                ViewMode::TopDown => true,
                ViewMode::Perspective => pointer.button != PointerButton::Primary,
            };
            if pans {
                self.camera.pan(pointer.last, point);
            } else {
                self.camera.orbit(point - pointer.last);
            }
            self.changed();
        }
        self.pointers[index].last = point;
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

    /// A pointer was released; a press released without dragging is a click and returns what
    /// is under it (`None` outside the scope).
    pub fn pointer_up(&mut self, id: i32, point: Vec2) -> Option<Pick> {
        let index = self.pointers.iter().position(|pointer| pointer.id == id)?;
        self.pointers.remove(index);
        let click = self.pointers.is_empty() && !self.dragged;
        click.then(|| self.pick(point)).flatten()
    }

    /// Zooms around a CSS point; positive `delta_y` (scrolling down) zooms out.
    pub fn wheel(&mut self, point: Vec2, delta_y: f32) {
        self.interrupt();
        self.camera.zoom(point, (delta_y * WHEEL_ZOOM).exp());
        self.changed();
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
            };
        }
        let lod = self.lod();
        let visible = self.visible_rooms();
        let mut interest = Interest {
            stats: Some(visible),
            ..Interest::default()
        };
        if lod < Lod::Terrain {
            return interest;
        }
        let centre = self.camera.target();
        let mut rooms: Vec<(f32, RoomCoord)> = visible
            .iter()
            .map(|room| (room_centre(room).distance_squared(centre), room))
            .collect();
        rooms.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let nearest = |count: usize| rooms.iter().take(count).map(|(_, room)| *room);
        interest.terrain = nearest(self.world.atlas.capacity()).collect();
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

    /// The tile under a CSS point (raised walls of detail scenes included) and its
    /// objects, topmost first; `None` outside the scope.
    pub fn pick(&self, point: Vec2) -> Option<Pick> {
        let (origin, direction) = self.camera.ray(point);
        let tile = camera::cast(origin, direction, WALL_HEIGHT, |x, y| {
            self.world.height((x, y))
        })?;
        let (room, x, y) = world::split(tile);
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
    pub const fn select(&mut self, tile: Option<(RoomCoord, u8, u8)>) {
        self.selected = tile;
    }

    /// The selection frame, if a tile of the scope is selected: at least
    /// [`SELECTION_MIN_PX`] wide, on top of the tile's terrain.
    pub fn selection(&self) -> Option<Instance> {
        let (room, x, y) = self.selected?;
        if !self.scope.bounds().contains(room) || x >= 50 || y >= 50 {
            return None;
        }
        let corner = tile_origin(room, x, y);
        let base = self
            .world
            .detail(room)
            .map_or(0.0, |detail| detail.scene.tile_height(x, y));
        let side = (SELECTION_MIN_PX * self.camera.tiles_per_px()).max(1.08);
        let centre = Vec3::new(corner.x + 0.5, base, corner.y + 0.5);
        Some(Instance {
            offset: centre.to_array(),
            scale: [side, 0.12, side],
            color: linear(SELECTION),
        })
    }
}
