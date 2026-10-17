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

/// The `sm` breakpoint while the stylesheet's `--breakpoint-sm` is empty (the stylesheet did
/// not load): its value in `app/tailwind.css`. The stylesheet's value wins.
const SM_FALLBACK: &str = "641px";
/// A finger, not a mouse, is the main pointer (Tailwind's `pointer-coarse:` variant).
pub const COARSE_POINTER: &str = "(pointer: coarse)";

/// The bottom-sheet layout of the world screen: narrower than the `sm` breakpoint of the
/// stylesheet, as Tailwind's `max-sm:` variant asks. The drawer is then a sheet under the map.
pub fn sheet_layout() -> String {
    below_breakpoint("sm", SM_FALLBACK)
}

/// The query of Tailwind's `max-{name}:` variant: narrower than `--breakpoint-{name}` of the
/// stylesheet (on the root element), else than `fallback` while the variable is empty.
fn below_breakpoint(name: &str, fallback: &str) -> String {
    let value = web_sys::window()
        .and_then(|window| {
            let root = window.document()?.document_element()?;
            window.get_computed_style(&root).ok().flatten()
        })
        .and_then(|style| {
            style
                .get_property_value(&format!("--breakpoint-{name}"))
                .ok()
        })
        .unwrap_or_default();
    narrower_than(&value, fallback)
}

/// The media query "narrower than `width`" (a CSS length), `fallback` for an empty `width`.
fn narrower_than(width: &str, fallback: &str) -> String {
    let width = width.trim();
    let width = if width.is_empty() { fallback } else { width };
    format!("(width < {width})")
}

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

#[cfg(test)]
mod tests {
    use super::narrower_than;

    #[test]
    fn a_breakpoint_gives_a_narrower_than_query_and_an_empty_one_the_fallback() {
        assert_eq!(narrower_than(" 641px", "1px"), "(width < 641px)");
        assert_eq!(narrower_than("40rem", "1px"), "(width < 40rem)");
        assert_eq!(narrower_than("", "641px"), "(width < 641px)");
        assert_eq!(narrower_than("  ", "641px"), "(width < 641px)");
    }
}
