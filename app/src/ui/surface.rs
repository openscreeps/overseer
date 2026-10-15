//! A canvas with an `overseer-view` [`Viewer`]: its creation, the DOM events, the frame loop,
//! the feed subscription and the interest the viewer reports.
//!
//! Frames are drawn from `requestAnimationFrame` only when something changed (input, data,
//! a resize) and for as long as the viewer reports [`FrameStats::animating`], so an idle view
//! costs nothing. After a frame whose camera or size changed, the viewer's interest goes to
//! the feed at most every [`INTEREST_INTERVAL_MS`] (the last change always arrives).

use std::rc::Rc;

use leptos::html::Canvas;
use leptos::prelude::*;
use overseer::map::{DetailState, WorldEvent};
use overseer_view::{CameraState, FrameStats, Pick, PointerButton, Viewer};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{PointerEvent, ResizeObserver, WheelEvent};

use crate::feed::Subscription;
use crate::ui::spawn_scoped;

/// Wheel delta of one line (`deltaMode` 1), in pixels.
const LINE_PIXELS: f64 = 16.0;
/// Shortest time between two interests sent to the feed, in milliseconds.
pub const INTEREST_INTERVAL_MS: f64 = 200.0;

/// Called with the viewer: once created, after each frame, on a click.
type Hook<T> = Rc<dyn Fn(&mut Viewer, T)>;

/// What the owner of a surface does at each step.
pub struct Hooks {
    /// The viewer was created (set its scope, camera, mode, user).
    pub ready: Hook<()>,
    /// A frame was drawn.
    pub frame: Hook<FrameStats>,
    /// A click on the canvas (a press released without dragging).
    pub pick: Hook<Pick>,
}

/// One viewer canvas; `Copy` handles for event handlers and tasks.
#[derive(Clone, Copy)]
pub struct Surface {
    /// The viewer, once created.
    viewer: StoredValue<Option<Viewer>, LocalStorage>,
    hooks: StoredValue<Option<Rc<Hooks>>, LocalStorage>,
    /// The feed subscription, once attached.
    subscription: StoredValue<Option<Subscription>, LocalStorage>,
    /// A frame is requested.
    scheduled: StoredValue<bool>,
    /// Canvas CSS width, height and device pixel ratio last given to the viewer.
    size: StoredValue<(f32, f32, f32)>,
    /// Camera and size of the last interest sent, when it was sent, and whether a trailing
    /// send is pending.
    interest: StoredValue<InterestClock>,
    canvas: NodeRef<Canvas>,
    /// Why the view cannot be drawn.
    pub error: RwSignal<Option<String>>,
    /// The viewer exists.
    pub ready: RwSignal<bool>,
    /// Graphics backend, once created.
    pub backend: RwSignal<Option<&'static str>>,
    /// CPU time of the last frame, in milliseconds.
    pub frame_ms: RwSignal<Option<f32>>,
}

/// When the feed last heard from the viewer.
#[derive(Clone, Copy, Default)]
struct InterestClock {
    sent: Option<(CameraState, (f32, f32, f32))>,
    at: f64,
    trailing: bool,
}

impl Surface {
    /// A surface for the component being built; [`SurfaceCanvas`] shows it.
    pub fn new(hooks: Hooks) -> Self {
        Self {
            viewer: StoredValue::new_local(None),
            hooks: StoredValue::new_local(Some(Rc::new(hooks))),
            subscription: StoredValue::new_local(None),
            scheduled: StoredValue::new(false),
            size: StoredValue::new((0.0, 0.0, 0.0)),
            interest: StoredValue::new(InterestClock::default()),
            canvas: NodeRef::new(),
            error: RwSignal::new(None),
            ready: RwSignal::new(false),
            backend: RwSignal::new(None),
            frame_ms: RwSignal::new(None),
        }
    }

    /// Runs `change` on the viewer, if there is one (and the component is alive), and
    /// requests a frame.
    pub fn with_viewer<T>(self, change: impl FnOnce(&mut Viewer) -> T) -> Option<T> {
        let result = self
            .viewer
            .try_update_value(|viewer| viewer.as_mut().map(change))
            .flatten();
        if result.is_some() {
            self.request();
        }
        result
    }

    /// Replaces the feed subscription (dropping the previous one unsubscribes it) and sends
    /// the viewer's interest at once.
    pub fn attach(self, subscription: Option<Subscription>) {
        self.subscription.set_value(subscription);
        self.interest.set_value(InterestClock::default());
        self.send_interest();
    }

    /// Applies a feed event to the viewer.
    pub fn apply(self, event: &WorldEvent) {
        self.with_viewer(|viewer| apply(viewer, event));
    }

    /// Requests a frame unless one is pending.
    pub fn request(self) {
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
        let Some(hooks) = self.hooks.try_get_value().flatten() else {
            return;
        };
        let result = self
            .viewer
            .try_update_value(|viewer| {
                let viewer = viewer.as_mut()?;
                if size != self.size.get_value() {
                    viewer.resize(size.0, size.1, size.2);
                    self.size.set_value(size);
                }
                Some(
                    viewer
                        .render()
                        .inspect(|&stats| (hooks.frame)(viewer, stats)),
                )
            })
            .flatten();
        match result {
            Some(Ok(stats)) => {
                self.frame_ms.set(Some(stats.cpu_ms));
                self.after_frame();
                if stats.animating {
                    self.request();
                }
            }
            Some(Err(error)) => self.error.set(Some(format!("Cannot draw: {error}"))),
            None => {}
        }
    }

    /// Sends the interest when the camera or size changed, at most every
    /// [`INTEREST_INTERVAL_MS`]; schedules a trailing send otherwise.
    fn after_frame(self) {
        let Some(camera) = self.with_viewer_quiet(Viewer::camera) else {
            return;
        };
        let Some(clock) = self.interest.try_get_value() else {
            return;
        };
        let current = (camera, self.size.get_value());
        if clock.sent == Some(current) || clock.trailing {
            return;
        }
        let elapsed = now() - clock.at;
        if elapsed >= INTEREST_INTERVAL_MS {
            self.send_interest();
        } else {
            self.interest.update_value(|clock| clock.trailing = true);
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a delay of at most INTEREST_INTERVAL_MS"
            )]
            let wait = std::time::Duration::from_millis((INTEREST_INTERVAL_MS - elapsed) as u64);
            set_timeout(move || self.send_interest(), wait);
        }
    }

    /// Sends the viewer's interest to the feed now.
    fn send_interest(self) {
        let Some(Some((camera, interest))) = self
            .viewer
            .try_with_value(|viewer| viewer.as_ref().map(|v| (v.camera(), v.interest())))
        else {
            return;
        };
        self.interest.set_value(InterestClock {
            sent: Some((camera, self.size.get_value())),
            at: now(),
            trailing: false,
        });
        self.subscription.with_value(|subscription| {
            if let Some(subscription) = subscription {
                subscription.set_interest(interest);
            }
        });
    }

    /// Like [`Self::with_viewer`] without requesting a frame.
    fn with_viewer_quiet<T>(self, read: impl FnOnce(&Viewer) -> T) -> Option<T> {
        self.viewer
            .try_with_value(|viewer| viewer.as_ref().map(read))
            .flatten()
    }
}

/// Applies a feed event to `viewer`.
pub fn apply(viewer: &mut Viewer, event: &WorldEvent) {
    match event {
        WorldEvent::Terrain(room, terrain) => viewer.set_terrain(*room, terrain),
        WorldEvent::Stats(stats) => {
            for (room, stat) in stats {
                viewer.set_stat(*room, Some(stat));
            }
        }
        WorldEvent::Minimap(room, minimap) => viewer.set_minimap(*room, Some(minimap)),
        WorldEvent::MinimapGone(room) => viewer.set_minimap(*room, None),
        WorldEvent::Detail(_, DetailState::Live(snapshot)) => viewer.set_room(snapshot),
        WorldEvent::Detail(room, DetailState::Refused(_)) | WorldEvent::DetailGone(room) => {
            viewer.clear_room(*room);
        }
        WorldEvent::Detail(_, DetailState::Pending) => {}
    }
}

/// The canvas of `surface`: creates the viewer once mounted and forwards pointer and wheel
/// events.
#[component]
pub fn SurfaceCanvas(
    /// The surface.
    surface: Surface,
    /// Accessible name of the canvas.
    #[prop(into)]
    label: Signal<String>,
    /// Accessible description (gestures).
    #[prop(into)]
    description: String,
    /// `data-testid` of the canvas (its accessible name changes with the view).
    testid: &'static str,
) -> impl IntoView {
    start(surface);
    on_cleanup(move || {
        // Drop the subscription first: it must not outlive the viewer.
        surface.subscription.try_set_value(None);
    });
    let canvas = surface.canvas;
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
        surface.with_viewer(|viewer| viewer.pointer_down(event.pointer_id(), x, y, button));
    };
    let pointer_move = move |event: PointerEvent| {
        if event.buttons() == 0 {
            return;
        }
        let (x, y) = position(&event);
        surface.with_viewer(|viewer| viewer.pointer_move(event.pointer_id(), x, y));
    };
    let pointer_up = move |event: PointerEvent| {
        let (x, y) = position(&event);
        let Some(hooks) = surface.hooks.try_get_value().flatten() else {
            return;
        };
        surface.with_viewer(|viewer| {
            if let Some(pick) = viewer.pointer_up(event.pointer_id(), x, y) {
                (hooks.pick)(viewer, pick);
            }
        });
    };
    let pointer_cancel = move |event: PointerEvent| {
        let (x, y) = position(&event);
        surface.with_viewer(|viewer| viewer.pointer_up(event.pointer_id(), x, y));
    };
    let wheel = move |event: WheelEvent| {
        event.prevent_default();
        let scale = match event.delta_mode() {
            WheelEvent::DOM_DELTA_LINE => LINE_PIXELS,
            WheelEvent::DOM_DELTA_PAGE => f64::from(surface.size.get_value().1),
            _ => 1.0,
        };
        #[expect(
            clippy::cast_possible_truncation,
            reason = "wheel deltas are small pixel counts"
        )]
        let delta = (event.delta_y() * scale) as f32;
        let (x, y) = (event.offset_x(), event.offset_y());
        #[expect(clippy::cast_precision_loss, reason = "CSS pixel offsets are small")]
        surface.with_viewer(|viewer| viewer.wheel(x as f32, y as f32, delta));
    };
    let description_id = format!("canvas-help-{}", next_id());
    view! {
        <canvas
            node_ref=canvas
            class="surface"
            data-testid=testid
            tabindex="0"
            aria-label=label
            aria-describedby=description_id.clone()
            on:pointerdown=pointer_down
            on:pointermove=pointer_move
            on:pointerup=pointer_up
            on:pointercancel=pointer_cancel
            on:wheel=wheel
            on:contextmenu=|event| event.prevent_default()
        ></canvas>
        <p id=description_id class="visually-hidden">{description}</p>
    }
}

/// A unique number for element ids.
fn next_id() -> u64 {
    thread_local! {
        static NEXT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }
    NEXT.with(|next| {
        let id = next.get();
        next.set(id + 1);
        id
    })
}

/// Creates the viewer once the canvas is in the document, and requests a frame whenever the
/// canvas or the window is resized.
fn start(surface: Surface) {
    Effect::new(move |created: Option<bool>| {
        if created == Some(true) {
            return true;
        }
        let Some(element) = surface.canvas.get() else {
            return false;
        };
        StoredValue::new_local(ResizeGuard::observe(&element, surface));
        let resize = window_event_listener(leptos::ev::resize, move |_| surface.request());
        on_cleanup(move || resize.remove());
        spawn_scoped(async move {
            match create_viewer(element).await {
                Ok(mut viewer) => {
                    if let Some(hooks) = surface.hooks.try_get_value().flatten() {
                        (hooks.ready)(&mut viewer, ());
                    }
                    surface.backend.set(Some(viewer.backend()));
                    surface.viewer.set_value(Some(viewer));
                    surface.ready.set(true);
                    surface.request();
                }
                Err(error) => surface.error.set(Some(error)),
            }
        });
        true
    });
}

/// CSS pixel position of a pointer event on the canvas.
#[expect(clippy::cast_precision_loss, reason = "CSS pixel offsets are small")]
fn position(event: &PointerEvent) -> (f32, f32) {
    (event.offset_x() as f32, event.offset_y() as f32)
}

/// `performance.now()` (or `Date.now()`), in milliseconds.
fn now() -> f64 {
    web_sys::window()
        .and_then(|window| window.performance())
        .map_or_else(js_sys::Date::now, |performance| performance.now())
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
    fn observe(element: &web_sys::HtmlCanvasElement, surface: Surface) -> Self {
        let callback = Closure::<dyn FnMut()>::new(move || surface.request());
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
    Err("the viewer needs a browser".to_owned())
}
