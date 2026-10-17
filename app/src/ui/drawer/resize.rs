//! The drawer's sizes ([`DrawerSize`]) and its resize handles. The saved settings own the
//! sizes: a handle shows its drag in a [`DrawerSize`] and saves the size when the drag ends.
//! Both handles are a [`Separator`]: it drags with any pointer (mouse, pen, finger; pointer
//! capture), moves with the keys and restores the default size on double-click.
//!
//! - [`Resizer`]: the left edge of the side drawer, its width in CSS pixels;
//! - [`SheetHandle`]: the grab handle on top of the bottom sheet (phone layout), its height in
//!   percent of the world screen. A drag ends on the nearest snap height ([`SNAPS`]).

use leptos::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{KeyboardEvent, PointerEvent};

use crate::saved::{DRAWER_HEIGHT, DRAWER_WIDTH};
use crate::state::AppState;
use crate::ui::icon::{Icon, IconKind};
use crate::ui::style::{self, concatcp};

/// Narrowest drawer, in CSS pixels (also the `min-w-[280px]` of the drawer's class).
const MIN_WIDTH: u16 = 280;
/// Widest drawer, in CSS pixels (the map keeps [`MAP_MIN_WIDTH`] besides).
const MAX_WIDTH: u16 = 960;
/// Narrowest map next to the drawer, in CSS pixels (also the `max-w-[calc(100%-320px)]` of the
/// drawer's class).
const MAP_MIN_WIDTH: f64 = 320.0;
/// Arrow-key resize step, in CSS pixels.
const KEY_STEP: f64 = 16.0;

/// The snap heights of the bottom sheet, in percent of the world screen, lowest first.
const SNAPS: [u8; 3] = [30, 50, 85];
/// The lowest sheet height during a drag, in percent.
const DRAG_MIN: f64 = 20.0;
/// The highest sheet height during a drag, in percent.
const DRAG_MAX: f64 = 90.0;

/// The resize edge: a line and a grip, always shown (quiet), in the accent while hovered,
/// dragged or focused. Wider for a finger.
const RESIZER: &str = concatcp!(
    "group absolute inset-y-0 -left-[4px] z-5 w-[8px] pointer-coarse:-left-[10px] \
    pointer-coarse:w-[20px] cursor-col-resize touch-none focus-visible:outline-none \
    before:absolute before:inset-y-0 before:left-[calc(50%-1px)] before:w-[2px] \
    before:bg-transparent before:transition-colors before:duration-(--duration-fast) \
    before:ease-standard hover:before:bg-accent active:before:bg-accent \
    focus-visible:before:bg-accent ",
    style::TIP_LEFT
);
/// The grip in the middle of the [`RESIZER`].
const GRIP: &str = "absolute top-1/2 left-1/2 flex items-center justify-center w-[16px] h-8 \
    text-faint bg-panel-2 border border-edge rounded-sm -translate-1/2 pointer-events-none \
    transition-colors duration-(--duration-fast) ease-standard group-hover:text-fg \
    group-hover:border-accent group-active:text-fg group-active:border-accent \
    group-focus-visible:text-fg group-focus-visible:border-accent";
/// The sheet's grab handle: a strip over the sheet's top edge (its hit area reaches a little
/// over the map) with a pill in the middle.
const SHEET_HANDLE: &str = "group relative flex flex-none items-center justify-center h-4 \
    pointer-coarse:h-6 cursor-row-resize touch-none focus-visible:outline-none \
    focus-visible:bg-hover before:absolute before:inset-x-0 before:-top-2 before:h-2";
/// The pill of the [`SHEET_HANDLE`].
const PILL: &str = "w-10 h-1 rounded-full bg-edge-strong pointer-events-none transition-colors \
    duration-(--duration-fast) ease-standard group-hover:bg-accent group-active:bg-accent \
    group-focus-visible:bg-accent";

/// The drawer's sizes as shown; `Copy`. The saved settings own the sizes; a drag shows its
/// size here until it ends and saves it.
#[derive(Clone, Copy)]
pub struct DrawerSize {
    state: AppState,
    /// The widest drawer the window allows, in CSS pixels.
    limit: RwSignal<u16>,
    /// The width during a drag, in CSS pixels (not clamped).
    dragged_width: RwSignal<Option<f64>>,
    /// The height during a drag, in percent (not clamped).
    dragged_height: RwSignal<Option<f64>>,
    /// The width at the right of the map, in CSS pixels: the drag's or the saved one, within
    /// what the window allows. A smaller window narrows the drawer, a larger one gives the
    /// saved width back.
    pub width: Memo<u16>,
    /// The height as a bottom sheet, in percent of the world screen: the drag's, else the
    /// saved one on its nearest snap height.
    pub height: Memo<f64>,
}

impl DrawerSize {
    /// The sizes, following the saved settings and the window while the calling component
    /// lives.
    pub fn new(state: AppState) -> Self {
        let limit = RwSignal::new(max_width());
        let resize = window_event_listener(leptos::ev::resize, move |_| limit.set(max_width()));
        on_cleanup(move || resize.remove());
        let dragged_width = RwSignal::new(None::<f64>);
        let dragged_height = RwSignal::new(None::<f64>);
        let width = Memo::new(move |_| {
            let width = dragged_width.get().unwrap_or_else(|| {
                f64::from(state.saved.with(|saved| saved.settings.drawer_width))
            });
            clamp_width(width, limit.get())
        });
        let height = Memo::new(move |_| match dragged_height.get() {
            Some(height) => height.clamp(DRAG_MIN, DRAG_MAX),
            None => f64::from(snap(f64::from(
                state.saved.with(|saved| saved.settings.drawer_height),
            ))),
        });
        Self {
            state,
            limit,
            dragged_width,
            dragged_height,
            width,
            height,
        }
    }

    /// Saves `width` (CSS pixels), within what the window allows.
    fn save_width(self, width: f64) {
        let width = clamp_width(width, self.limit.get_untracked());
        self.state
            .update_saved(|saved| saved.settings.drawer_width = width);
    }

    /// Saves the snap height nearest to `height` (percent).
    fn save_height(self, height: f64) {
        let height = snap(height);
        self.state
            .update_saved(|saved| saved.settings.drawer_height = height);
    }
}

/// The widest drawer the window allows.
fn max_width() -> u16 {
    let window = web_sys::window()
        .and_then(|window| window.inner_width().ok())
        .and_then(|width| width.as_f64())
        .unwrap_or(f64::from(MAX_WIDTH) + MAP_MIN_WIDTH);
    limit_for(window)
}

/// The widest drawer in a window `window` CSS pixels wide.
fn limit_for(window: f64) -> u16 {
    let room = (window - MAP_MIN_WIDTH).clamp(f64::from(MIN_WIDTH), f64::from(MAX_WIDTH));
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to the u16 range of MIN_WIDTH..=MAX_WIDTH"
    )]
    let room = room as u16;
    room
}

/// `width` between [`MIN_WIDTH`] and `max`, in whole CSS pixels.
fn clamp_width(width: f64, max: u16) -> u16 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to the u16 range of MIN_WIDTH..=max"
    )]
    let width = width
        .round()
        .clamp(f64::from(MIN_WIDTH), f64::from(max.max(MIN_WIDTH))) as u16;
    width
}

/// The width that `key` gives a drawer `width` wide (clamped when saved); `None` for another
/// key. ← widens, → narrows, Home and End go to the limits.
fn width_key(key: &str, width: f64) -> Option<f64> {
    match key {
        "ArrowLeft" => Some(width + KEY_STEP),
        "ArrowRight" => Some(width - KEY_STEP),
        "Home" => Some(f64::from(MIN_WIDTH)),
        "End" => Some(f64::from(MAX_WIDTH)),
        _ => None,
    }
}

/// The snap height nearest to `height` (percent).
fn snap(height: f64) -> u8 {
    SNAPS
        .into_iter()
        .min_by(|a, b| {
            (f64::from(*a) - height)
                .abs()
                .total_cmp(&(f64::from(*b) - height).abs())
        })
        .unwrap_or(DRAWER_HEIGHT)
}

/// The next snap height above `height` (`up`) or below it; the highest or lowest at the end.
fn step(height: u8, up: bool) -> u8 {
    if up {
        SNAPS
            .into_iter()
            .find(|snap| *snap > height)
            .unwrap_or(SNAPS[SNAPS.len() - 1])
    } else {
        SNAPS
            .into_iter()
            .rev()
            .find(|snap| *snap < height)
            .unwrap_or(SNAPS[0])
    }
}

/// The height that `key` gives a sheet `height` high (percent); `None` for another key.
/// ↑ and ↓ go to the next snap height, Home and End to the lowest and highest.
fn height_key(key: &str, height: f64) -> Option<f64> {
    let current = snap(height);
    let next = match key {
        "ArrowUp" => step(current, true),
        "ArrowDown" => step(current, false),
        "Home" => SNAPS[0],
        "End" => SNAPS[SNAPS.len() - 1],
        _ => return None,
    };
    Some(f64::from(next))
}

/// The axis of a [`Separator`]: `Vertical` is an edge between panes side by side (it drags
/// along x), `Horizontal` one between panes one over the other (it drags along y).
#[derive(Clone, Copy)]
enum Orientation {
    Vertical,
    Horizontal,
}

impl Orientation {
    /// The `aria-orientation` value.
    fn aria(self) -> &'static str {
        match self {
            Self::Vertical => "vertical",
            Self::Horizontal => "horizontal",
        }
    }

    /// The pointer's position along the drag axis, in CSS pixels.
    fn position(self, event: &PointerEvent) -> f64 {
        match self {
            Self::Vertical => f64::from(event.client_x()),
            Self::Horizontal => f64::from(event.client_y()),
        }
    }
}

/// A drag in progress.
#[derive(Clone, Copy)]
struct Drag {
    /// The pointer's position along the axis when the drag began, in CSS pixels.
    origin: f64,
    /// The size when the drag began.
    start: f64,
    /// The size change per CSS pixel.
    scale: f64,
}

impl Drag {
    /// The size with the pointer at `position`: it grows toward the start of the axis (the
    /// drawer is at the right of the map or under it).
    fn size(self, position: f64) -> f64 {
        self.start + (self.origin - position) * self.scale
    }
}

/// A resize handle of the drawer (`role="separator"`, named "Resize drawer"). A drag shows
/// its size in `dragged` and saves it when it ends; a key and a double-click save at once.
#[component]
fn Separator(
    /// The handle's classes.
    class: &'static str,
    orientation: Orientation,
    /// The tooltip, if any.
    #[prop(optional)]
    tip: Option<&'static str>,
    /// The size shown.
    #[prop(into)]
    size: Signal<f64>,
    /// The size for assistive technology (`aria-valuenow`).
    #[prop(into)]
    now: Signal<u16>,
    /// The smallest `now`.
    min: u16,
    /// The largest `now`.
    #[prop(into)]
    max: Signal<u16>,
    /// The size change per CSS pixel of a drag on the handle; `None` refuses the drag.
    scale: fn(&web_sys::Element) -> Option<f64>,
    /// The size during a drag (not clamped).
    dragged: RwSignal<Option<f64>>,
    /// The size a key gives from the size shown; `None` for a key it does not use.
    key: fn(&str, f64) -> Option<f64>,
    /// Saves a size.
    save: Callback<f64>,
    /// The default size, restored on double-click.
    reset: f64,
    children: Children,
) -> impl IntoView {
    let drag = StoredValue::new(None::<Drag>);
    let down = move |event: PointerEvent| {
        if event.button() != 0 {
            return;
        }
        let Some(handle) = event
            .current_target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        else {
            return;
        };
        let Some(scale) = scale(&handle) else {
            return;
        };
        event.prevent_default();
        let _ = handle.set_pointer_capture(event.pointer_id());
        drag.set_value(Some(Drag {
            origin: orientation.position(&event),
            start: size.get_untracked(),
            scale,
        }));
    };
    let moved = move |event: PointerEvent| {
        if let Some(drag) = drag.get_value() {
            dragged.set(Some(drag.size(orientation.position(&event))));
        }
    };
    let up = move |_: PointerEvent| {
        if drag.get_value().is_some() {
            drag.set_value(None);
            save.run(size.get_untracked());
            dragged.set(None);
        }
    };
    let keydown = move |event: KeyboardEvent| {
        if let Some(next) = key(&event.key(), size.get_untracked()) {
            event.prevent_default();
            save.run(next);
        }
    };
    view! {
        <div
            class=class
            role="separator"
            tabindex="0"
            aria-orientation=orientation.aria()
            aria-label="Resize drawer"
            aria-controls="drawer"
            aria-valuemin=min
            aria-valuemax=move || max.get()
            aria-valuenow=move || now.get()
            data-tip=tip
            on:pointerdown=down
            on:pointermove=moved
            on:pointerup=up
            on:pointercancel=up
            on:keydown=keydown
            on:dblclick=move |_| save.run(reset)
        >
            {children()}
        </div>
    }
}

/// The drawer's left edge: drag it (or use the arrow keys on it) to resize the drawer;
/// double-click restores the default width.
#[component]
pub fn Resizer(size: DrawerSize) -> impl IntoView {
    let width = size.width;
    view! {
        <Separator
            class=RESIZER
            orientation=Orientation::Vertical
            tip="Drag to resize · double-click to reset"
            size=Signal::derive(move || f64::from(width.get()))
            now=width
            min=MIN_WIDTH
            max=size.limit
            scale=|_| Some(1.0)
            dragged=size.dragged_width
            key=width_key
            save=Callback::new(move |width| size.save_width(width))
            reset=f64::from(DRAWER_WIDTH)
        >
            <span class=GRIP aria-hidden="true">
                <Icon kind=IconKind::Grip />
            </span>
        </Separator>
    }
}

/// The bottom sheet's grab handle: drag it up or down to resize the sheet (it ends on the
/// nearest snap height), ArrowUp and ArrowDown go to the next snap height, Home and End to
/// the lowest and highest; double-click restores the default height.
#[component]
pub fn SheetHandle(size: DrawerSize) -> impl IntoView {
    let height = size.height;
    view! {
        <Separator
            class=SHEET_HANDLE
            orientation=Orientation::Horizontal
            size=height
            now=Signal::derive(move || u16::from(snap(height.get())))
            min=u16::from(SNAPS[0])
            max=Signal::stored(u16::from(SNAPS[SNAPS.len() - 1]))
            scale=sheet_scale
            dragged=size.dragged_height
            key=height_key
            save=Callback::new(move |height| size.save_height(height))
            reset=f64::from(DRAWER_HEIGHT)
        >
            <span class=PILL aria-hidden="true"></span>
        </Separator>
    }
}

/// Percent of the world screen per CSS pixel, for the sheet's `handle`; `None` while the
/// screen has no height.
fn sheet_scale(handle: &web_sys::Element) -> Option<f64> {
    let screen = handle.closest("#drawer").ok()??.parent_element()?;
    let total = screen.get_bounding_client_rect().height();
    (total > 0.0).then(|| 100.0 / total)
}

#[cfg(test)]
mod tests {
    use super::{
        Drag, MAX_WIDTH, MIN_WIDTH, SNAPS, clamp_width, height_key, limit_for, snap, step,
        width_key,
    };

    #[test]
    fn a_drag_ends_on_the_nearest_snap_height() {
        assert_eq!(snap(0.0), 30);
        assert_eq!(snap(39.0), 30);
        assert_eq!(snap(41.0), 50);
        assert_eq!(snap(67.0), 50);
        assert_eq!(snap(68.0), 85);
        assert_eq!(snap(100.0), 85);
        // A saved height of any value shows as a snap height.
        for height in SNAPS {
            assert_eq!(snap(f64::from(height)), height);
        }
    }

    #[test]
    fn the_keys_step_through_the_snap_heights_and_stop_at_the_ends() {
        assert_eq!(step(30, true), 50);
        assert_eq!(step(50, true), 85);
        assert_eq!(step(85, true), 85);
        assert_eq!(step(85, false), 50);
        assert_eq!(step(50, false), 30);
        assert_eq!(step(30, false), 30);
        // From a height between snap heights (during a drag), the keys start at the nearest.
        assert_eq!(height_key("ArrowUp", 47.0), Some(85.0));
        assert_eq!(height_key("ArrowDown", 47.0), Some(30.0));
        assert_eq!(height_key("Home", 85.0), Some(30.0));
        assert_eq!(height_key("End", 30.0), Some(85.0));
        assert_eq!(height_key("ArrowLeft", 50.0), None);
    }

    #[test]
    fn the_width_stays_between_its_limits_and_the_window() {
        assert_eq!(limit_for(1920.0), MAX_WIDTH);
        assert_eq!(limit_for(1000.0), 680);
        assert_eq!(limit_for(400.0), MIN_WIDTH);
        assert_eq!(clamp_width(400.4, 680), 400);
        assert_eq!(clamp_width(100.0, 680), MIN_WIDTH);
        assert_eq!(clamp_width(2000.0, 680), 680);
        // A limit under the narrowest drawer keeps the narrowest drawer.
        assert_eq!(clamp_width(500.0, 100), MIN_WIDTH);
    }

    #[test]
    fn the_keys_resize_the_width_by_steps_and_to_the_limits() {
        assert_eq!(width_key("ArrowLeft", 400.0), Some(416.0));
        assert_eq!(width_key("ArrowRight", 400.0), Some(384.0));
        assert_eq!(width_key("Home", 400.0), Some(f64::from(MIN_WIDTH)));
        assert_eq!(width_key("End", 400.0), Some(f64::from(MAX_WIDTH)));
        assert_eq!(width_key("ArrowUp", 400.0), None);
    }

    #[test]
    fn a_drag_grows_the_size_toward_the_start_of_its_axis() {
        let drag = Drag {
            origin: 500.0,
            start: 400.0,
            scale: 1.0,
        };
        assert!((drag.size(380.0) - 520.0).abs() < f64::EPSILON);
        assert!((drag.size(600.0) - 300.0).abs() < f64::EPSILON);
        let sheet = Drag {
            origin: 300.0,
            start: 50.0,
            scale: 0.1,
        };
        assert!((sheet.size(200.0) - 60.0).abs() < 1e-9);
    }
}
