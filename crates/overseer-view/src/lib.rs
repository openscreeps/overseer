//! Room viewer of the Overseer app: draws live [`RoomSnapshot`]s with wgpu into a browser
//! `<canvas>`, through WebGPU when the browser grants an adapter and WebGL2 otherwise.
//!
//! # Scene
//!
//! Both [`ViewMode`]s draw the same 3D scene: the terrain (plains and swamps flat, natural
//! walls raised) and every object as a simple shape whose size and colour follow its kind;
//! owned objects take the colour of their owner: the signed-in user ([`Viewer::set_user`]),
//! other players, the Invader NPC (user `2`) or the Source Keeper NPC (user `3`). The
//! [`ViewMode::TopDown`] camera is orthographic and looks straight down; the
//! [`ViewMode::Perspective`] camera orbits the same target at a distance framing the same
//! zoom. A selected tile ([`Viewer::select`]) is framed in yellow, drawn over everything.
//!
//! Terrain geometry is rebuilt only when a snapshot brings a different terrain. Objects are
//! matched by id between snapshots: one that moved glides to its new tile over
//! [`MOVE_DURATION_MS`] ([`FrameStats::animating`] is `true` meanwhile); new objects appear
//! and removed objects disappear at once, as does everything when the room changes.
//!
//! # Wiring a canvas
//!
//! The viewer owns no event listeners and no frame loop; the app forwards DOM events and
//! calls [`Viewer::render`] from `requestAnimationFrame`:
//!
//! 1. Create the viewer with `Viewer::new` (wasm only) on a canvas in the document, then
//!    call [`Viewer::resize`] with the canvas' CSS size (`clientWidth`, `clientHeight`) and
//!    `devicePixelRatio`, again whenever either changes (a `ResizeObserver`). `resize` sets
//!    the canvas' backing size itself; the app only styles its CSS size.
//! 2. Forward `pointerdown` / `pointermove` / `pointerup` (and `pointercancel`, as an up whose
//!    [`Pick`] is ignored) with `pointerId`, `offsetX`/`offsetY` and the button
//!    (`0` → [`PointerButton::Primary`], `1` → [`PointerButton::Middle`], `2` →
//!    [`PointerButton::Secondary`]); capture the pointer on down so that drags leaving the
//!    canvas keep reporting. Forward `wheel` (calling `preventDefault`) with `deltaY` in
//!    pixels (multiply line-mode deltas by about 16). Set CSS `touch-action: none` on the
//!    canvas so touch gestures reach the viewer, and suppress `contextmenu` for right-drag.
//! 3. Feed each [`RoomSnapshot`] of the room watch to [`Viewer::set_room`], and the
//!    signed-in user id to [`Viewer::set_user`].
//! 4. Request an animation frame after any of the calls above, and call [`Viewer::render`]
//!    in it; request another while [`FrameStats::animating`] is `true`. Nothing else
//!    changes the picture, so an idle room costs no frames.
//!
//! Gestures: dragging with the primary button pans in top-down and orbits in perspective;
//! the secondary and middle buttons pan in both. The wheel and a two-pointer pinch zoom
//! around the pointer (the pinch also pans with its midpoint). A press released without
//! dragging is a click: [`Viewer::pointer_up`] returns the tile under it and its objects,
//! topmost first; the app decides whether to [`Viewer::select`] it.
//!
//! ```ignore
//! let viewer = Rc::new(RefCell::new(Viewer::new(canvas.clone()).await?));
//! viewer.borrow_mut().resize(width, height, device_pixel_ratio);
//! // pointerdown:
//! viewer.borrow_mut().pointer_down(event.pointer_id(), x, y, PointerButton::Primary);
//! // pointerup:
//! if let Some(pick) = viewer.borrow_mut().pointer_up(event.pointer_id(), x, y) {
//!     viewer.borrow_mut().select(Some((pick.x, pick.y)));
//! }
//! // requestAnimationFrame callback:
//! let stats = viewer.borrow_mut().render()?;
//! if stats.animating {
//!     request_frame();
//! }
//! ```
//!
//! The scene, camera, picking and interpolation are platform-independent and compile
//! natively (where the tests run); only `Viewer::new` needs a browser.

mod camera;
mod clock;
mod gpu;
mod mesh;
mod scene;
mod view;

#[cfg(test)]
mod tests;

use glam::Vec2;
use overseer::RoomSnapshot;

use crate::gpu::Gpu;
use crate::view::View;

/// How long an object glides to a new tile, in milliseconds.
pub const MOVE_DURATION_MS: f64 = 250.0;

/// The camera projection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ViewMode {
    /// Orthographic, looking straight down, north up.
    #[default]
    TopDown,
    /// Orbit camera around the same target.
    Perspective,
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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pick {
    /// Tile x, `0..50`.
    pub x: u8,
    /// Tile y, `0..50`.
    pub y: u8,
    /// Ids of the objects on the tile, topmost first.
    pub objects: Vec<String>,
}

/// Measurements of one [`Viewer::render`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameStats {
    /// CPU time spent in `render`, in milliseconds.
    pub cpu_ms: f32,
    /// Objects drawn.
    pub objects: usize,
    /// Objects are still moving (or the frame was skipped): keep requesting frames.
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

/// The room viewer of one canvas.
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
    /// otherwise. Call [`Self::resize`] next.
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
    /// backing store (to at most the device's texture size).
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

    /// Sets the signed-in user's id: their objects are coloured as own.
    pub fn set_user(&mut self, user: Option<String>) {
        self.view.scene.set_user(user);
    }

    /// Shows a snapshot. The terrain mesh is rebuilt only when the terrain changed; objects
    /// that moved (by id) glide to their new tile.
    pub fn set_room(&mut self, snapshot: &RoomSnapshot) {
        self.view.scene.set_room(snapshot, clock::now());
        self.gpu.sync(&self.view.scene);
    }

    /// Switches the camera projection (target and zoom are kept).
    pub const fn set_mode(&mut self, mode: ViewMode) {
        self.view.set_mode(mode);
    }

    /// A pointer was pressed at CSS pixel `(x, y)` of the canvas.
    pub fn pointer_down(&mut self, id: i32, x: f32, y: f32, button: PointerButton) {
        self.view.pointer_down(id, Vec2::new(x, y), button);
    }

    /// A pointer moved to CSS pixel `(x, y)`; pans, orbits or pinches while pressed.
    pub fn pointer_move(&mut self, id: i32, x: f32, y: f32) {
        self.view.pointer_move(id, Vec2::new(x, y));
    }

    /// A press released without dragging is a click: returns what is under it.
    pub fn pointer_up(&mut self, id: i32, x: f32, y: f32) -> Option<Pick> {
        self.view.pointer_up(id, Vec2::new(x, y))
    }

    /// Zooms around CSS pixel `(x, y)`; positive `delta_y` (scrolling down) zooms out.
    pub fn wheel(&mut self, x: f32, y: f32, delta_y: f32) {
        self.view.wheel(Vec2::new(x, y), delta_y);
    }

    /// Highlights a tile (`None` clears the highlight).
    pub const fn select(&mut self, tile: Option<(u8, u8)>) {
        self.view.scene.select(tile);
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
        let frame = self.view.scene.frame(start);
        let (texture, suboptimal) = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) => (texture, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(texture) => (texture, true),
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(self.stats(start, true));
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.gpu.device, &self.config);
                return Ok(self.stats(start, true));
            }
            wgpu::CurrentSurfaceTexture::Lost => return Err(ViewError::SurfaceLost),
            wgpu::CurrentSurfaceTexture::Validation => return Err(ViewError::SurfaceValidation),
        };
        let output = texture.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.gpu.format),
            ..wgpu::TextureViewDescriptor::default()
        });
        let view_projection = self.view.camera.view_projection();
        self.gpu
            .draw(&output, view_projection, &self.view.scene, &frame);
        self.gpu.present(texture);
        if suboptimal {
            self.surface.configure(&self.gpu.device, &self.config);
        }
        Ok(self.stats(start, frame.animating))
    }

    fn stats(&self, start: f64, animating: bool) -> FrameStats {
        FrameStats {
            cpu_ms: elapsed_ms(start),
            objects: self.view.scene.object_count(),
            animating,
        }
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

/// Browser setup: adapter, device and canvas surface.
#[cfg(target_arch = "wasm32")]
mod web {
    use super::{Gpu, View, Viewer, gpu::DEPTH};

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
        let mut gpu = Gpu::new(device, queue, view_format, samples);
        gpu.resize(width, height);
        Ok(Viewer {
            view: View::default(),
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
