//! The console of the signed-in account: the live output of its code (`Console::feed`, after
//! what `Console::history` kept) coloured by kind, with timestamps, auto-scroll that pauses
//! while the user reads older lines ("Paused · n new"), filters by kind and text, and clear;
//! and an input for expressions with a command history (↑/↓, remembered per server across
//! restarts, [`history`]), multi-line with Shift+Enter, and a shard selector on sharded
//! servers.
//!
//! Console output is HTML; only a safe subset is rendered ([`markup`]), always as text nodes.
//! The drawer's Console tab and the console window (`#/<server>/<shard>/console/window`)
//! both show [`ConsolePanel`] over a [`ConsoleState`].

mod history;
mod markup;

use std::sync::Arc;
use std::time::Duration;

use leptos::html::{Div, Textarea};
use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::console::HISTORY_CAPACITY;
use overseer::net::rt::UNIX_EPOCH;
use overseer::{Console, ConsoleEntry, ConsoleKind, Overseer};
use web_sys::KeyboardEvent;

use self::history::{CommandHistory, SavedCommands};
use self::markup::Piece;
use crate::platform;
use crate::ui::icon::{Icon, IconKind};
use crate::ui::spawn_scoped;

/// Pixels from the bottom that still count as "at the bottom" (auto-scroll follows).
const BOTTOM_SLACK: i32 = 8;
/// Most rows the input grows to.
const INPUT_ROWS: usize = 8;

/// The kinds of lines, in filter order.
const KINDS: [(ConsoleKind, &str, &str); 4] = [
    (ConsoleKind::Log, "Log", "log"),
    (ConsoleKind::Result, "Results", "result"),
    (ConsoleKind::Error, "Errors", "error"),
    (ConsoleKind::Command, "Commands", "command"),
];

/// The filter index of `kind`.
const fn kind_index(kind: ConsoleKind) -> usize {
    match kind {
        ConsoleKind::Log => 0,
        ConsoleKind::Result => 1,
        ConsoleKind::Error => 2,
        ConsoleKind::Command => 3,
    }
}

/// One line as shown.
#[derive(Debug, PartialEq, Eq)]
pub struct Line {
    /// Unique in its console.
    id: u64,
    /// What it is.
    kind: ConsoleKind,
    /// Where it ran (sharded servers).
    shard: Option<String>,
    /// The text, the safe subset of its HTML.
    pieces: Vec<Piece>,
    /// The text without markup, lower-case (the filter).
    search: String,
    /// When it arrived, `HH:MM:SS` local time.
    time: String,
}

/// A console and what it showed; `Copy`.
#[derive(Clone, Copy)]
pub struct ConsoleState {
    /// The lines, oldest first, at most [`HISTORY_CAPACITY`].
    lines: RwSignal<Vec<Arc<Line>>>,
    /// The next line id.
    next: StoredValue<u64>,
    /// The library's console.
    console: StoredValue<Option<Console>, LocalStorage>,
    /// Base URL of the server (the command history's key).
    server: StoredValue<String>,
    /// The input's history.
    history: StoredValue<CommandHistory>,
    /// The server's shards (empty on single-world servers).
    pub shards: RwSignal<Vec<String>>,
    /// The shard commands run on.
    pub shard: RwSignal<Option<String>>,
    /// Why the console is not available.
    pub error: RwSignal<Option<String>>,
    /// The console is on screen (the drawer's tab is selected).
    pub shown: RwSignal<bool>,
    /// Lines that arrived while it was not on screen.
    pub unread: RwSignal<usize>,
}

impl ConsoleState {
    /// Follows the console of `overseer` (on server `server`, commands to `shard` by default)
    /// for as long as the calling component lives.
    pub fn start(overseer: &Overseer, server: &str, shard: Option<String>) -> Self {
        let console = overseer.console();
        let saved = SavedCommands::parse(platform::load_item(history::STORAGE_KEY).as_deref());
        let mut commands = saved.commands(server).to_vec();
        commands.extend(console.recent_commands());
        let state = Self {
            lines: RwSignal::new(Vec::new()),
            next: StoredValue::new(0),
            console: StoredValue::new_local(Some(console.clone())),
            server: StoredValue::new(server.to_owned()),
            history: StoredValue::new(CommandHistory::new(commands)),
            shards: RwSignal::new(overseer.world().shards().to_vec()),
            shard: RwSignal::new(shard),
            error: RwSignal::new(None),
            shown: RwSignal::new(true),
            unread: RwSignal::new(0),
        };
        match console.feed() {
            Ok(mut feed) => {
                state.push_all(&console.history());
                spawn_scoped(async move {
                    while let Ok(entry) = feed.recv().await {
                        state.push_all(std::slice::from_ref(&entry));
                    }
                });
            }
            Err(error) => state.error.set(Some(format!(
                "The console is unavailable: {}",
                crate::session::describe(&error)
            ))),
        }
        state
    }

    /// Appends `entries`.
    fn push_all(self, entries: &[ConsoleEntry]) {
        if entries.is_empty() {
            return;
        }
        let lines: Vec<Arc<Line>> = entries
            .iter()
            .map(|entry| {
                let id = self.next.get_value();
                self.next.set_value(id + 1);
                let pieces = if entry.kind == ConsoleKind::Command {
                    vec![Piece::Text {
                        text: entry.text.clone(),
                        bold: false,
                        italic: false,
                        colour: None,
                    }]
                } else {
                    markup::parse(&entry.text)
                };
                let millis = entry
                    .received
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis();
                Arc::new(Line {
                    id,
                    kind: entry.kind,
                    shard: entry.shard.clone(),
                    pieces,
                    search: entry.plain_text().to_lowercase(),
                    time: clock(millis),
                })
            })
            .collect();
        let added = lines.len();
        self.lines.try_update(|known| {
            known.extend(lines);
            if known.len() > HISTORY_CAPACITY {
                known.drain(..known.len() - HISTORY_CAPACITY);
            }
        });
        if self.shown.try_get_untracked() == Some(false) {
            self.unread.try_update(|unread| *unread += added);
        }
    }

    /// A line of the app's own (a command that could not be sent).
    fn note(self, kind: ConsoleKind, text: &str) {
        self.push_all(&[ConsoleEntry {
            kind,
            shard: None,
            text: markup_escape(text),
            received: overseer::net::rt::SystemTime::now(),
        }]);
    }

    /// Forgets the lines shown.
    fn clear(self) {
        self.lines.set(Vec::new());
        self.unread.set(0);
    }

    /// Runs `expression` on the selected shard and remembers it for the server.
    fn run(self, expression: String) {
        if expression.trim().is_empty() {
            return;
        }
        self.history
            .update_value(|history| history.record(&expression));
        let server = self.server.get_value();
        let mut saved = SavedCommands::parse(platform::load_item(history::STORAGE_KEY).as_deref());
        saved.record(&server, &expression);
        // Remembering commands is best effort.
        let _ = platform::save_item(history::STORAGE_KEY, &saved.to_json());
        let Some(console) = self.console.get_value() else {
            return;
        };
        let shard = self.shard.get_untracked();
        spawn_local(async move {
            if let Err(error) = console.execute(&expression, shard.as_deref()).await {
                self.note(
                    ConsoleKind::Error,
                    &format!(
                        "Could not run the command: {}",
                        crate::session::describe(&error)
                    ),
                );
            }
        });
    }
}

/// `text` as HTML text (so that [`markup::parse`] gives it back unchanged).
fn markup_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// `HH:MM:SS` of the local time `millis` milliseconds after the Unix epoch.
#[expect(clippy::cast_precision_loss, reason = "times are far below 2^53 ms")]
fn clock(millis: u128) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(millis as f64));
    format!(
        "{:02}:{:02}:{:02}",
        date.get_hours(),
        date.get_minutes(),
        date.get_seconds()
    )
}

/// The console: output, tools and input.
#[component]
pub fn ConsolePanel(
    /// The console.
    state: ConsoleState,
    /// Pops the console out into a window (the drawer).
    #[prop(optional)]
    pop_out: Option<Callback<()>>,
) -> impl IntoView {
    let kinds = RwSignal::new([true; 4]);
    let query = RwSignal::new(String::new());
    let following = RwSignal::new(true);
    let unseen = RwSignal::new(0_usize);
    let output = NodeRef::<Div>::new();
    let visible = Memo::new(move |_| {
        let kinds = kinds.get();
        let query = query.with(|query| query.trim().to_lowercase());
        state.lines.with(|lines| {
            lines
                .iter()
                .filter(|line| kinds[kind_index(line.kind)])
                .filter(|line| query.is_empty() || line.search.contains(&query))
                .cloned()
                .collect::<Vec<_>>()
        })
    });
    let to_bottom = move || {
        request_animation_frame(move || {
            if let Some(output) = output.get_untracked() {
                output.set_scroll_top(output.scroll_height());
            }
        });
    };
    // New lines: follow them, or count them while paused.
    Effect::new(move |previous: Option<Option<u64>>| {
        let (last, count) = state
            .lines
            .with(|lines| (lines.last().map(|line| line.id), lines.len()));
        let previous = previous.flatten();
        if following.get_untracked() {
            to_bottom();
        } else if let Some(last) = last {
            let added = previous.map_or(count, |previous| {
                usize::try_from(last.saturating_sub(previous)).unwrap_or(count)
            });
            unseen.update(|unseen| *unseen += added);
        }
        last
    });
    // Only scrolling up pauses: lines added under the view and our own scrolls to the bottom
    // never lower `scrollTop`, but a scroll event can arrive after the next line made the log
    // taller, which a bottom check alone would read as the user leaving the bottom.
    let last_top = StoredValue::new(0);
    let on_scroll = move |_| {
        let Some(output) = output.get_untracked() else {
            return;
        };
        let top = output.scroll_top();
        let up = top < last_top.get_value();
        last_top.set_value(top);
        let bottom = top + output.client_height() >= output.scroll_height() - BOTTOM_SLACK;
        if bottom {
            if !following.get_untracked() {
                following.set(true);
            }
            unseen.set(0);
        } else if up && following.get_untracked() {
            following.set(false);
        }
    };
    let resume = move || {
        following.set(true);
        unseen.set(0);
        to_bottom();
    };
    let kind_buttons = KINDS
        .iter()
        .map(|&(kind, label, _)| {
            let index = kind_index(kind);
            view! {
                <button
                    type="button"
                    aria-pressed=move || kinds.with(|kinds| kinds[index]).to_string()
                    on:click=move |_| kinds.update(|kinds| kinds[index] = !kinds[index])
                >
                    {label}
                </button>
            }
        })
        .collect_view();
    let pop_out = pop_out.map(|pop_out| {
        view! {
            <button
                type="button"
                class="icon-button sm"
                aria-label="Pop out console"
                data-tip="Pop out console"
                data-tip-pos="bottom-end"
                on:click=move |_| pop_out.run(())
            >
                <Icon kind=IconKind::PopOut />
            </button>
        }
    });
    let empty = move || {
        let message = if state.lines.with(Vec::is_empty) {
            "No output yet: your code's console.log lines and command results show here."
        } else if visible.with(Vec::is_empty) {
            "No lines match the filter."
        } else {
            return None;
        };
        Some(view! { <p class="empty-state console-empty">{message}</p> })
    };
    let paused = move || {
        (!following.get()).then(|| {
            let label = match unseen.get() {
                0 => "Paused · back to the latest".to_owned(),
                1 => "Paused · 1 new line".to_owned(),
                n => format!("Paused · {n} new lines"),
            };
            view! {
                <button type="button" class="console-resume" on:click=move |_| resume()>
                    <Icon kind=IconKind::ChevronDown />
                    {label}
                </button>
            }
        })
    };
    view! {
        <section class="console" aria-label="Console">
            <div class="console-bar" role="toolbar" aria-label="Console tools">
                <div class="segmented kinds" role="group" aria-label="Show">
                    {kind_buttons}
                </div>
                <label class="filter">
                    <Icon kind=IconKind::Filter />
                    <input
                        type="search"
                        placeholder="Filter"
                        aria-label="Filter output"
                        prop:value=move || query.get()
                        on:input=move |event| query.set(event_target_value(&event))
                    />
                </label>
                <button
                    type="button"
                    class="icon-button sm"
                    aria-label=move || if following.get() { "Pause auto-scroll" } else { "Resume auto-scroll" }
                    data-tip=move || if following.get() { "Pause auto-scroll" } else { "Resume auto-scroll" }
                    on:click=move |_| {
                        if following.get_untracked() {
                            following.set(false);
                        } else {
                            resume();
                        }
                    }
                >
                    {move || if following.get() {
                        view! { <Icon kind=IconKind::Pause /> }
                    } else {
                        view! { <Icon kind=IconKind::Play /> }
                    }}
                </button>
                <button
                    type="button"
                    class="icon-button sm"
                    aria-label="Clear console"
                    data-tip="Clear console"
                    on:click=move |_| {
                        state.clear();
                        unseen.set(0);
                    }
                >
                    <Icon kind=IconKind::Trash />
                </button>
                {pop_out}
            </div>
            {move || state.error.get().map(|error| view! { <p class="error console-error" role="alert">{error}</p> })}
            <div class="console-stage">
                <div
                    class="console-output"
                    role="log"
                    aria-label="Console output"
                    tabindex="0"
                    node_ref=output
                    on:scroll=on_scroll
                >
                    {empty}
                    <For each=move || visible.get() key=|line| line.id let(line)>
                        <LineView line />
                    </For>
                </div>
                {paused}
            </div>
            <ConsoleInput state />
        </section>
    }
}

/// One output line.
#[component]
fn LineView(line: Arc<Line>) -> impl IntoView {
    let class = KINDS[kind_index(line.kind)].2;
    let pieces = line
        .pieces
        .iter()
        .map(|piece| match piece {
            Piece::Text {
                text,
                bold,
                italic,
                colour,
            } => view! {
                <span class:b=*bold class:i=*italic style:color=colour.clone()>
                    {text.clone()}
                </span>
            }
            .into_any(),
            Piece::Break => view! { <br /> }.into_any(),
        })
        .collect_view();
    let shard = line
        .shard
        .clone()
        .map(|shard| view! { <span class="line-shard">{shard}</span> });
    view! {
        <div class=format!("line {class}")>
            <time class="line-time">{line.time.clone()}</time>
            {shard}
            <span class="line-text">{pieces}</span>
        </div>
    }
}

/// The expression input: Enter runs, Shift+Enter adds a line, ↑/↓ on the first/last line go
/// through the history.
#[component]
fn ConsoleInput(state: ConsoleState) -> impl IntoView {
    let text = RwSignal::new(String::new());
    let input = NodeRef::<Textarea>::new();
    let rows = move || text.with(|text| text.lines().count().clamp(1, INPUT_ROWS));
    let send = move || {
        let expression = text.get_untracked();
        if expression.trim().is_empty() {
            return;
        }
        text.set(String::new());
        state.run(expression);
    };
    let set = move |value: String| {
        text.set(value);
        // The caret goes to the end once the value is in.
        set_timeout(
            move || {
                if let Some(input) = input.get_untracked() {
                    let end = u32::try_from(input.value().len()).unwrap_or(u32::MAX);
                    let _ = input.set_selection_range(end, end);
                }
            },
            Duration::ZERO,
        );
    };
    let keydown = move |event: KeyboardEvent| {
        if event.is_composing() {
            return;
        }
        let Some(element) = input.get_untracked() else {
            return;
        };
        let value = element.value();
        let start = element
            .selection_start()
            .ok()
            .flatten()
            .and_then(|start| usize::try_from(start).ok())
            .unwrap_or_default();
        let end = element
            .selection_end()
            .ok()
            .flatten()
            .and_then(|end| usize::try_from(end).ok())
            .unwrap_or(value.len());
        // Selection offsets are UTF-16 units; the history only needs to know about new lines.
        let before = value
            .encode_utf16()
            .take(start)
            .any(|unit| unit == u16::from(b'\n'));
        let after = value
            .encode_utf16()
            .skip(end)
            .any(|unit| unit == u16::from(b'\n'));
        match event.key().as_str() {
            "Enter" if !event.shift_key() => {
                event.prevent_default();
                send();
            }
            "ArrowUp" if !before && !event.shift_key() => {
                if let Some(previous) = state
                    .history
                    .try_update_value(|history| history.previous(&value))
                    .flatten()
                {
                    event.prevent_default();
                    set(previous);
                }
            }
            "ArrowDown" if !after && !event.shift_key() => {
                if let Some(next) = state
                    .history
                    .try_update_value(CommandHistory::next)
                    .flatten()
                {
                    event.prevent_default();
                    set(next);
                }
            }
            _ => {}
        }
    };
    let shards = move || {
        let shards = state.shards.get();
        (!shards.is_empty()).then(|| {
            let options = shards
                .into_iter()
                .map(|shard| {
                    let selected = state
                        .shard
                        .with_untracked(|current| current.as_deref() == Some(shard.as_str()));
                    view! { <option value=shard.clone() selected=selected>{shard.clone()}</option> }
                })
                .collect_view();
            view! {
                <select
                    class="console-shard"
                    aria-label="Shard"
                    on:change=move |event| state.shard.set(Some(event_target_value(&event)))
                >
                    {options}
                </select>
            }
        })
    };
    view! {
        <form
            class="console-input"
            on:submit=move |event| {
                event.prevent_default();
                send();
            }
        >
            {shards}
            <span class="prompt" aria-hidden="true">"›"</span>
            <textarea
                node_ref=input
                rows=rows
                spellcheck="false"
                autocomplete="off"
                aria-label="Console expression"
                placeholder="Expression (Enter runs, ↑ history)"
                prop:value=move || text.get()
                on:input=move |event| {
                    text.set(event_target_value(&event));
                    state.history.update_value(CommandHistory::reset);
                }
                on:keydown=keydown
            ></textarea>
            <button type="submit" class="icon-button sm" aria-label="Run" data-tip="Run (Enter)" data-tip-pos="top">
                <Icon kind=IconKind::Send />
            </button>
        </form>
    }
}
