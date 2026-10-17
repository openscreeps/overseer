//! The tree of the Memory tab: the path field, the status line, the loaded value and its rows,
//! and the editor of the selected value under them. The phone's bottom sheet is short: there
//! the editor takes the place of the path field and the tree while it is open.

use std::collections::{HashMap, HashSet};

use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::Memory;
use overseer::memory::MemoryValue;
use serde_json::Value;

use super::editor::{Draft, Editor, Write, WriteNotice};
use super::model::{self, Keys, PAGE, Row, RowId, RowKind};
use super::{SMALL_BUTTON, clock};
use crate::session;
use crate::ui::icon::{Icon, IconKind};
use crate::ui::media::{SHEET_LAYOUT, media_query};
use crate::ui::objects::{FILTER_ICON, FILTER_INPUT};
use crate::ui::style::{self, concatcp};

/// The deepest indent of a row, in levels: a deep value still has room for its key.
const MAX_INDENT: usize = 8;

/// A row's button: the key and the preview of its value.
const ROW_BUTTON: &str = "flex items-center gap-2 flex-1 min-w-0 min-h-row pr-3 text-left \
    font-normal leading-tight whitespace-nowrap text-fg bg-transparent cursor-pointer \
    transition-colors duration-(--duration-fast) ease-standard hover:bg-hover \
    aria-[current=true]:bg-accent-soft aria-[current=true]:shadow-[inset_2px_0_0_var(--color-accent)]";
/// The expand button of an object or an array: a chevron that points down while expanded.
const EXPAND_BUTTON: &str = "group inline-flex flex-none items-center justify-center \
    size-control-sm min-h-row text-muted bg-transparent rounded-md cursor-pointer \
    transition-colors duration-(--duration-fast) ease-standard hover:text-fg hover:bg-hover";
/// The space of the expand button in the row of another value.
const EXPAND_SPACE: &str = "flex-none size-control-sm";
/// A "Show more" button.
const MORE_BUTTON: &str = concatcp!(style::LINK_BUTTON, " min-h-row text-sm");

/// The value loaded into the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loaded {
    /// Its path (empty for all of `Memory`).
    pub base: String,
    /// The value.
    pub value: MemoryValue,
}

/// The tree and the editor; `Copy`.
#[derive(Clone, Copy)]
pub struct TreeState {
    /// The library's Memory.
    pub memory: StoredValue<Memory, LocalStorage>,
    /// The shard to read and write.
    pub shard: RwSignal<Option<String>>,
    /// The text of the path field.
    path: RwSignal<String>,
    /// Why the path in the field is not correct.
    path_error: RwSignal<Option<&'static str>>,
    /// The loaded value.
    pub loaded: RwSignal<Option<Loaded>>,
    /// A load runs.
    loading: RwSignal<bool>,
    /// Why the last load failed.
    error: RwSignal<Option<String>>,
    /// When the value loaded (`HH:MM:SS`).
    at: RwSignal<Option<String>>,
    /// The number of the latest load: the result of an older one is dropped.
    loads: StoredValue<u64>,
    /// The objects and arrays expanded, by keys.
    pub expanded: RwSignal<HashSet<Keys>>,
    /// How many children show of an object or array, by keys (default [`PAGE`]).
    shown: RwSignal<HashMap<Keys, usize>>,
    /// What the editor edits.
    pub draft: RwSignal<Option<Draft>>,
    /// The editor's text: the value as JSON.
    pub text: RwSignal<String>,
    /// The new key of [`Draft::Add`].
    pub key: RwSignal<String>,
    /// The state of the latest write.
    pub write: RwSignal<Write>,
    /// The number of the latest write: an older one stops waiting.
    pub writes: StoredValue<u64>,
}

impl TreeState {
    /// The tree of `memory` on `shard`, empty.
    pub fn new(memory: StoredValue<Memory, LocalStorage>, shard: RwSignal<Option<String>>) -> Self {
        Self {
            memory,
            shard,
            path: RwSignal::new(String::new()),
            path_error: RwSignal::new(None),
            loaded: RwSignal::new(None),
            loading: RwSignal::new(false),
            error: RwSignal::new(None),
            at: RwSignal::new(None),
            loads: StoredValue::new(0),
            expanded: RwSignal::new(HashSet::new()),
            shown: RwSignal::new(HashMap::new()),
            draft: RwSignal::new(None),
            text: RwSignal::new(String::new()),
            key: RwSignal::new(String::new()),
            write: RwSignal::new(Write::Idle),
            writes: StoredValue::new(0),
        }
    }

    /// The path of the loaded value (empty for all of `Memory`, and before the first load).
    pub fn base(self) -> String {
        self.loaded
            .with_untracked(|loaded| loaded.as_ref().map(|loaded| loaded.base.clone()))
            .unwrap_or_default()
    }

    /// The path of the value at `keys` under the loaded value; `None` when a key cannot be a part
    /// of a path.
    pub fn path(self, keys: &[String]) -> Option<String> {
        model::path_of(&self.base(), keys)
    }

    /// Calls `f` with the value at `keys` under the loaded value.
    pub fn with_value<T>(self, keys: &[String], f: impl FnOnce(Option<&Value>) -> T) -> T {
        self.loaded.with_untracked(|loaded| match loaded {
            Some(Loaded {
                value: MemoryValue::Json(value),
                ..
            }) => f(model::value_at(value, keys)),
            _ => f(None),
        })
    }

    /// Loads the value at path `base`. A new path starts with everything collapsed.
    pub fn load(self, base: String) {
        let load = self.loads.get_value() + 1;
        self.loads.set_value(load);
        self.loading.set(true);
        self.error.set(None);
        let memory = self.memory.get_value();
        let shard = self.shard.get_untracked();
        spawn_local(async move {
            let result = memory.get(&base, shard.as_deref()).await;
            if self.loads.try_get_value() != Some(load) {
                return;
            }
            self.loading.try_set(false);
            match result {
                Ok(value) => {
                    if self.base() != base {
                        self.collapse();
                    }
                    self.loaded.try_set(Some(Loaded { base, value }));
                    self.at.try_set(Some(clock()));
                }
                Err(error) => {
                    self.error.try_set(Some(format!(
                        "Could not load Memory: {}",
                        session::describe(&error)
                    )));
                }
            }
        });
    }

    /// Loads the value shown again.
    pub fn reload(self) {
        self.load(self.base());
    }

    /// Forgets the tree of the previous shard and loads the same path on the new one.
    pub fn change_shard(self) {
        self.collapse();
        self.writes.update_value(|writes| *writes += 1);
        self.write.set(Write::Idle);
        let base = self.base();
        self.loaded.set(None);
        self.load(base);
    }

    /// Collapses everything and closes the editor.
    fn collapse(self) {
        self.expanded.try_update(HashSet::clear);
        self.shown.try_update(HashMap::clear);
        self.draft.try_set(None);
    }

    /// Loads the path in the path field (all of `Memory` when it is empty).
    fn go(self) {
        match model::parse_path(&self.path.get_untracked()) {
            Ok(base) => {
                self.path_error.set(None);
                self.load(base);
            }
            Err(error) => self.path_error.set(Some(error)),
        }
    }

    /// Edits the value at `keys`.
    pub fn select(self, keys: Keys) {
        let text = self.with_value(&keys, |value| {
            value
                .and_then(|value| serde_json::to_string_pretty(value).ok())
                .unwrap_or_default()
        });
        self.text.set(text);
        self.draft.set(Some(Draft::Edit(keys)));
    }

    /// Adds a key to the object at `keys`.
    pub fn add(self, keys: Keys) {
        self.key.set(String::new());
        self.text.set(String::new());
        self.draft.set(Some(Draft::Add(keys)));
    }

    /// Expands or collapses the object or array at `keys`.
    fn toggle(self, keys: Keys) {
        self.expanded.update(|expanded| {
            if !expanded.remove(&keys) {
                expanded.insert(keys);
            }
        });
    }

    /// Shows [`PAGE`] more children of the object or array at `keys`.
    fn more(self, keys: Keys) {
        self.shown.update(|shown| {
            *shown.entry(keys).or_insert(PAGE) += PAGE;
        });
    }
}

/// What the head of the tree shows of the loaded value.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Head {
    /// The path (empty for all of `Memory`).
    base: String,
    /// The value's preview; `None` when nothing is at the path.
    preview: Option<String>,
    /// The path goes through a value that is not an object.
    incorrect: bool,
    /// The value is an object (it takes new keys).
    object: bool,
}

/// The path field, the status line, the tree and the editor.
#[component]
pub fn TreeView(
    /// The tree.
    state: TreeState,
) -> impl IntoView {
    // The first time the tab shows, load all of Memory.
    if state.loaded.with_untracked(Option::is_none) && !state.loading.get_untracked() {
        state.go();
    }
    let sheet = media_query(SHEET_LAYOUT);
    let covered = move || sheet.get() && state.draft.with(Option::is_some);
    let rows = Memo::new(move |_| {
        state.loaded.with(|loaded| match loaded {
            Some(Loaded {
                value: MemoryValue::Json(value),
                ..
            }) => state.expanded.with(|expanded| {
                state
                    .shown
                    .with(|shown| model::rows(value, expanded, shown))
            }),
            _ => Vec::new(),
        })
    });
    let by_id = Memo::new(move |_| {
        rows.with(|rows| {
            rows.iter()
                .map(|row| (row.id.clone(), row.clone()))
                .collect::<HashMap<_, _>>()
        })
    });
    let ids = move || rows.with(|rows| rows.iter().map(|row| row.id.clone()).collect::<Vec<_>>());
    let status = move || {
        if state.loading.get() {
            view! {
                <span class=style::SPINNER_SMALL aria-hidden="true"></span>
                <span>"Loading…"</span>
            }
            .into_any()
        } else {
            let text = state
                .at
                .get()
                .map(|at| format!("Loaded at {at}"))
                .unwrap_or_default();
            view! { <span>{text}</span> }.into_any()
        }
    };
    view! {
        <div class="flex flex-col flex-1 min-h-0">
            <form
                class="flex items-center gap-2 flex-none px-3 py-2 border-b border-edge"
                hidden=covered
                on:submit=move |event| {
                    event.prevent_default();
                    state.go();
                }
            >
                <label class="relative flex items-center flex-1 min-w-0 font-normal">
                    <Icon kind=IconKind::Search class=FILTER_ICON />
                    <input
                        type="search"
                        class=FILTER_INPUT
                        placeholder="Path, e.g. creeps.c123"
                        aria-label="Path"
                        enterkeyhint="go"
                        spellcheck="false"
                        autocomplete="off"
                        aria-invalid=move || state.path_error.with(Option::is_some).to_string()
                        prop:value=move || state.path.get()
                        on:input=move |event| state.path.set(event_target_value(&event))
                    />
                </label>
                <button type="submit" class=SMALL_BUTTON>"Show"</button>
            </form>
            <p class="px-3 pt-1 text-sm text-error empty:hidden" role="alert">
                {move || state.path_error.get()}
            </p>
            <p
                class="flex items-center gap-2 flex-none min-h-control-sm px-3 text-xs text-muted"
                role="status"
            >
                {status}
            </p>
            <WriteNotice write=state.write />
            <p class="px-3 pb-2 text-sm text-error empty:hidden" role="alert">
                {move || state.error.get()}
            </p>
            <div
                class="flex-[1_1_45%] min-h-24 overflow-auto pb-1 border-t border-edge"
                role="group"
                aria-label="Memory tree"
                hidden=covered
            >
                <TreeHead state />
                <ul>
                    <For each=ids key=Clone::clone let(id)>
                        <RowView id rows=by_id state />
                    </For>
                </ul>
            </div>
            <Editor state />
        </div>
    }
}

/// The head of the tree: the path of the loaded value, its preview, and its buttons.
#[component]
fn TreeHead(state: TreeState) -> impl IntoView {
    let head = Memo::new(move |_| {
        state.loaded.with(|loaded| {
            loaded.as_ref().map(|loaded| Head {
                base: loaded.base.clone(),
                preview: match &loaded.value {
                    MemoryValue::Json(value) => Some(model::preview(value)),
                    MemoryValue::Undefined | MemoryValue::IncorrectPath => None,
                },
                incorrect: loaded.value == MemoryValue::IncorrectPath,
                object: matches!(loaded.value, MemoryValue::Json(Value::Object(_))),
            })
        })
    });
    move || {
        head.get().map(|head| {
            let label = if head.base.is_empty() {
                "Memory".to_owned()
            } else {
                head.base.clone()
            };
            let message = if head.incorrect {
                Some("The path goes through a value that is not an object.")
            } else if head.preview.is_none() {
                Some("Nothing is stored at this path. Edit it to store a value.")
            } else {
                None
            };
            // All of Memory is not edited at once: one key at a time.
            let edit = (!head.base.is_empty() && !head.incorrect).then(|| {
                view! {
                    <button type="button" class=SMALL_BUTTON on:click=move |_| state.select(Vec::new())>
                        "Edit"
                    </button>
                }
            });
            let add = head.object.then(|| {
                view! {
                    <button type="button" class=SMALL_BUTTON on:click=move |_| state.add(Vec::new())>
                        <Icon kind=IconKind::Plus />
                        "Add key"
                    </button>
                }
            });
            view! {
                <div class="flex flex-wrap items-center gap-x-2 gap-y-1 px-3 py-1.5">
                    <span class="min-w-0 font-mono text-sm font-semibold break-all">{label}</span>
                    <span class="font-mono text-xs text-muted">{head.preview}</span>
                    <span class="flex gap-2 ml-auto">{edit}{add}</span>
                </div>
                {message.map(|message| view! { <p class=style::EMPTY_STATE>{message}</p> })}
            }
        })
    }
}

/// One row of the tree, updated in place (a reload keeps the focus on it).
#[component]
fn RowView(
    /// The row.
    id: RowId,
    /// Every row by id.
    rows: Memo<HashMap<RowId, Row>>,
    state: TreeState,
) -> impl IntoView {
    let row = {
        let id = id.clone();
        Memo::new(move |_| rows.with(|rows| rows.get(&id).cloned()))
    };
    let depth = row
        .with_untracked(|row| row.as_ref().map_or(0, |row| row.depth))
        .min(MAX_INDENT);
    let indent = format!("--depth: {depth}");
    match id {
        RowId::More(keys) => {
            let keys = StoredValue::new(keys);
            let label = move || {
                row.with(|row| match row.as_ref().map(|row| &row.kind) {
                    Some(RowKind::More { hidden }) => {
                        format!("Show {} more ({hidden} not shown)", (*hidden).min(PAGE))
                    }
                    _ => String::new(),
                })
            };
            view! {
                <li class="flex pl-[calc(var(--depth)*0.875rem+var(--spacing-control-sm))]" style=indent>
                    <button type="button" class=MORE_BUTTON on:click=move |_| state.more(keys.get_value())>
                        {label}
                    </button>
                </li>
            }
            .into_any()
        }
        RowId::Entry(keys) => entry_row(keys, row, indent, state).into_any(),
    }
}

/// The row of the value at `keys`: its expand button (objects and arrays), its key and preview.
fn entry_row(
    keys: Keys,
    row: Memo<Option<Row>>,
    indent: String,
    state: TreeState,
) -> impl IntoView {
    let key = keys.last().cloned().unwrap_or_default();
    let keys = StoredValue::new(keys);
    let preview = Memo::new(move |_| {
        row.with(|row| match row.as_ref().map(|row| &row.kind) {
            Some(RowKind::Entry { preview, .. }) => preview.clone(),
            _ => String::new(),
        })
    });
    let expanded = Memo::new(move |_| {
        row.with(|row| match row.as_ref().map(|row| &row.kind) {
            Some(RowKind::Entry { expanded, .. }) => *expanded,
            _ => None,
        })
    });
    let current = move || {
        state.draft.with(|draft| {
            keys.with_value(|keys| matches!(draft, Some(Draft::Edit(edited)) if edited == keys))
        })
    };
    let label = {
        let key = key.clone();
        move || format!("{key} = {}", preview.get())
    };
    let children_label = format!("Children of {key}");
    let expand = move || match expanded.get() {
        Some(open) => view! {
            <button
                type="button"
                class=EXPAND_BUTTON
                aria-label=children_label.clone()
                aria-expanded=open.to_string()
                on:click=move |_| state.toggle(keys.get_value())
            >
                <span
                    class="inline-flex transition-transform duration-(--duration-fast) \
                        ease-standard group-aria-[expanded=false]:-rotate-90"
                    aria-hidden="true"
                >
                    <Icon kind=IconKind::ChevronDown />
                </span>
            </button>
        }
        .into_any(),
        None => view! { <span class=EXPAND_SPACE aria-hidden="true"></span> }.into_any(),
    };
    // Selecting an object or an array expands it too: a finger then needs one tap.
    let pick = move |_| {
        if expanded.get_untracked() == Some(false) {
            state.toggle(keys.get_value());
        }
        state.select(keys.get_value());
    };
    view! {
        <li class="flex items-stretch pl-[calc(var(--depth)*0.875rem)]" style=indent>
            {expand}
            <button
                type="button"
                class=ROW_BUTTON
                aria-label=label
                aria-current=move || current().then_some("true")
                on:click=pick
            >
                <span class="flex-none max-w-[60%] truncate font-mono text-sm">{key}</span>
                <span class="min-w-0 truncate font-mono text-xs text-muted">{preview}</span>
            </button>
        </li>
    }
}
