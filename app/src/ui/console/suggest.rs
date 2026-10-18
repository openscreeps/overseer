//! The suggestion list of the console input ([`complete`](super::complete)): shown above the
//! input while the user types, the selected entry highlighted.
//!
//! Tab accepts the selected suggestion (with the list closed, Tab accepts the only suggestion
//! or opens the list). While the list shows, ↑/↓ move the selection and Escape closes it;
//! while it is closed, ↑/↓ stay the history's. Enter never accepts: it runs the command. A tap
//! on a suggestion accepts it. During IME composition the list stays closed.

use std::time::Duration;

use leptos::html::Textarea;
use leptos::prelude::*;
use web_sys::KeyboardEvent;

use super::complete::{self, Completion};
use super::history::CommandHistory;
use super::names::CompletionNames;

/// The suggestion list: above the input, as wide as the input at most.
const LIST: &str = "absolute bottom-full left-3 z-30 w-[min(28rem,calc(100%-1.5rem))] \
    max-h-[min(22rem,45vh)] mb-1 overflow-y-auto overscroll-contain p-1 list-none bg-panel-2 \
    border border-edge rounded-lg shadow-lg";
/// A suggestion: a row (finger size on a coarse pointer), highlighted when selected.
const OPTION: &str = "flex items-center gap-3 min-h-row px-2 font-mono text-sm leading-tight \
    text-fg rounded-md cursor-pointer select-none hover:bg-hover aria-selected:bg-accent-soft \
    aria-selected:text-accent";
/// The name of a suggestion; a long one is cut.
const OPTION_LABEL: &str = "flex-1 min-w-0 truncate";
/// What a suggestion is, after its name.
const OPTION_DETAIL: &str = "flex-none max-w-[45%] truncate font-sans text-xs text-faint";

/// The completion state of a console input; `Copy`.
#[derive(Clone, Copy)]
pub struct Completer {
    /// The input's text.
    text: RwSignal<String>,
    /// The input.
    input: NodeRef<Textarea>,
    /// The input's command history.
    history: StoredValue<CommandHistory>,
    /// The list may show: the user typed since the last accept, run, Escape or caret move.
    open: RwSignal<bool>,
    /// An IME composition runs.
    composing: StoredValue<bool>,
    /// The caret when the user last typed (UTF-16).
    caret: RwSignal<usize>,
    /// The highlighted suggestion.
    selected: RwSignal<usize>,
    /// The suggestions for the text and the caret (empty while closed).
    completion: Memo<Completion>,
    /// Pixels between the top of the console and the input: the list is never taller.
    space: RwSignal<Option<f64>>,
    /// The id of the list element (its options add `-<index>`).
    id: StoredValue<String>,
}

impl Completer {
    /// The completion of the input `input` holding `text`, with the names of `names` and the
    /// commands of `history`.
    pub fn new(
        text: RwSignal<String>,
        input: NodeRef<Textarea>,
        history: StoredValue<CommandHistory>,
        names: CompletionNames,
    ) -> Self {
        let open = RwSignal::new(false);
        let caret = RwSignal::new(0);
        let completion = Memo::new(move |_| {
            if !open.get() {
                return Completion::default();
            }
            let caret = caret.get();
            text.with(|text| {
                names.with(|names, memory| {
                    history.with_value(|history| {
                        complete::complete(text, caret, names, memory, history.entries())
                    })
                })
            })
        });
        // The names a completion asks for load in the background; the list follows.
        Effect::new(move |_| {
            names.ensure(completion.with(|completion| completion.needs));
        });
        Self {
            text,
            input,
            history,
            open,
            composing: StoredValue::new(false),
            caret,
            selected: RwSignal::new(0),
            completion,
            space: RwSignal::new(None),
            id: StoredValue::new(format!("console-suggestions-{}", crate::ui::next_id())),
        }
    }

    /// The id of the list element.
    pub fn list_id(self) -> String {
        self.id.get_value()
    }

    /// The id of the selected option while the list shows (`aria-activedescendant`).
    pub fn active_id(self) -> Option<String> {
        let count = self
            .completion
            .with(|completion| completion.suggestions.len());
        (count > 0).then(|| {
            format!(
                "{}-{}",
                self.id.get_value(),
                self.selected.get().min(count - 1)
            )
        })
    }

    /// The caret of the input (UTF-16), if it has no selection.
    fn input_caret(self) -> Option<usize> {
        let input = self.input.get_untracked()?;
        let start = input.selection_start().ok().flatten()?;
        let end = input.selection_end().ok().flatten()?;
        (start == end)
            .then(|| usize::try_from(start).ok())
            .flatten()
    }

    /// Pixels between the top of the console (the `section` around the input) and the form
    /// of the input, less a small gap.
    fn space_above(self) -> Option<f64> {
        let input = self.input.get_untracked()?;
        let form = input.closest("form").ok()??;
        let console = input.closest("section").ok()??;
        let space =
            form.get_bounding_client_rect().top() - console.get_bounding_client_rect().top();
        Some((space - 8.0).max(0.0))
    }

    /// Opens the list for the caret where it is now (the user typed).
    pub fn typed(self) {
        if self.composing.get_value() {
            return;
        }
        match self.input_caret() {
            Some(caret) => {
                self.caret.set(caret);
                self.selected.set(0);
                self.space.set(self.space_above());
                self.open.set(true);
            }
            None => self.close(),
        }
    }

    /// An IME composition starts (`true`) or ends (`false`).
    pub fn compose(self, composing: bool) {
        self.composing.set_value(composing);
        if composing {
            self.close();
        } else {
            self.typed();
        }
    }

    /// Closes the list.
    pub fn close(self) {
        if self.open.get_untracked() {
            self.open.set(false);
        }
    }

    /// Applies the suggestion `index`: the caret goes after the inserted text.
    fn accept(self, index: usize) {
        let Some(suggestion) = self
            .completion
            .with_untracked(|completion| completion.suggestions.get(index).cloned())
        else {
            return;
        };
        let (value, caret) = self.text.with_untracked(|text| suggestion.apply(text));
        self.open.set(false);
        self.history.update_value(CommandHistory::reset);
        if let Some(input) = self.input.get_untracked() {
            input.set_value(&value);
        }
        self.text.set(value);
        place_caret(self.input, u32::try_from(caret).unwrap_or(u32::MAX));
    }

    /// Handles the list's keys; `true` when the key was the list's (the caller does nothing
    /// more with it).
    pub fn keydown(self, event: &KeyboardEvent) -> bool {
        let modified = event.ctrl_key() || event.alt_key() || event.meta_key();
        let count = self
            .completion
            .with_untracked(|completion| completion.suggestions.len());
        match event.key().as_str() {
            "Tab" if !modified && !event.shift_key() => {
                if count == 0 {
                    // Closed (or nothing for the caret): open it now.
                    self.typed();
                    let count = self
                        .completion
                        .with_untracked(|completion| completion.suggestions.len());
                    match count {
                        // Nothing to complete: Tab moves the focus as usual.
                        0 => {
                            self.close();
                            return false;
                        }
                        1 => self.accept(0),
                        _ => {}
                    }
                } else {
                    self.accept(self.selected.get_untracked().min(count - 1));
                }
                event.prevent_default();
                true
            }
            "ArrowDown" | "ArrowUp" if count > 0 && !modified && !event.shift_key() => {
                let current = self.selected.get_untracked().min(count - 1);
                let next = if event.key() == "ArrowDown" {
                    (current + 1) % count
                } else {
                    (current + count - 1) % count
                };
                self.selected.set(next);
                self.reveal(next);
                event.prevent_default();
                true
            }
            "Escape" if count > 0 => {
                self.close();
                event.prevent_default();
                // The list closes, not what holds the console.
                event.stop_propagation();
                true
            }
            // Enter runs the command; a caret move leaves the word.
            "Enter" | "ArrowLeft" | "ArrowRight" | "Home" | "End" | "PageUp" | "PageDown"
            | "ArrowDown" | "ArrowUp" | "Escape" => {
                self.close();
                false
            }
            _ => false,
        }
    }

    /// Scrolls the option `index` into the list's view.
    fn reveal(self, index: usize) {
        let id = format!("{}-{index}", self.id.get_value());
        if let Some(option) = document().get_element_by_id(&id) {
            option.scroll_into_view_with_bool(false);
        }
    }
}

/// Puts the caret of `input` at UTF-16 offset `caret` (past the end: at the end), now and again
/// once a new value is rendered, in case rendering moved the caret.
pub fn place_caret(input: NodeRef<Textarea>, caret: u32) {
    let place = move || {
        if let Some(input) = input.get_untracked() {
            let _ = input.set_selection_range(caret, caret);
        }
    };
    place();
    set_timeout(place, Duration::ZERO);
}

/// The suggestion list of `completer`; nothing while it has no suggestions.
#[component]
pub fn SuggestionList(completer: Completer) -> impl IntoView {
    let id = completer.list_id();
    move || {
        let suggestions = completer
            .completion
            .with(|completion| completion.suggestions.clone());
        (!suggestions.is_empty()).then(|| {
            let selected = completer.selected;
            let count = suggestions.len();
            let options = suggestions
                .into_iter()
                .enumerate()
                .map(|(index, suggestion)| {
                    let option_id = format!("{id}-{index}");
                    let name = format!("{}, {}", suggestion.label, suggestion.detail);
                    view! {
                        // `pointerdown` keeps the focus in the input: a phone's keyboard stays
                        // open.
                        <li
                            id=option_id
                            class=OPTION
                            role="option"
                            aria-label=name
                            aria-selected=move || {
                                (selected.get().min(count - 1) == index).to_string()
                            }
                            on:pointerdown=|event: web_sys::PointerEvent| event.prevent_default()
                            on:click=move |_| completer.accept(index)
                        >
                            <span class=OPTION_LABEL>{suggestion.label}</span>
                            <span class=OPTION_DETAIL>{suggestion.detail}</span>
                        </li>
                    }
                })
                .collect_view();
            // Never taller than the console above the input: a short bottom sheet cuts nothing.
            let space = completer
                .space
                .get_untracked()
                .map_or_else(String::new, |space| {
                    format!("max-height: min(22rem, 45vh, {space:.0}px)")
                });
            view! {
                <ul id=id.clone() class=LIST style=space role="listbox" aria-label="Suggestions">
                    {options}
                </ul>
            }
        })
    }
}
