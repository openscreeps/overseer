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
use crate::ui::objects::{FILTER_ICON, FILTER_INPUT};
use crate::ui::spawn_scoped;
use crate::ui::style;
use crate::ui::style::concatcp;

/// Pixels from the bottom that still count as "at the bottom" (auto-scroll follows).
const BOTTOM_SLACK: i32 = 8;
/// Most rows the input grows to.
const INPUT_ROWS: usize = 8;

/// The kinds of lines, in filter order: the kind, its filter's label, its `data-kind`.
const KINDS: [(ConsoleKind, &str, &str); 4] = [
    (ConsoleKind::Log, "Log", "log"),
    (ConsoleKind::Result, "Results", "result"),
    (ConsoleKind::Error, "Errors", "error"),
    (ConsoleKind::Command, "Commands", "command"),
];

/// A quiet small icon button of the console's bar, its tooltip below.
const BAR_BUTTON: &str = concatcp!(style::ICON_BUTTON_GHOST_SM, " relative ", style::TIP);

/// The classes of a line of kind `kind` and of its text.
const fn line_classes(kind: ConsoleKind) -> (&'static str, &'static str) {
    match kind {
        ConsoleKind::Log => (
            "flex gap-2 px-3 py-px border-l-2 border-transparent hover:bg-hover",
            "flex-1 min-w-0 whitespace-pre-wrap wrap-anywhere",
        ),
        ConsoleKind::Result => (
            "flex gap-2 px-3 py-px border-l-2 border-transparent hover:bg-hover",
            "flex-1 min-w-0 whitespace-pre-wrap wrap-anywhere text-muted",
        ),
        ConsoleKind::Error => (
            "flex gap-2 px-3 py-px border-l-2 border-error text-error bg-error-soft",
            "flex-1 min-w-0 whitespace-pre-wrap wrap-anywhere",
        ),
        ConsoleKind::Command => (
            "flex gap-2 px-3 py-px border-l-2 border-transparent text-fg hover:bg-hover",
            "flex-1 min-w-0 whitespace-pre-wrap wrap-anywhere before:content-['›_'] \
             before:text-accent",
        ),
    }
}

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

/// A kind filter of the console bar: a small segment.
const KIND_BUTTON: &str = concatcp!(
    style::BTN,
    " ",
    style::BTN_DEFAULT,
    " min-h-control-sm px-2 text-xs ",
    style::SEGMENT_SHAPE
);
/// The console bar: kinds, filter and buttons.
const BAR: &str = "flex flex-wrap items-center gap-x-1 gap-y-2 flex-none py-2 pr-2 pl-3 border-b \
    border-edge";
/// The filter of the console bar; a narrow console gives it its own row under the kinds and the
/// buttons.
const FILTER_FIELD: &str = "relative flex items-center flex-[1_1_4rem] min-w-16 mx-1 font-normal \
    @max-[440px]:order-1 @max-[440px]:basis-full @max-[440px]:m-0";
/// The output.
const OUTPUT: &str = "flex-1 min-w-0 overflow-auto py-1 font-mono text-xs leading-normal \
    bg-sunken focus-visible:-outline-offset-2";
/// The message of an empty output.
const EMPTY: &str = concatcp!(style::EMPTY_STATE, " h-full justify-center font-sans");
/// The "Paused" pill over the output: back to the latest lines.
const RESUME_BUTTON: &str = "absolute bottom-3 left-1/2 z-5 inline-flex items-center \
    justify-center gap-2 min-h-control-sm px-3 text-sm font-medium leading-tight whitespace-nowrap \
    text-fg bg-panel-3 border border-edge-strong rounded-full shadow-lg cursor-pointer \
    -translate-x-1/2 animate-fade-in-fast";

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
    // On the next frame, unless the user paused meanwhile (a scroll up between a new line and
    // this frame must not be undone).
    let to_bottom = move || {
        request_animation_frame(move || {
            if following.get_untracked()
                && let Some(output) = output.get_untracked()
            {
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
    // Scrolling up pauses; scrolling down to the bottom follows again. Only the direction counts:
    // a scroll event can arrive after the next line made the log taller (a bottom check alone
    // would read that as the user leaving the bottom), or after the user paused with the button
    // (the event of our own last scroll to the bottom must not undo that pause).
    let last_top = StoredValue::new(0);
    let on_scroll = move |_| {
        let Some(output) = output.get_untracked() else {
            return;
        };
        let top = output.scroll_top();
        let previous = last_top.get_value();
        last_top.set_value(top);
        let bottom = top + output.client_height() >= output.scroll_height() - BOTTOM_SLACK;
        if top > previous && bottom {
            if !following.get_untracked() {
                following.set(true);
            }
            unseen.set(0);
        } else if top < previous && !bottom && following.get_untracked() {
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
                    class=KIND_BUTTON
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
                class=concatcp!(style::ICON_BUTTON_GHOST_SM, " relative ", style::TIP_END)
                aria-label="Pop out console"
                data-tip="Pop out console"
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
        Some(view! {
            <p class=EMPTY>{message}</p>
        })
    };
    let paused = move || {
        (!following.get()).then(|| {
            let label = match unseen.get() {
                0 => "Paused · back to the latest".to_owned(),
                1 => "Paused · 1 new line".to_owned(),
                n => format!("Paused · {n} new lines"),
            };
            view! {
                <button
                    type="button"
                    class=RESUME_BUTTON
                    on:click=move |_| resume()
                >
                    <Icon kind=IconKind::ChevronDown />
                    {label}
                </button>
            }
        })
    };
    view! {
        <section class="@container flex flex-col flex-1 min-h-0" aria-label="Console">
            <div class=BAR role="toolbar" aria-label="Console tools">
                <div class=concatcp!(style::SEGMENTED, " @max-[440px]:mr-auto") role="group" aria-label="Show">
                    {kind_buttons}
                </div>
                // A narrow console: the filter takes its own row under the kinds and the buttons.
                <label class=FILTER_FIELD>
                    <Icon kind=IconKind::Filter class=FILTER_ICON />
                    <input
                        type="search"
                        class=FILTER_INPUT
                        placeholder="Filter"
                        aria-label="Filter output"
                        prop:value=move || query.get()
                        on:input=move |event| query.set(event_target_value(&event))
                    />
                </label>
                <button
                    type="button"
                    class=BAR_BUTTON
                    aria-label=move || if following.get() { "Pause auto-scroll" } else { "Resume auto-scroll" }
                    data-tip=move || if following.get() { "Pause auto-scroll" } else { "Resume auto-scroll" }
                    on:click=move |_| {
                        if following.get_untracked() {
                            // Where the log is now: a scroll event of our last scroll to the
                            // bottom may still come, and it is no move of the user's.
                            if let Some(output) = output.get_untracked() {
                                last_top.set_value(output.scroll_top());
                            }
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
                    class=BAR_BUTTON
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
            {move || {
                state.error.get().map(|error| {
                    view! {
                        <p class="px-3 py-2 text-sm text-error border-b border-edge" role="alert">
                            {error}
                        </p>
                    }
                })
            }}
            <div class="relative flex flex-1 min-h-0">
                <div class=OUTPUT role="log" aria-label="Console output" tabindex="0" node_ref=output on:scroll=on_scroll>
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
    let kind = KINDS[kind_index(line.kind)].2;
    let (class, text_class) = line_classes(line.kind);
    let pieces = line
        .pieces
        .iter()
        .map(|piece| match piece {
            Piece::Text {
                text,
                bold,
                italic,
                colour,
            } => {
                let class = match (*bold, *italic) {
                    (true, true) => "font-semibold italic",
                    (true, false) => "font-semibold",
                    (false, true) => "italic",
                    (false, false) => "",
                };
                view! {
                    <span class=class style:color=colour.clone()>
                        {text.clone()}
                    </span>
                }
                .into_any()
            }
            Piece::Break => view! { <br /> }.into_any(),
        })
        .collect_view();
    let shard = line
        .shard
        .clone()
        .map(|shard| view! { <span class="flex-none text-info">{shard}</span> });
    view! {
        <div class=class data-kind=kind>
            <time class="flex-none text-faint tabular-nums select-none">{line.time.clone()}</time>
            {shard}
            <span class=text_class>{pieces}</span>
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
                    class="min-h-control-sm px-3 text-sm text-fg bg-sunken border border-edge \
                        rounded-md transition-[border-color] duration-(--duration-fast) \
                        ease-standard hover:border-edge-strong focus-visible:outline-offset-0 \
                        focus-visible:border-focus"
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
            class="flex flex-row items-end gap-2 flex-none px-3 py-2 bg-panel border-t border-edge"
            on:submit=move |event| {
                event.prevent_default();
                send();
            }
        >
            {shards}
            <span class="self-center font-mono text-accent" aria-hidden="true">"›"</span>
            <textarea
                class="flex-1 min-w-0 min-h-control-sm max-h-40 px-2 py-1 font-mono text-sm \
                    leading-normal text-fg bg-sunken border border-edge rounded-md resize-none \
                    hover:border-edge-strong focus-visible:outline-offset-0 \
                    focus-visible:border-focus placeholder:text-faint"
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
            <button
                type="submit"
                class=concatcp!(style::ICON_BUTTON_SM, " relative self-end ", style::TIP_TOP)
                aria-label="Run"
                data-tip="Run (Enter)"
            >
                <Icon kind=IconKind::Send />
            </button>
        </form>
    }
}
