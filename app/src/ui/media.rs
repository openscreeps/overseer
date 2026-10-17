//! Media queries as signals: a component asks how the window is laid out and follows the
//! changes (a rotated phone, a resized window).
//!
//! Use CSS (`max-sm:`, `pointer-coarse:`) for what only looks different. Use a signal from here
//! only where the layout changes what is built (for example a second canvas that a phone does
//! not get).

use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::MediaQueryList;

/// The bottom-sheet layout of the world screen: narrower than the `sm` breakpoint of
/// `app/tailwind.css`, as Tailwind's `max-sm:` variant asks. The drawer is then a sheet under
/// the map.
pub const SHEET_LAYOUT: &str = "(width < 641px)";
/// A finger, not a mouse, is the main pointer (Tailwind's `pointer-coarse:` variant).
pub const COARSE_POINTER: &str = "(pointer: coarse)";

/// Whether `query` matches now, updated on each change while the calling component lives.
/// `false` where the window cannot answer the query.
pub fn media_query(query: &str) -> Signal<bool> {
    let Some(list) = web_sys::window().and_then(|window| window.match_media(query).ok().flatten())
    else {
        return Signal::stored(false);
    };
    let matches = RwSignal::new(list.matches());
    let watched = list.clone();
    let listener = Closure::<dyn Fn()>::new(move || {
        matches.try_set(watched.matches());
    });
    let _ = list.add_event_listener_with_callback("change", listener.as_ref().unchecked_ref());
    // Stops the listener when the component goes (the stored guard is dropped with it).
    StoredValue::new_local(MediaGuard { list, listener });
    matches.read_only().into()
}

/// A `change` listener on a media query list; removed when dropped.
struct MediaGuard {
    list: MediaQueryList,
    listener: Closure<dyn Fn()>,
}

impl Drop for MediaGuard {
    fn drop(&mut self) {
        let _ = self
            .list
            .remove_event_listener_with_callback("change", self.listener.as_ref().unchecked_ref());
    }
}
