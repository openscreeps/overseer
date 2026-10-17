//! A canvas with an `overseer-view` [`Viewer`]: its creation, the DOM events, the frame loop,
//! the feed subscription and the interest the viewer reports.
//!
//! Frames are drawn from `requestAnimationFrame` only when something changed (input, a
//! resize, data of the rooms in view: [`shows`]) and for as long as the viewer reports
//! [`FrameStats::animating`], so an idle view costs nothing, and neither does the rest of
//! the world loading in the background (its data waits in the viewer for the next frame).
//! After a frame whose camera or size changed, the viewer's interest goes to
//! the feed at most every [`INTEREST_INTERVAL_MS`] (the last change always arrives), and the
//! [`Hooks::interest`] hook sees the viewer at each send: one camera change drives everything
//! that follows the view.
//!
//! A mouse or pen moving over the canvas without a button pressed hovers it: the
//! [`Hooks::hover`] hook learns the point at most once per animation frame, and again when
//! the camera moves under a resting pointer; it learns `None` when the pointer leaves, presses
//! (dragging hides the hover) or is a touch.

use std::rc::Rc;

use leptos::html::Canvas;
use leptos::prelude::*;
use overseer::map::{DetailState, RoomCoord, RoomRect, WorldEvent};
use overseer_view::{CameraState, FrameStats, Pick, PointerButton, Viewer};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{PointerEvent, ResizeObserver, WheelEvent};

use crate::feed::Subscription;
use crate::ui::{next_id, spawn_scoped};

/// Wheel delta of one line (`deltaMode` 1), in pixels.
const LINE_PIXELS: f64 = 16.0;
/// Shortest time between two interests sent to the feed, in milliseconds.
pub const INTEREST_INTERVAL_MS: f64 = 200.0;

/// Called with the viewer: once created, after each frame, on a click.
type Hook<T> = Rc<dyn Fn(&mut Viewer, T)>;

/// Called with the viewer and the hovered CSS point (`None`: nothing hovered); returns
/// whether the view changed and needs a frame.
pub type HoverHook = Rc<dyn Fn(&mut Viewer, Option<(f32, f32)>) -> bool>;

/// Called with the viewer whenever its interest goes to the feed.
pub type InterestHook = Rc<dyn Fn(&Viewer)>;

/// What the owner of a surface does at each step.
pub struct Hooks {
    /// The viewer was created (set its scope, camera, mode, user).
    pub ready: Hook<()>,
    /// A frame was drawn.
    pub frame: Hook<FrameStats>,
    /// A click on the canvas (a press released without dragging).
    pub pick: Hook<Pick>,
    /// The hovered point changed (see the module docs); `None` ignores hovering.
    pub hover: Option<HoverHook>,
    /// The viewer's interest went to the feed (see the module docs); `None`: nothing else
    /// follows the view.
    pub interest: Option<InterestHook>,
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
    /// Where a resting mouse or pen hovers, in CSS pixels.
    pointer: StoredValue<Option<(f32, f32)>>,
    /// A hover update is requested, and the camera of the last one.
    hover: StoredValue<(bool, Option<CameraState>)>,
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
    /// The camera, canvas size and [`Viewer::interest_revision`] of the latest send.
    sent: Option<(CameraState, (f32, f32, f32), u64)>,
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
            pointer: StoredValue::new(None),
            hover: StoredValue::new((false, None)),
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

    /// Applies a feed event to the viewer; requests a frame when it changes what the canvas
    /// shows ([`shows`]), else sends the interest when the event changed what the viewer
    /// needs ([`Viewer::interest_revision`]: portals of rooms out of view, say).
    pub fn apply(self, event: &WorldEvent) {
        let outcome = self
            .viewer
            .try_update_value(|viewer| {
                viewer.as_mut().map(|viewer| {
                    let (view, revision) = (viewer.visible_rooms(), viewer.interest_revision());
                    apply(viewer, event);
                    (shows(view, event), viewer.interest_revision() != revision)
                })
            })
            .flatten();
        match outcome {
            Some((true, _)) => self.request(),
            Some((false, true)) => self.follow_interest(),
            _ => {}
        }
    }

    /// Requests a frame unless one is pending.
    pub fn request(self) {
        if self.scheduled.try_get_value() != Some(false) {
            return;
        }
        self.scheduled.set_value(true);
        request_animation_frame(move || self.draw());
    }

    /// Moves the keyboard focus to the canvas (after a search, say) without a focus ring:
    /// the user did not tab there. The ring comes back once the canvas loses focus.
    pub fn focus(self) {
        if let Some(canvas) = self.canvas.get_untracked() {
            let _ = canvas.set_attribute("data-quiet-focus", "");
            let _ = canvas.focus();
        }
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

    /// Updates the hover when the camera moved under a hovering pointer, and follows the
    /// interest ([`Self::follow_interest`]).
    fn after_frame(self) {
        let Some(camera) = self.with_viewer_quiet(Viewer::camera) else {
            return;
        };
        if self.pointer.get_value().is_some() && self.hover.get_value().1 != Some(camera) {
            self.request_hover();
        }
        self.follow_interest();
    }

    /// Sends the interest when the camera, size or [`Viewer::interest_revision`] changed
    /// (the hovered or selected room, portals arriving whose destinations the map wants), at
    /// most every [`INTEREST_INTERVAL_MS`]; schedules a trailing send otherwise.
    fn follow_interest(self) {
        let Some((camera, revision)) =
            self.with_viewer_quiet(|viewer| (viewer.camera(), viewer.interest_revision()))
        else {
            return;
        };
        let Some(clock) = self.interest.try_get_value() else {
            return;
        };
        let current = (camera, self.size.get_value(), revision);
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

    /// Sets where the pointer hovers (`None`: nowhere) and requests a hover update; nothing
    /// without a hover hook.
    fn set_pointer(self, point: Option<(f32, f32)>) {
        let hovers = self
            .hooks
            .try_with_value(|hooks| hooks.as_ref().is_some_and(|hooks| hooks.hover.is_some()));
        if hovers != Some(true) || (point.is_none() && self.pointer.get_value().is_none()) {
            return;
        }
        self.pointer.try_set_value(point);
        self.request_hover();
    }

    /// Requests a hover update on the next animation frame unless one is pending.
    fn request_hover(self) {
        if self
            .hover
            .try_get_value()
            .is_none_or(|(requested, _)| requested)
        {
            return;
        }
        self.hover.update_value(|hover| hover.0 = true);
        request_animation_frame(move || self.update_hover());
    }

    /// Tells the hover hook where the pointer hovers; draws a frame when the view changed.
    fn update_hover(self) {
        let Some(point) = self.pointer.try_get_value() else {
            return;
        };
        let Some(hook) = self
            .hooks
            .try_get_value()
            .flatten()
            .and_then(|hooks| hooks.hover.clone())
        else {
            return;
        };
        let changed = self
            .viewer
            .try_update_value(|viewer| {
                let viewer = viewer.as_mut()?;
                let changed = hook(viewer, point);
                Some((changed, viewer.camera()))
            })
            .flatten();
        let camera = changed.map(|(_, camera)| camera);
        self.hover.try_set_value((false, camera));
        if changed.is_some_and(|(changed, _)| changed) {
            self.request();
        }
    }

    /// Sends the viewer's interest to the feed now and tells the [`Hooks::interest`] hook.
    fn send_interest(self) {
        let hook = self
            .hooks
            .try_get_value()
            .flatten()
            .and_then(|hooks| hooks.interest.clone());
        let Some(Some((camera, revision, interest))) = self.viewer.try_with_value(|viewer| {
            viewer.as_ref().map(|viewer| {
                if let Some(hook) = &hook {
                    hook(viewer);
                }
                (
                    viewer.camera(),
                    viewer.interest_revision(),
                    viewer.interest(),
                )
            })
        }) else {
            return;
        };
        self.interest.set_value(InterestClock {
            sent: Some((camera, self.size.get_value(), revision)),
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

    /// The canvas size in CSS pixels, as last given to the viewer.
    pub fn css_size(self) -> (f32, f32) {
        let (width, height, _) = self.size.try_get_value().unwrap_or_default();
        (width, height)
    }
}

/// Applies a feed event to `viewer`.
pub fn apply(viewer: &mut Viewer, event: &WorldEvent) {
    match event {
        WorldEvent::Terrain(room, terrain) => viewer.set_terrain(*room, terrain),
        WorldEvent::NoTerrain(room) => viewer.mark_no_terrain(*room),
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
        WorldEvent::Portals(room, portals) => viewer.set_portals(*room, portals),
        // Pauses show in the status bar (the world's progress).
        WorldEvent::Detail(_, DetailState::Pending) | WorldEvent::StatsPaused(_) => {}
    }
}

/// Whether `event` changes what a canvas showing `view` (its visible rooms) draws: data of a
/// room in view or next to it (outlines, labels and portal runs reach over the edge), portals
/// leading into it, and every change of a live room or detail state.
pub fn shows(view: RoomRect, event: &WorldEvent) -> bool {
    let near = RoomRect::new(
        RoomCoord {
            x: view.min.x - 1,
            y: view.min.y - 1,
        },
        RoomCoord {
            x: view.max.x + 1,
            y: view.max.y + 1,
        },
    );
    match event {
        WorldEvent::Terrain(room, _)
        | WorldEvent::NoTerrain(room)
        | WorldEvent::Minimap(room, _)
        | WorldEvent::MinimapGone(room) => near.contains(*room),
        WorldEvent::Stats(stats) => stats.iter().any(|(room, _)| near.contains(*room)),
        WorldEvent::Portals(room, portals) => {
            near.contains(*room)
                || portals.iter().any(|portal| {
                    portal
                        .destination
                        .as_ref()
                        .is_some_and(|destination| near.contains(destination.room))
                })
        }
        WorldEvent::Detail(..) | WorldEvent::DetailGone(_) => true,
        WorldEvent::StatsPaused(_) => false,
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
        surface.set_pointer(None);
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
        let primary = event.is_primary();
        surface.with_viewer(|viewer| {
            viewer.pointer_down(event.pointer_id(), x, y, button, primary);
        });
    };
    let pointer_move = move |event: PointerEvent| {
        let (x, y) = position(&event);
        if event.buttons() == 0 {
            // A mouse or pen resting over the map hovers it; touches never hover.
            let hovering = event.pointer_type() != "touch";
            surface.set_pointer(hovering.then_some((x, y)));
            return;
        }
        surface.set_pointer(None);
        surface.with_viewer(|viewer| {
            viewer.pointer_move(event.pointer_id(), x, y, event.time_stamp());
        });
    };
    // Every release and cancel reaches the viewer, whatever the hooks: a pointer it still
    // thought pressed would turn the next press into a pinch.
    let pointer_up = move |event: PointerEvent| {
        let (x, y) = position(&event);
        let hooks = surface.hooks.try_get_value().flatten();
        surface.with_viewer(|viewer| {
            if let Some(pick) = viewer.pointer_up(event.pointer_id(), x, y, event.time_stamp())
                && let Some(hooks) = &hooks
            {
                (hooks.pick)(viewer, pick);
            }
        });
    };
    // A cancel, or a capture lost without a release (after one, a no-op).
    let pointer_cancel = move |event: PointerEvent| {
        surface.with_viewer(|viewer| viewer.pointer_cancel(event.pointer_id()));
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
            class="absolute inset-0 block size-full touch-none bg-sunken focus-visible:outline-offset-[-3px] data-quiet-focus:focus-visible:outline-none"
            data-testid=testid
            tabindex="0"
            aria-label=label
            aria-describedby=description_id.clone()
            on:pointerdown=pointer_down
            on:pointermove=pointer_move
            on:pointerleave=move |_| surface.set_pointer(None)
            on:pointerup=pointer_up
            on:pointercancel=pointer_cancel
            on:lostpointercapture=pointer_cancel
            on:wheel=wheel
            on:contextmenu=|event| event.prevent_default()
            on:blur=move |_| {
                if let Some(element) = canvas.get_untracked() {
                    let _ = element.remove_attribute("data-quiet-focus");
                }
            }
        ></canvas>
        <p id=description_id class="sr-only">{description}</p>
    }
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
        StoredValue::new_local(TilePointHook::install(&element, surface));
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

/// Name of the canvas property holding the [`TilePointHook`] function.
const TILE_POINT: &str = "overseerTilePoint";

/// Test hook: `canvas.overseerTilePoint(room, x, y)` answers the CSS pixel of the canvas
/// (`[x, y]`, as `offsetX`/`offsetY`) where the top centre of tile `(x, y)` of `room` (a name
/// such as `W1N1`) is drawn: a click there picks the tile. `null` before the viewer exists,
/// for an unknown room name, or behind the camera. End-to-end tests click objects through
/// it; removed with the canvas.
struct TilePointHook {
    element: web_sys::HtmlCanvasElement,
    /// Kept alive while installed.
    _callback: Closure<dyn Fn(String, u8, u8) -> wasm_bindgen::JsValue>,
}

impl TilePointHook {
    /// Installs the hook on `element`.
    fn install(element: &web_sys::HtmlCanvasElement, surface: Surface) -> Self {
        let callback = Closure::<dyn Fn(String, u8, u8) -> wasm_bindgen::JsValue>::new(
            move |room: String, x: u8, y: u8| {
                room.parse()
                    .ok()
                    .and_then(|room| {
                        surface
                            .with_viewer_quiet(|viewer| viewer.tile_point(room, x, y))
                            .flatten()
                    })
                    .map_or(wasm_bindgen::JsValue::NULL, |(x, y)| {
                        js_sys::Array::of2(&x.into(), &y.into()).into()
                    })
            },
        );
        let _ = js_sys::Reflect::set(element, &TILE_POINT.into(), callback.as_ref());
        Self {
            element: element.clone(),
            _callback: callback,
        }
    }
}

impl Drop for TilePointHook {
    fn drop(&mut self) {
        let _ = js_sys::Reflect::delete_property(&self.element, &TILE_POINT.into());
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use overseer::map::{
        DetailState, Portal, PortalDestination, RoomCoord, RoomRect, RoomStat, RoomStatus,
        WorldEvent,
    };

    use super::shows;

    fn room(name: &str) -> RoomCoord {
        name.parse().unwrap()
    }

    fn stat(rooms: &[&str]) -> WorldEvent {
        let stat = Arc::new(RoomStat {
            status: RoomStatus::Normal,
            status_until: None,
            owner: None,
            reservation: None,
            safe_mode: false,
            mineral: None,
            mineral_density: None,
            fetched_at: 0,
        });
        WorldEvent::Stats(
            rooms
                .iter()
                .map(|name| (room(name), Arc::clone(&stat)))
                .collect(),
        )
    }

    #[test]
    fn background_rooms_draw_nothing_until_they_come_into_view() {
        // W2N2..W0N0 in view: W3N3 touches it, W5N5 and E5S5 do not.
        let view = RoomRect::new(room("W2N2"), room("W0N0"));
        assert!(shows(view, &WorldEvent::NoTerrain(room("W1N1"))));
        assert!(shows(view, &WorldEvent::NoTerrain(room("W3N3"))));
        assert!(shows(view, &WorldEvent::NoTerrain(room("E0S0"))));
        assert!(!shows(view, &WorldEvent::NoTerrain(room("W5N5"))));
        assert!(!shows(view, &WorldEvent::MinimapGone(room("E5S5"))));
        // One room of a batch in view is enough.
        assert!(!shows(view, &stat(&["W9N9", "E5S5"])));
        assert!(shows(view, &stat(&["W9N9", "W2N1"])));
        assert!(!shows(view, &WorldEvent::StatsPaused(Some(1))));
        // Live rooms and detail states always draw.
        assert!(shows(view, &WorldEvent::DetailGone(room("E5S5"))));
        assert!(shows(
            view,
            &WorldEvent::Detail(room("E5S5"), DetailState::Pending)
        ));
    }

    #[test]
    fn portals_draw_when_they_stand_or_lead_into_view() {
        let view = RoomRect::new(room("W2N2"), room("W0N0"));
        let portals = |at: &str, to: Option<&str>| {
            let mut portal = Portal::pending(room(at), 25, 25);
            portal.destination = to.map(|to| PortalDestination {
                room: room(to),
                shard: None,
                x: Some(25),
                y: Some(25),
            });
            WorldEvent::Portals(room(at), Arc::from(vec![portal]))
        };
        assert!(shows(view, &portals("W1N1", None)));
        assert!(!shows(view, &portals("W10N10", None)));
        assert!(shows(view, &portals("W10N10", Some("W1N1"))));
        assert!(!shows(view, &portals("W10N10", Some("W20N20"))));
        // No portals left in a room out of view.
        assert!(!shows(
            view,
            &WorldEvent::Portals(room("W10N10"), Arc::from(Vec::new()))
        ));
    }
}
