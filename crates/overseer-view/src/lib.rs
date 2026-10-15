//! World and room viewer of the Overseer app: draws a shard's rooms with wgpu into a browser
//! `<canvas>`, through WebGPU when the browser grants an adapter and WebGL2 otherwise, from
//! the whole shard down to live [`RoomSnapshot`]s.
//!
//! # Scope
//!
//! A [`Viewer`] shows a [`Scope`]: [`Scope::World`] is a rectangle of rooms (a whole shard)
//! to zoom continuously from all rooms into one; [`Scope::Room`] is one room, the camera kept
//! over it (the room control and its pop-out window). Positions are world tiles: the tile
//! `(x, y)` of room [`RoomCoord`] `(rx, ry)` is at `(rx × 50 + x, ry × 50 + y)`, `x` east and
//! `y` south, as in [`CameraState::centre`].
//!
//! # Levels of detail
//!
//! In a world scope the [`Lod`] follows the size of a room on the canvas in CSS pixels
//! ([`Viewer::lod`]); each layer fades in above its threshold so that zooming is continuous:
//!
//! | [`Lod`] | room size | drawn |
//! |---|---|---|
//! | [`Lod::Overview`] | below [`TERRAIN_PX_PER_ROOM`] (24) | every room as one cell coloured by status and owner (one instanced draw) |
//! | [`Lod::Terrain`] | from [`TERRAIN_PX_PER_ROOM`] | + each room's terrain from a texture atlas, tinted by its owner or status; room borders |
//! | [`Lod::Minimap`] | from [`MINIMAP_PX_PER_ROOM`] (100) | + live minimap points per kind and owner |
//! | [`Lod::Detail`] | from [`DETAIL_PX_PER_ROOM`] (300) | + the live room scene of rooms with a snapshot, over their minimap |
//!
//! Terrain fades in between 24 and 36 pixels per room, minimaps between 100 and 150, detail
//! scenes between 300 and 375 (and over 300 ms after their first snapshot); room borders
//! between 4 and 10. A room scope is always [`Lod::Detail`] with every layer shown.
//!
//! Overview colours ([`Viewer::set_stat`]): normal rooms grey, novice areas green, respawn
//! areas blue, closed rooms near black, unknown rooms dark; owned rooms take their owner's
//! colour (stronger with the controller level) and reserved rooms a lighter share of it.
//! Owner colours are those of objects: the signed-in user ([`Viewer::set_user`]) green, other
//! players red, the Invader NPC (user `2`) orange, the Source Keeper NPC (user `3`) purple.
//!
//! # Detail scenes
//!
//! Both [`ViewMode`]s draw the same 3D scene: the terrain (plains and swamps flat, natural
//! walls raised) and every object as a simple shape whose size and colour follow its kind
//! and owner. The [`ViewMode::TopDown`] camera is orthographic and looks straight down; the
//! [`ViewMode::Perspective`] camera orbits the same target at a distance framing the same
//! zoom. A selected tile ([`Viewer::select`]) is framed in yellow over everything (at least
//! 14 pixels wide, so it shows at every zoom).
//!
//! A room's terrain mesh is rebuilt only when a snapshot brings a different terrain. Objects
//! are matched by id between snapshots: one that moved glides to its new tile over
//! [`MOVE_DURATION_MS`] ([`FrameStats::animating`] is `true` meanwhile); new objects appear
//! and removed objects disappear at once. Up to four rooms keep a scene at once.
//!
//! # Interest
//!
//! [`Viewer::interest`] says what the camera needs; hand it to
//! `overseer::map::WorldMap::set_interest` (throttled) and feed the world's events back
//! (`Terrain` → [`Viewer::set_terrain`], `Stats` → [`Viewer::set_stat`], `Minimap` /
//! `MinimapGone` → [`Viewer::set_minimap`], `Detail(Live)` → [`Viewer::set_room`],
//! `DetailGone` → [`Viewer::clear_room`]). Rooms are listed nearest to the camera's centre
//! first. In a world scope:
//!
//! - `stats`: the rooms in view, at every level of detail;
//! - `terrain`: from [`Lod::Terrain`], the rooms in view, at most one atlas of rooms (the
//!   atlas holds `⌊min(3200, max texture size) / 50⌋²` rooms: 4096 on most GPUs);
//! - `minimaps`: from [`Lod::Minimap`], at most [`MINIMAP_ROOMS`];
//! - `detail`: at [`Lod::Detail`], the room under the centre ([`Viewer::focused_room`]) then
//!   the nearest other, at most [`DETAIL_ROOMS`] (the server's per-user limit).
//!
//! A room scope wants its room's terrain, minimap and detail, and no stats.
//!
//! The viewer stores what it is given (terrain of every room seen, packed at 2 bits per tile)
//! and uploads only what changed: one cell per changed overview, one atlas slot per new
//! terrain, one minimap slot per changed minimap. Rooms that come back into view after their
//! atlas slot was reused are uploaded again from that store. Drawing allocates nothing.
//!
//! # Wiring a canvas
//!
//! The viewer owns no event listeners and no frame loop; the app forwards DOM events and
//! calls [`Viewer::render`] from `requestAnimationFrame`:
//!
//! 1. Create the viewer with `Viewer::new` (wasm only) on a canvas in the document, call
//!    [`Viewer::set_scope`] (then [`Viewer::set_camera`] to restore a saved camera), then
//!    [`Viewer::resize`] with the canvas' CSS size (`clientWidth`, `clientHeight`) and
//!    `devicePixelRatio`, again whenever either changes (a `ResizeObserver`). `resize` sets
//!    the canvas' backing size itself; the app only styles its CSS size.
//! 2. Forward `pointerdown` / `pointermove` / `pointerup` (and `pointercancel`, as an up whose
//!    [`Pick`] is ignored) with `pointerId`, `offsetX`/`offsetY` and the button
//!    (`0` → [`PointerButton::Primary`], `1` → [`PointerButton::Middle`], `2` →
//!    [`PointerButton::Secondary`]); capture the pointer on down so that drags leaving the
//!    canvas keep reporting. Forward `wheel` (calling `preventDefault`) with `deltaY` in
//!    pixels (multiply line-mode deltas by about 16). Set CSS `touch-action: none` on the
//!    canvas so touch gestures reach the viewer, and suppress `contextmenu` for right-drag.
//! 3. Feed the world's data as described under Interest, and the signed-in user id to
//!    [`Viewer::set_user`].
//! 4. Request an animation frame after any of the calls above, and call [`Viewer::render`]
//!    in it; request another while [`FrameStats::animating`] is `true`. Nothing else
//!    changes the picture, so an idle view costs no frames.
//!
//! Gestures: dragging with the primary button pans in top-down and orbits in perspective;
//! the secondary and middle buttons pan in both. The wheel and a two-pointer pinch zoom
//! around the pointer (the pinch also pans with its midpoint). A press released without
//! dragging is a click: [`Viewer::pointer_up`] returns the room and tile under it and the
//! tile's objects (rooms with a detail scene), topmost first; the app decides whether to
//! [`Viewer::select`] it.
//!
//! ```ignore
//! let viewer = Rc::new(RefCell::new(Viewer::new(canvas.clone()).await?));
//! viewer.borrow_mut().set_scope(Scope::World(world.bounds()));
//! viewer.borrow_mut().resize(width, height, device_pixel_ratio);
//! // pointerup:
//! if let Some(pick) = viewer.borrow_mut().pointer_up(event.pointer_id(), x, y) {
//!     viewer.borrow_mut().select(Some((pick.room, pick.x, pick.y)));
//! }
//! // requestAnimationFrame callback:
//! let stats = viewer.borrow_mut().render()?;
//! if stats.animating {
//!     request_frame();
//! }
//! ```
//!
//! The scene, camera, picking, interest and interpolation are platform-independent and
//! compile natively (where the tests run); only `Viewer::new` needs a browser.

mod camera;
mod clock;
mod gpu;
mod mesh;
mod scene;
mod view;
mod world;

#[cfg(test)]
mod tests;

use glam::Vec2;
use overseer::RoomSnapshot;
use overseer::core::api::Terrain;
use overseer::map::{Interest, RoomCoord, RoomMap, RoomRect, RoomStat};

use crate::gpu::{Draw, Gpu};
use crate::view::View;

/// How long an object glides to a new tile, in milliseconds.
pub const MOVE_DURATION_MS: f64 = 250.0;
/// How long an animated [`Viewer::look_at`] or [`Viewer::fit`] takes, in milliseconds.
pub const FLIGHT_MS: f64 = 600.0;
/// Room size (CSS pixels) from which terrain is drawn: [`Lod::Terrain`].
pub const TERRAIN_PX_PER_ROOM: f32 = 24.0;
/// Room size (CSS pixels) from which minimaps are drawn: [`Lod::Minimap`].
pub const MINIMAP_PX_PER_ROOM: f32 = 100.0;
/// Room size (CSS pixels) from which detail scenes are drawn: [`Lod::Detail`].
pub const DETAIL_PX_PER_ROOM: f32 = 300.0;
/// Most minimaps wanted ([`Viewer::interest`]) and drawn at once.
pub const MINIMAP_ROOMS: usize = 64;
/// Most detail rooms wanted at once: the server's per-user limit of room subscriptions.
pub const DETAIL_ROOMS: usize = 2;
/// Largest terrain atlas side in texels (64 × 64 rooms).
const ATLAS_MAX_TEXELS: u32 = 3200;

/// The camera projection.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum ViewMode {
    /// Orthographic, looking straight down, north up.
    #[default]
    TopDown,
    /// Orbit camera around the same target.
    Perspective,
}

/// What a viewer shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    /// A rectangle of rooms (a shard's world); the camera zooms from all of it to one room.
    World(RoomRect),
    /// One room; the camera stays over it.
    Room(RoomCoord),
}

impl Scope {
    /// The rooms of the scope.
    #[must_use]
    pub const fn bounds(self) -> RoomRect {
        match self {
            Self::World(rect) => rect,
            Self::Room(room) => RoomRect {
                min: room,
                max: room,
            },
        }
    }
}

/// Level of detail, from far to near; see the crate documentation for the thresholds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Lod {
    /// Every room as a coloured cell.
    Overview,
    /// Terrain.
    Terrain,
    /// Terrain and minimaps.
    Minimap,
    /// Live room scenes.
    Detail,
}

impl Lod {
    /// The level of detail of rooms `px_per_room` CSS pixels wide.
    #[must_use]
    pub fn of(px_per_room: f32) -> Self {
        if px_per_room >= DETAIL_PX_PER_ROOM {
            Self::Detail
        } else if px_per_room >= MINIMAP_PX_PER_ROOM {
            Self::Minimap
        } else if px_per_room >= TERRAIN_PX_PER_ROOM {
            Self::Terrain
        } else {
            Self::Overview
        }
    }
}

/// The camera, to persist and restore ([`Viewer::camera`], [`Viewer::set_camera`]).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CameraState {
    /// The ground point looked at, in world tiles (`[x, y]`, see the crate documentation).
    pub centre: [f32; 2],
    /// Half of the visible height at the centre, in tiles (smaller is nearer).
    pub half_height: f32,
    /// The projection.
    pub mode: ViewMode,
    /// Orbit angle around the vertical axis, radians; 0 looks north.
    pub yaw: f32,
    /// Orbit elevation above the ground, radians.
    pub pitch: f32,
}

/// The button of a pointer press.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PointerButton {
    /// Left mouse button, touch or pen contact.
    Primary,
    /// Right mouse button.
    Secondary,
    /// Middle mouse button.
    Middle,
}

/// A clicked tile and what is on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pick {
    /// The room.
    pub room: RoomCoord,
    /// Tile x, `0..50`.
    pub x: u8,
    /// Tile y, `0..50`.
    pub y: u8,
    /// Ids of the objects on the tile, topmost first (empty without a detail scene).
    pub objects: Vec<String>,
}

/// Measurements of one [`Viewer::render`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameStats {
    /// CPU time spent in `render`, in milliseconds.
    pub cpu_ms: f32,
    /// Objects drawn (in detail scenes).
    pub objects: usize,
    /// Something still moves, flies or fades (or the frame was skipped): keep requesting
    /// frames.
    pub animating: bool,
}

/// Why the viewer cannot draw.
#[derive(Debug, thiserror::Error)]
pub enum ViewError {
    /// Neither WebGPU nor WebGL2 is usable on the canvas.
    #[error("no graphics adapter (WebGPU: {webgpu}; WebGL2: {webgl2})")]
    Unavailable {
        /// Why WebGPU failed.
        webgpu: String,
        /// Why WebGL2 failed.
        webgl2: String,
    },
    /// The canvas surface was lost; create a new viewer.
    #[error("the canvas surface was lost")]
    SurfaceLost,
    /// Acquiring the canvas texture raised a validation error.
    #[error("the canvas surface raised a validation error")]
    SurfaceValidation,
}

/// The world viewer of one canvas.
#[derive(Debug)]
pub struct Viewer {
    view: View,
    gpu: Gpu,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    backend: &'static str,
}

impl Viewer {
    /// A viewer drawing into `canvas`: WebGPU when the browser grants an adapter, WebGL2
    /// otherwise. It shows the room `E0S0` until [`Self::set_scope`]; call [`Self::resize`]
    /// next.
    ///
    /// # Errors
    ///
    /// [`ViewError::Unavailable`] when neither backend works, with the reason of each.
    #[cfg(target_arch = "wasm32")]
    pub async fn new(canvas: web_sys::HtmlCanvasElement) -> Result<Self, ViewError> {
        let size = (canvas.width().max(1), canvas.height().max(1));
        let webgpu = match web::connect(&canvas, wgpu::Backends::BROWSER_WEBGPU, size).await {
            Ok(viewer) => return Ok(viewer),
            Err(error) => error,
        };
        match web::connect(&canvas, wgpu::Backends::GL, size).await {
            Ok(viewer) => Ok(viewer),
            Err(webgl2) => Err(ViewError::Unavailable { webgpu, webgl2 }),
        }
    }

    /// The graphics backend in use: `"WebGPU"` or `"WebGL2"`.
    #[must_use]
    pub const fn backend(&self) -> &'static str {
        self.backend
    }

    /// Sets the canvas size in CSS pixels and the device pixel ratio; resizes the canvas'
    /// backing store (to at most the device's texture size). Until the camera is placed
    /// ([`Self::set_camera`], [`Self::look_at`], a gesture…) the scope stays framed.
    pub fn resize(&mut self, css_width: f32, css_height: f32, device_pixel_ratio: f32) {
        self.view.resize(css_width, css_height);
        let ratio = if device_pixel_ratio.is_finite() && device_pixel_ratio > 0.0 {
            device_pixel_ratio
        } else {
            1.0
        };
        let max = self.gpu.device.limits().max_texture_dimension_2d;
        let size = (
            physical(css_width * ratio, max),
            physical(css_height * ratio, max),
        );
        if size != (self.config.width, self.config.height) {
            (self.config.width, self.config.height) = size;
            self.surface.configure(&self.gpu.device, &self.config);
        }
        self.gpu.resize(size.0, size.1);
    }

    /// Shows another scope and frames it. Data of rooms outside it is kept (and shown again
    /// when a later scope includes them). Setting the current scope again changes nothing.
    pub fn set_scope(&mut self, scope: Scope) {
        self.view.set_scope(scope);
    }

    /// Sets the signed-in user's id: their rooms and objects are coloured as own.
    pub fn set_user(&mut self, user: Option<String>) {
        self.view.world.set_user(user);
    }

    /// Stores a room's terrain (and draws it when in view). Terrain is kept for the
    /// viewer's life.
    pub fn set_terrain(&mut self, room: RoomCoord, terrain: &Terrain) {
        self.view.world.set_terrain(room, terrain);
    }

    /// Sets (`None`: forgets) a room's overview: status, owner and reservation.
    pub fn set_stat(&mut self, room: RoomCoord, stat: Option<&RoomStat>) {
        self.view.world.set_stat(room, stat);
    }

    /// Shows (`Some`) or removes (`None`) a room's minimap. Beyond [`MINIMAP_ROOMS`]
    /// rooms, the farthest from the camera is dropped.
    pub fn set_minimap(&mut self, room: RoomCoord, map: Option<&RoomMap>) {
        self.view.world.set_minimap(room, map);
    }

    /// Shows a snapshot as its room's detail scene (and stores its terrain). The terrain mesh
    /// is rebuilt only when the terrain changed; objects that moved (by id) glide to their
    /// new tile. Rooms without world coordinates (`sim`) are ignored.
    pub fn set_room(&mut self, snapshot: &RoomSnapshot) {
        self.view.world.set_room(snapshot, clock::now());
    }

    /// Removes a room's detail scene (its minimap and terrain stay).
    pub fn clear_room(&mut self, room: RoomCoord) {
        self.view.world.clear_room(room);
    }

    /// The camera, to persist.
    #[must_use]
    pub const fn camera(&self) -> CameraState {
        self.view.camera_state()
    }

    /// Restores a camera, clamped to the scope (call after [`Self::set_scope`]).
    pub fn set_camera(&mut self, camera: CameraState) {
        self.view.set_camera_state(camera);
    }

    /// Centres a room, or one of its tiles, zooming in to at least the room's framing
    /// ([`Lod::Detail`] on most canvases); `animate` flies there over [`FLIGHT_MS`].
    pub fn look_at(&mut self, room: RoomCoord, tile: Option<(u8, u8)>, animate: bool) {
        self.view.look_at(room, tile, animate, clock::now());
    }

    /// Frames the whole scope; `animate` flies there over [`FLIGHT_MS`].
    pub fn fit(&mut self, animate: bool) {
        self.view.fit(animate, clock::now());
    }

    /// Switches the camera projection (target and zoom are kept).
    pub fn set_mode(&mut self, mode: ViewMode) {
        self.view.set_mode(mode);
    }

    /// The current level of detail.
    #[must_use]
    pub fn lod(&self) -> Lod {
        self.view.lod()
    }

    /// The room under the canvas centre (always the room of a room scope).
    #[must_use]
    pub fn focused_room(&self) -> Option<RoomCoord> {
        self.view.focused_room()
    }

    /// What the camera needs now (see the crate documentation).
    #[must_use]
    pub fn interest(&self) -> Interest {
        self.view.interest()
    }

    /// Moves the view by CSS pixels: positive `dx` shows more of the east, positive `dy`
    /// more of the south.
    pub fn pan_by(&mut self, dx: f32, dy: f32) {
        self.view.pan_by(Vec2::new(dx, dy));
    }

    /// Zooms around the canvas centre; `factor > 1` zooms in.
    pub fn zoom_by(&mut self, factor: f32) {
        self.view.zoom_by(factor);
    }

    /// A pointer was pressed at CSS pixel `(x, y)` of the canvas.
    pub fn pointer_down(&mut self, id: i32, x: f32, y: f32, button: PointerButton) {
        self.view.pointer_down(id, Vec2::new(x, y), button);
    }

    /// A pointer moved to CSS pixel `(x, y)`; pans, orbits or pinches while pressed.
    pub fn pointer_move(&mut self, id: i32, x: f32, y: f32) {
        self.view.pointer_move(id, Vec2::new(x, y));
    }

    /// A press released without dragging is a click: returns what is under it (`None`
    /// outside the scope).
    pub fn pointer_up(&mut self, id: i32, x: f32, y: f32) -> Option<Pick> {
        self.view.pointer_up(id, Vec2::new(x, y))
    }

    /// Zooms around CSS pixel `(x, y)`; positive `delta_y` (scrolling down) zooms out.
    pub fn wheel(&mut self, x: f32, y: f32, delta_y: f32) {
        self.view.wheel(Vec2::new(x, y), delta_y);
    }

    /// Highlights a room's tile (`None` clears the highlight).
    pub const fn select(&mut self, tile: Option<(RoomCoord, u8, u8)>) {
        self.view.select(tile);
    }

    /// Draws a frame.
    ///
    /// # Errors
    ///
    /// [`ViewError::SurfaceLost`] or [`ViewError::SurfaceValidation`] when the canvas cannot
    /// be drawn to. A frame the browser is not ready for is skipped and reported as
    /// `animating` so that the app asks again.
    pub fn render(&mut self) -> Result<FrameStats, ViewError> {
        let start = clock::now();
        let flying = self.view.advance(start);
        let fades = self.view.fades();
        // The atlas follows the camera only while terrain shows (the overview would scan
        // every room of the shard for nothing); detail scenes advance only while shown.
        let placing = fades.terrain > 0.0 && self.view.world.refresh();
        let (objects, moving) = if fades.detail > 0.0 {
            self.view.world.frame(start)
        } else {
            (0, false)
        };
        self.gpu.sync(&mut self.view.world);
        let stats = |animating| FrameStats {
            cpu_ms: elapsed_ms(start),
            objects,
            animating,
        };
        let (texture, suboptimal) = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) => (texture, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(texture) => (texture, true),
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(stats(true));
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.gpu.device, &self.config);
                return Ok(stats(true));
            }
            wgpu::CurrentSurfaceTexture::Lost => return Err(ViewError::SurfaceLost),
            wgpu::CurrentSurfaceTexture::Validation => return Err(ViewError::SurfaceValidation),
        };
        let output = texture.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.gpu.format),
            ..wgpu::TextureViewDescriptor::default()
        });
        let draw = Draw::of(&self.view, start);
        self.gpu.draw(&output, &draw, &self.view.world);
        self.gpu.present(texture);
        if suboptimal {
            self.surface.configure(&self.gpu.device, &self.config);
        }
        Ok(stats(flying || placing || moving))
    }
}

/// Milliseconds since `start` (a [`clock::now`] value).
#[expect(
    clippy::cast_possible_truncation,
    reason = "frame times are small; f32 is plenty"
)]
fn elapsed_ms(start: f64) -> f32 {
    (clock::now() - start) as f32
}

/// A CSS length times the pixel ratio as a backing-store length in `1..=max`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "clamped to 1..=max (a texture size) before the cast"
)]
fn physical(length: f32, max: u32) -> u32 {
    let length = if length.is_finite() { length } else { 1.0 };
    length.round().clamp(1.0, max as f32) as u32
}

/// Atlas slots per row for a device whose textures are at most `max_texture` texels wide.
#[cfg_attr(
    not(any(target_arch = "wasm32", test)),
    expect(
        dead_code,
        reason = "only the wasm canvas constructor and the tests create a renderer"
    )
)]
const fn atlas_per_row(max_texture: u32) -> u32 {
    let texels = if max_texture < ATLAS_MAX_TEXELS {
        max_texture
    } else {
        ATLAS_MAX_TEXELS
    };
    let per_row = texels / 50;
    if per_row == 0 { 1 } else { per_row }
}

/// Browser setup: adapter, device and canvas surface.
#[cfg(target_arch = "wasm32")]
mod web {
    use super::{Gpu, View, Viewer, atlas_per_row, gpu::DEPTH};

    /// A viewer on `canvas` through `backends` (WebGPU or WebGL2), or why it failed.
    pub async fn connect(
        canvas: &web_sys::HtmlCanvasElement,
        backends: wgpu::Backends,
        (width, height): (u32, u32),
    ) -> Result<Viewer, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let webgpu = backends == wgpu::Backends::BROWSER_WEBGPU;
        // WebGL2 needs the surface to pick an adapter. WebGPU does not: its surface (the
        // canvas' `webgpu` context) is created last, so that a failure before leaves the
        // canvas free for WebGL2.
        let mut surface = if webgpu {
            None
        } else {
            Some(create_surface(&instance, canvas)?)
        };
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: surface.as_ref(),
                ..wgpu::RequestAdapterOptions::default()
            })
            .await
            .map_err(|error| error.to_string())?;
        let backend = match adapter.get_info().backend {
            wgpu::Backend::BrowserWebGpu => "WebGPU",
            wgpu::Backend::Gl => "WebGL2",
            other => return Err(format!("unexpected backend {}", other.to_str())),
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("overseer-view"),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                    .using_resolution(adapter.limits()),
                ..wgpu::DeviceDescriptor::default()
            })
            .await
            .map_err(|error| error.to_string())?;
        let surface = match surface.take() {
            Some(surface) => surface,
            None => create_surface(&instance, canvas)?,
        };
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .first()
            .copied()
            .ok_or_else(|| "the canvas surface is not supported by the adapter".to_owned())?;
        // Colours are linear: draw through an sRGB view of the canvas texture.
        let view_format = format.add_srgb_suffix();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: capabilities
                .alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: if view_format == format {
                Vec::new()
            } else {
                vec![view_format]
            },
        };
        surface.configure(&device, &config);
        let multisampled = |format| {
            adapter
                .get_texture_format_features(format)
                .flags
                .sample_count_supported(4)
        };
        let samples = if multisampled(view_format) && multisampled(DEPTH) {
            4
        } else {
            1
        };
        let per_row = atlas_per_row(device.limits().max_texture_dimension_2d);
        let mut gpu = Gpu::new(device, queue, view_format, samples, per_row);
        gpu.resize(width, height);
        Ok(Viewer {
            view: View::new(per_row),
            gpu,
            surface,
            config,
            backend,
        })
    }

    fn create_surface(
        instance: &wgpu::Instance,
        canvas: &web_sys::HtmlCanvasElement,
    ) -> Result<wgpu::Surface<'static>, String> {
        instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
            .map_err(|error| error.to_string())
    }
}
