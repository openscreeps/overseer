//! The `overseer-view` canvas: viewer lifetime, the room watch, DOM events and the frame loop.
//!
//! Frames are drawn from `requestAnimationFrame` only when something changed (input, a room
//! update, a resize) and for as long as the viewer reports [`FrameStats::animating`], so an
//! idle room costs nothing.

use std::sync::Arc;

use leptos::html::Canvas;
use leptos::prelude::*;
use overseer::{Overseer, RoomSnapshot};
use overseer_view::{FrameStats, PointerButton, ViewMode, Viewer};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{PointerEvent, ResizeObserver, WheelEvent};

use super::RoomTarget;
use crate::session;
use crate::state::{AppState, RoomStats};
use crate::ui::spawn_scoped;

/// Wheel delta of one line (`deltaMode` 1), in pixels.
const LINE_PIXELS: f64 = 16.0;
/// Wheel delta of a zoom button press, in pixels.
const ZOOM_STEP: f32 = 240.0;

/// The canvas, its toolbar and the frame loop.
#[component]
pub fn RoomCanvas(
    overseer: Overseer,
    target: RwSignal<Option<RoomTarget>>,
    snapshot: RwSignal<Option<Arc<RoomSnapshot>>>,
    selected: RwSignal<Option<(u8, u8)>>,
) -> impl IntoView {
    let state = AppState::expect();
    let canvas = NodeRef::<Canvas>::new();
    let frames = Frames {
        viewer: StoredValue::new_local(None),
        scheduled: StoredValue::new(false),
        size: StoredValue::new((0.0, 0.0, 0.0)),
        canvas,
        state,
        error: RwSignal::new(None),
    };
    let mode = RwSignal::new(ViewMode::TopDown);
    on_cleanup(move || state.room_stats.set(RoomStats::default()));
    start_viewer(
        frames,
        mode,
        snapshot,
        overseer.user_id().map(str::to_owned),
    );
    watch_room(frames, overseer, target, snapshot, selected);

    let set_mode = move |next: ViewMode| {
        mode.set(next);
        frames.with_viewer(|viewer| viewer.set_mode(next));
        frames.request();
    };
    let zoom = move |delta: f32| {
        let (width, height, _) = frames.size.get_value();
        frames.with_viewer(|viewer| viewer.wheel(width / 2.0, height / 2.0, delta));
        frames.request();
    };
    let pointer_down = move |event: PointerEvent| {
        event.prevent_default();
        if let Some(element) = canvas.get_untracked() {
            let _ = element.focus();
            let _ = element.set_pointer_capture(event.pointer_id());
        }
        let button = match event.button() {
            1 => PointerButton::Middle,
            2 => PointerButton::Secondary,
            _ => PointerButton::Primary,
        };
        let (x, y) = position(&event);
        frames.with_viewer(|viewer| viewer.pointer_down(event.pointer_id(), x, y, button));
        frames.request();
    };
    let pointer_move = move |event: PointerEvent| {
        if event.buttons() == 0 {
            return;
        }
        let (x, y) = position(&event);
        frames.with_viewer(|viewer| viewer.pointer_move(event.pointer_id(), x, y));
        frames.request();
    };
    let pointer_up = move |event: PointerEvent| {
        let (x, y) = position(&event);
        let pick = frames
            .with_viewer(|viewer| {
                let pick = viewer.pointer_up(event.pointer_id(), x, y);
                if let Some(pick) = &pick {
                    viewer.select(Some((pick.x, pick.y)));
                }
                pick
            })
            .flatten();
        if let Some(pick) = pick {
            selected.set(Some((pick.x, pick.y)));
        }
        frames.request();
    };
    let pointer_cancel = move |event: PointerEvent| {
        let (x, y) = position(&event);
        frames.with_viewer(|viewer| viewer.pointer_up(event.pointer_id(), x, y));
        frames.request();
    };
    let wheel = move |event: WheelEvent| {
        event.prevent_default();
        let scale = match event.delta_mode() {
            WheelEvent::DOM_DELTA_LINE => LINE_PIXELS,
            WheelEvent::DOM_DELTA_PAGE => f64::from(frames.size.get_value().1),
            _ => 1.0,
        };
        #[expect(
            clippy::cast_possible_truncation,
            reason = "wheel deltas are small pixel counts"
        )]
        let delta = (event.delta_y() * scale) as f32;
        let (x, y) = (event.offset_x(), event.offset_y());
        #[expect(clippy::cast_precision_loss, reason = "CSS pixel offsets are small")]
        frames.with_viewer(|viewer| viewer.wheel(x as f32, y as f32, delta));
        frames.request();
    };
    let label = move || {
        target.get().map_or_else(
            || "Room view".to_owned(),
            |room| {
                format!(
                    "Room {}: drag to pan or orbit, scroll to zoom, click a tile to select it",
                    room.label()
                )
            },
        )
    };
    view! {
        <section class="panel viewer" aria-label="Room view">
            <div class="toolbar">
                <div class="segmented" role="group" aria-label="View mode">
                    <button
                        type="button"
                        aria-pressed=move || (mode.get() == ViewMode::TopDown).to_string()
                        on:click=move |_| set_mode(ViewMode::TopDown)
                    >
                        "2D"
                    </button>
                    <button
                        type="button"
                        aria-pressed=move || (mode.get() == ViewMode::Perspective).to_string()
                        on:click=move |_| set_mode(ViewMode::Perspective)
                    >
                        "3D"
                    </button>
                </div>
                <div class="segmented" role="group" aria-label="Zoom">
                    <button type="button" aria-label="Zoom in" on:click=move |_| zoom(-ZOOM_STEP)>
                        "+"
                    </button>
                    <button type="button" aria-label="Zoom out" on:click=move |_| zoom(ZOOM_STEP)>
                        "−"
                    </button>
                </div>
            </div>
            <div class="canvas-box">
                <canvas
                    node_ref=canvas
                    tabindex="0"
                    aria-label=label
                    on:pointerdown=pointer_down
                    on:pointermove=pointer_move
                    on:pointerup=pointer_up
                    on:pointercancel=pointer_cancel
                    on:wheel=wheel
                    on:contextmenu=|event| event.prevent_default()
                ></canvas>
            </div>
            <p class="error" role="alert">{move || frames.error.get()}</p>
        </section>
    }
}

/// Creates the viewer once the canvas is in the document, and requests a frame whenever the
/// canvas or the window is resized.
fn start_viewer(
    frames: Frames,
    mode: RwSignal<ViewMode>,
    snapshot: RwSignal<Option<Arc<RoomSnapshot>>>,
    user: Option<String>,
) {
    Effect::new(move |created: Option<bool>| {
        if created == Some(true) {
            return true;
        }
        let Some(element) = frames.canvas.get() else {
            return false;
        };
        StoredValue::new_local(ResizeGuard::observe(&element, frames));
        let resize = window_event_listener(leptos::ev::resize, move |_| frames.request());
        on_cleanup(move || resize.remove());
        let user = user.clone();
        spawn_scoped(async move {
            match create_viewer(element).await {
                Ok(mut viewer) => {
                    viewer.set_user(user);
                    viewer.set_mode(mode.get_untracked());
                    if let Some(latest) = snapshot.get_untracked() {
                        viewer.set_room(&latest);
                    }
                    let backend = viewer.backend();
                    frames.viewer.set_value(Some(viewer));
                    frames
                        .state
                        .room_stats
                        .update(|stats| stats.backend = Some(backend));
                    frames.request();
                }
                Err(error) => frames.error.set(Some(error)),
            }
        });
        true
    });
}

/// Watches the target room and feeds every snapshot to the viewer; a new target replaces the
/// watch.
fn watch_room(
    frames: Frames,
    overseer: Overseer,
    target: RwSignal<Option<RoomTarget>>,
    snapshot: RwSignal<Option<Arc<RoomSnapshot>>>,
    selected: RwSignal<Option<(u8, u8)>>,
) {
    Effect::new(move |_| {
        let Some(room) = target.get() else {
            return;
        };
        let rooms = overseer.rooms();
        selected.set(None);
        snapshot.set(None);
        frames.error.set(None);
        frames.with_viewer(|viewer| viewer.select(None));
        frames.state.room_stats.update(|stats| {
            stats.room = Some(room.label());
            stats.game_time = None;
        });
        spawn_scoped(async move {
            let watch = match &room.shard {
                Some(shard) => rooms.watch_on(shard, &room.room).await,
                None => rooms.watch(&room.room).await,
            };
            let mut watch = match watch {
                Ok(watch) => watch,
                Err(error) => {
                    frames.error.set(Some(format!(
                        "Cannot watch {}: {}",
                        room.label(),
                        session::describe(&error)
                    )));
                    return;
                }
            };
            let show = |latest: Arc<RoomSnapshot>| {
                frames.error.set(latest.subscription_error.clone());
                frames.with_viewer(|viewer| viewer.set_room(&latest));
                frames
                    .state
                    .room_stats
                    .update(|stats| stats.game_time = latest.game_time);
                snapshot.set(Some(latest));
                frames.request();
            };
            if let Some(latest) = watch.latest() {
                show(latest);
            }
            while let Ok(latest) = watch.changed().await {
                show(latest);
            }
        });
    });
}

/// CSS pixel position of a pointer event on the canvas.
#[expect(clippy::cast_precision_loss, reason = "CSS pixel offsets are small")]
fn position(event: &PointerEvent) -> (f32, f32) {
    (event.offset_x() as f32, event.offset_y() as f32)
}

/// The viewer and its frame loop; `Copy` handles for event handlers and tasks.
#[derive(Clone, Copy)]
struct Frames {
    /// The viewer, once created.
    viewer: StoredValue<Option<Viewer>, LocalStorage>,
    /// A frame is requested.
    scheduled: StoredValue<bool>,
    /// Canvas CSS width, height and device pixel ratio last given to the viewer.
    size: StoredValue<(f32, f32, f32)>,
    canvas: NodeRef<Canvas>,
    state: AppState,
    /// Why the room cannot be shown.
    error: RwSignal<Option<String>>,
}

impl Frames {
    /// Runs `change` on the viewer, if there is one (and the component is alive).
    fn with_viewer<T>(self, change: impl FnOnce(&mut Viewer) -> T) -> Option<T> {
        self.viewer
            .try_update_value(|viewer| viewer.as_mut().map(change))
            .flatten()
    }

    /// Requests a frame unless one is pending.
    fn request(self) {
        if self.scheduled.try_get_value() != Some(false) {
            return;
        }
        self.scheduled.set_value(true);
        request_animation_frame(move || self.draw());
    }

    /// Draws a frame: follows the canvas size first, asks for another while animating.
    fn draw(self) {
        if self
            .scheduled
            .try_update_value(|scheduled| *scheduled = false)
            .is_none()
        {
            return;
        }
        let Some(canvas) = self.canvas.get_untracked() else {
            return;
        };
        #[expect(clippy::cast_precision_loss, reason = "CSS pixel sizes are small")]
        let size = (
            canvas.client_width() as f32,
            canvas.client_height() as f32,
            device_pixel_ratio(),
        );
        // Remembered only once a viewer took it: the first frames may come before it exists.
        if size != self.size.get_value()
            && self
                .with_viewer(|viewer| viewer.resize(size.0, size.1, size.2))
                .is_some()
        {
            self.size.set_value(size);
        }
        match self.with_viewer(Viewer::render) {
            Some(Ok(FrameStats {
                cpu_ms,
                objects,
                animating,
            })) => {
                self.state.room_stats.update(|stats| {
                    stats.frame_ms = Some(cpu_ms);
                    stats.objects = objects;
                });
                if animating {
                    self.request();
                }
            }
            Some(Err(error)) => self
                .error
                .set(Some(format!("Cannot draw the room: {error}"))),
            None => {}
        }
    }
}

/// `window.devicePixelRatio`.
#[expect(clippy::cast_possible_truncation, reason = "pixel ratios are small")]
fn device_pixel_ratio() -> f32 {
    web_sys::window().map_or(1.0, |window| window.device_pixel_ratio() as f32)
}

/// A `ResizeObserver` on the canvas requesting frames; disconnects when dropped.
struct ResizeGuard {
    observer: Option<ResizeObserver>,
    /// Kept alive while observing.
    _callback: Closure<dyn FnMut()>,
}

impl ResizeGuard {
    /// Observes `element`.
    fn observe(element: &web_sys::HtmlCanvasElement, frames: Frames) -> Self {
        let callback = Closure::<dyn FnMut()>::new(move || frames.request());
        let observer = ResizeObserver::new(callback.as_ref().unchecked_ref()).ok();
        if let Some(observer) = &observer {
            observer.observe(element);
        }
        Self {
            observer,
            _callback: callback,
        }
    }
}

impl Drop for ResizeGuard {
    fn drop(&mut self) {
        if let Some(observer) = &self.observer {
            observer.disconnect();
        }
    }
}

/// A viewer on `canvas` (WebGPU, else WebGL2).
#[cfg(target_arch = "wasm32")]
async fn create_viewer(canvas: web_sys::HtmlCanvasElement) -> Result<Viewer, String> {
    Viewer::new(canvas).await.map_err(|error| error.to_string())
}

/// Natively there is no canvas to draw into.
#[cfg(not(target_arch = "wasm32"))]
#[expect(clippy::unused_async, reason = "the same signature as in browsers")]
async fn create_viewer(_canvas: web_sys::HtmlCanvasElement) -> Result<Viewer, String> {
    Err("the room viewer needs a browser".to_owned())
}
