//! The drawer's sizes ([`DrawerSize`]) and its resize handles. Each handle drags with any
//! pointer (mouse, pen, finger; pointer capture), moves with the keys and saves the new size
//! when the drag ends:
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

/// Narrowest drawer, in CSS pixels.
const MIN_WIDTH: u16 = 280;
/// Widest drawer, in CSS pixels (the map keeps [`MAP_MIN_WIDTH`] besides).
const MAX_WIDTH: u16 = 960;
/// Narrowest map next to the drawer, in CSS pixels.
const MAP_MIN_WIDTH: f64 = 320.0;
/// Arrow-key resize step, in CSS pixels.
const KEY_STEP: u16 = 16;

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

/// The drawer's sizes; `Copy`.
#[derive(Clone, Copy)]
pub struct DrawerSize {
    /// The width at the right of the map, in CSS pixels.
    pub width: RwSignal<u16>,
    /// The widest drawer the window allows, in CSS pixels.
    pub limit: RwSignal<u16>,
    /// The height as a bottom sheet, in percent of the world screen.
    pub height: RwSignal<f64>,
}

impl DrawerSize {
    /// The saved sizes, within what the window allows. While the calling component lives, a
    /// smaller window narrows the drawer again and a larger one gives the saved width back.
    pub fn new(state: AppState) -> Self {
        let saved_width = move || {
            state
                .saved
                .with_untracked(|saved| saved.settings.drawer_width)
        };
        let width = RwSignal::new(clamp_width(f64::from(saved_width())));
        let limit = RwSignal::new(max_width());
        let resize = window_event_listener(leptos::ev::resize, move |_| {
            limit.set(max_width());
            width.set(clamp_width(f64::from(saved_width())));
        });
        on_cleanup(move || resize.remove());
        let height = state
            .saved
            .with_untracked(|saved| saved.settings.drawer_height);
        Self {
            width,
            limit,
            height: RwSignal::new(f64::from(snap(f64::from(height)))),
        }
    }
}

/// The widest drawer the window allows.
fn max_width() -> u16 {
    let window = web_sys::window()
        .and_then(|window| window.inner_width().ok())
        .and_then(|width| width.as_f64())
        .unwrap_or(f64::from(MAX_WIDTH) + MAP_MIN_WIDTH);
    let room = (window - MAP_MIN_WIDTH).clamp(f64::from(MIN_WIDTH), f64::from(MAX_WIDTH));
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to the u16 range of MIN_WIDTH..=MAX_WIDTH"
    )]
    let room = room as u16;
    room
}

/// `width` within what the window allows.
fn clamp_width(width: f64) -> u16 {
    let max = max_width();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to the u16 range of MIN_WIDTH..=max"
    )]
    let width = width.round().clamp(f64::from(MIN_WIDTH), f64::from(max)) as u16;
    width
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

/// The element that got `event` (the handle itself, not a child).
fn handle_of(event: &PointerEvent) -> Option<web_sys::Element> {
    event
        .current_target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
}

/// The drawer's left edge: drag it (or use the arrow keys on it) to resize the drawer;
/// double-click restores the default width. The width is saved when the drag ends.
#[component]
pub fn Resizer(
    /// The drawer's width.
    width: RwSignal<u16>,
    /// The widest drawer the window allows.
    limit: Signal<u16>,
) -> impl IntoView {
    let state = AppState::expect();
    // The pointer's x and the width when the drag began.
    let drag = StoredValue::new(None::<(f64, u16)>);
    let save = move || {
        let width = width.get_untracked();
        state.update_saved(|saved| saved.settings.drawer_width = width);
    };
    let down = move |event: PointerEvent| {
        if event.button() != 0 {
            return;
        }
        event.prevent_default();
        if let Some(handle) = handle_of(&event) {
            let _ = handle.set_pointer_capture(event.pointer_id());
        }
        drag.set_value(Some((f64::from(event.client_x()), width.get_untracked())));
    };
    let moved = move |event: PointerEvent| {
        if let Some((x, start)) = drag.get_value() {
            width.set(clamp_width(
                f64::from(start) + x - f64::from(event.client_x()),
            ));
        }
    };
    let up = move |_: PointerEvent| {
        if drag.get_value().is_some() {
            drag.set_value(None);
            save();
        }
    };
    let keydown = move |event: KeyboardEvent| {
        let current = width.get_untracked();
        let next = match event.key().as_str() {
            "ArrowLeft" => current.saturating_add(KEY_STEP),
            "ArrowRight" => current.saturating_sub(KEY_STEP),
            "Home" => MIN_WIDTH,
            "End" => MAX_WIDTH,
            _ => return,
        };
        event.prevent_default();
        width.set(clamp_width(f64::from(next)));
        save();
    };
    view! {
        <div
            class=RESIZER
            role="separator"
            tabindex="0"
            aria-orientation="vertical"
            aria-label="Resize drawer"
            aria-controls="drawer"
            aria-valuemin=MIN_WIDTH
            aria-valuemax=move || limit.get()
            aria-valuenow=move || width.get()
            data-tip="Drag to resize · double-click to reset"
            on:pointerdown=down
            on:pointermove=moved
            on:pointerup=up
            on:pointercancel=up
            on:keydown=keydown
            on:dblclick=move |_| {
                width.set(clamp_width(f64::from(DRAWER_WIDTH)));
                save();
            }
        >
            <span class=GRIP aria-hidden="true">
                <Icon kind=IconKind::Grip />
            </span>
        </div>
    }
}

/// The bottom sheet's grab handle: drag it up or down to resize the sheet (it ends on the
/// nearest snap height), ArrowUp and ArrowDown go to the next snap height, Home and End to
/// the lowest and highest; double-click restores the default height. The height is saved
/// when the drag ends.
#[component]
pub fn SheetHandle(
    /// The sheet's height in percent of the world screen (between snap heights during a
    /// drag).
    height: RwSignal<f64>,
) -> impl IntoView {
    let state = AppState::expect();
    // The pointer's y, the height when the drag began and the world screen's height, in CSS
    // pixels.
    let drag = StoredValue::new(None::<(f64, f64, f64)>);
    let settle = move |next: u8| {
        height.set(f64::from(next));
        state.update_saved(|saved| saved.settings.drawer_height = next);
    };
    let down = move |event: PointerEvent| {
        if event.button() != 0 {
            return;
        }
        let Some(handle) = handle_of(&event) else {
            return;
        };
        let Some(screen) = handle
            .closest("#drawer")
            .ok()
            .flatten()
            .and_then(|drawer| drawer.parent_element())
        else {
            return;
        };
        let total = screen.get_bounding_client_rect().height();
        if total <= 0.0 {
            return;
        }
        event.prevent_default();
        let _ = handle.set_pointer_capture(event.pointer_id());
        drag.set_value(Some((
            f64::from(event.client_y()),
            height.get_untracked(),
            total,
        )));
    };
    let moved = move |event: PointerEvent| {
        if let Some((y, start, total)) = drag.get_value() {
            let moved = (y - f64::from(event.client_y())) / total * 100.0;
            height.set((start + moved).clamp(DRAG_MIN, DRAG_MAX));
        }
    };
    let up = move |_: PointerEvent| {
        if drag.get_value().is_some() {
            drag.set_value(None);
            settle(snap(height.get_untracked()));
        }
    };
    let keydown = move |event: KeyboardEvent| {
        let current = snap(height.get_untracked());
        let next = match event.key().as_str() {
            "ArrowUp" => step(current, true),
            "ArrowDown" => step(current, false),
            "Home" => SNAPS[0],
            "End" => SNAPS[SNAPS.len() - 1],
            _ => return,
        };
        event.prevent_default();
        settle(next);
    };
    view! {
        <div
            class=SHEET_HANDLE
            role="separator"
            tabindex="0"
            aria-orientation="horizontal"
            aria-label="Resize drawer"
            aria-controls="drawer"
            aria-valuemin=SNAPS[0]
            aria-valuemax=SNAPS[SNAPS.len() - 1]
            aria-valuenow=move || snap(height.get())
            on:pointerdown=down
            on:pointermove=moved
            on:pointerup=up
            on:pointercancel=up
            on:keydown=keydown
            on:dblclick=move |_| settle(DRAWER_HEIGHT)
        >
            <span class=PILL aria-hidden="true"></span>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::{SNAPS, snap, step};

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
    }
}
