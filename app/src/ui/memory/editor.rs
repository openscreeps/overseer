//! The editor of the Memory tab: the selected value as JSON, or a new key and its value; Save,
//! Delete (after a confirmation) and "Add key"; and the state of the latest write.
//!
//! The server queues a write: the player's code applies it on its next tick. The library
//! sends it and reads the path again until the change shows
//! ([`overseer::Memory::write_and_observe`], at most three reads: screeps.com limits the reads
//! of Memory per day), and then puts it into the session's snapshots: the tree and the
//! console's completion show it. A write that does not show in time is reported as late
//! (Reload shows it). The editor shows the progress, and selects a new key when it shows.

use futures_util::StreamExt as _;
use futures_util::future::abortable;
use leptos::ev::SubmitEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::memory::{MemoryChange, MemoryPath, ObservePolicy, WriteProgress};
use serde_json::Value;

use super::TEXT_AREA;
use super::model::{self, Keys};
use super::tree::TreeState;
use crate::session;
use crate::ui::icon::{Icon, IconKind};
use crate::ui::style::{self, concatcp};

/// The editor: under the tree, as tall as its content up to half of the panel.
const EDITOR: &str = "flex-[1_1_55%] has-[>p]:flex-none min-h-0 overflow-auto p-3 bg-panel-2 \
    border-t border-edge";
/// A line of the write state.
const NOTICE: &str = "flex items-start gap-2 flex-none px-3 pb-2 text-sm";

/// What the editor edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Draft {
    /// The value at these keys under the loaded value.
    Edit(Keys),
    /// A new key of the object at these keys.
    Add(Keys),
}

/// The state of the latest write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Write {
    /// No write, or its state was dismissed.
    Idle,
    /// The write of this path goes to the server.
    Sending(String),
    /// The server took the write of this path; it applies on the next tick.
    Waiting(String),
    /// The write applied: what changed.
    Done(String),
    /// The write of this path did not show in time.
    Late(String),
    /// The write failed: why.
    Failed(String),
}

impl TreeState {
    /// Queues `change` of the value at `keys` (path `path`) under the loaded value, and shows
    /// its progress until the player's code applied it. A newer write stops observing this
    /// one.
    fn write(self, keys: Keys, path: &MemoryPath, change: MemoryChange) {
        let memory = self.account.memory();
        let shard = self.account.shard.get_untracked();
        let base = self.base();
        let text = path.to_string();
        self.write.set(Write::Sending(text.clone()));
        let deleted = change == MemoryChange::Delete;
        let mut progress = Box::pin(memory.write_and_observe(
            path,
            change,
            shard.as_deref(),
            ObservePolicy::default(),
        ));
        let (task, handle) = abortable(async move {
            while let Some(step) = progress.next().await {
                let state = match step {
                    WriteProgress::Queued => Write::Waiting(text.clone()),
                    WriteProgress::Observed(_) => {
                        self.sync();
                        self.show_write(&base, shard.as_deref(), keys.clone(), deleted);
                        Write::Done(if deleted {
                            format!("{text} is deleted from Memory.")
                        } else {
                            format!("{text} is now in Memory.")
                        })
                    }
                    WriteProgress::NotObserved => Write::Late(text.clone()),
                    WriteProgress::Failed(error) => {
                        let verb = if deleted { "delete" } else { "save" };
                        Write::Failed(format!(
                            "Could not {verb} {text}: {}",
                            session::error_message(&error)
                        ))
                    }
                };
                self.write.try_set(state);
            }
        });
        self.observe(Some(handle));
        spawn_local(async move {
            // `Err` only says it was aborted.
            let _ = task.await;
        });
    }

    /// When the tree still shows the value at path `base` on `shard`, under which the write at
    /// `keys` applied: selects a new key, and closes the editor of a deleted value. The
    /// library has put the write into the values shown.
    fn show_write(self, base: &MemoryPath, shard: Option<&str>, keys: Keys, deleted: bool) {
        let same_tree = self.account.shard.try_get_untracked().flatten().as_deref() == shard
            && self.base() == *base;
        if !same_tree {
            return;
        }
        let draft = self.draft.try_get_untracked().flatten();
        if deleted {
            if draft == Some(Draft::Edit(keys)) {
                self.draft.try_set(None);
            }
        } else if let Some(Draft::Add(parent)) = draft
            && keys.split_last().is_some_and(|(_, rest)| *rest == parent)
        {
            if !parent.is_empty() {
                self.expanded.try_update(|expanded| {
                    expanded.insert(parent);
                });
            }
            self.select(keys);
        }
    }
}

/// The state of the latest write, under the status line.
#[component]
pub fn WriteNotice(
    /// The latest write.
    write: RwSignal<Write>,
) -> impl IntoView {
    move || {
        let (icon, tone, text, role) = match write.get() {
            Write::Idle => return None,
            Write::Sending(path) => (
                None,
                "text-muted",
                format!("Sending the change of {path}…"),
                "status",
            ),
            Write::Waiting(path) => (
                None,
                "text-muted",
                format!(
                    "The server has the change of {path}. It applies on the next tick of your code…"
                ),
                "status",
            ),
            Write::Done(text) => (Some(IconKind::Check), "text-ok", text, "status"),
            Write::Late(path) => (
                Some(IconKind::Alert),
                "text-warn",
                format!(
                    "The change of {path} does not show yet. It applies when your code runs: reload to see it."
                ),
                "status",
            ),
            Write::Failed(text) => (Some(IconKind::Alert), "text-error", text, "alert"),
        };
        let mark = match icon {
            Some(kind) => view! { <Icon kind /> }.into_any(),
            None => view! { <span class=concatcp!(style::SPINNER_SMALL, " mt-1") aria-hidden="true"></span> }
                .into_any(),
        };
        Some(view! {
            <p class=NOTICE role=role>
                <span class=tone>{mark}</span>
                <span class="min-w-0 break-words">{text}</span>
                <button
                    type="button"
                    class=concatcp!(style::ICON_BUTTON_GHOST_SM, " ml-auto -mt-1")
                    aria-label="Dismiss"
                    on:click=move |_| write.set(Write::Idle)
                >
                    <Icon kind=IconKind::Close />
                </button>
            </p>
        })
    }
}

/// The editor of the selected value or of a new key (hidden while nothing is selected).
#[component]
pub fn Editor(
    /// The tree.
    state: TreeState,
) -> impl IntoView {
    move || {
        state
            .draft
            .get()
            .map(|draft| view! { <DraftEditor state draft /> })
    }
}

/// The editor of `draft`.
#[component]
fn DraftEditor(state: TreeState, draft: Draft) -> impl IntoView {
    let confirming = RwSignal::new(false);
    let json_error = RwSignal::new(None::<String>);
    let key_error = RwSignal::new(None::<&'static str>);
    let (keys, adding) = match &draft {
        Draft::Edit(keys) => (keys.clone(), false),
        Draft::Add(parent) => (parent.clone(), true),
    };
    let path = state.path(&keys);
    let base = state.base();
    let object = state.with_value(&keys, |value| value.is_some_and(Value::is_object));
    let title = match (&path, adding) {
        (Some(path), true) if path.is_root() => "New key in Memory".to_owned(),
        (Some(path), true) => format!("New key in {path}"),
        (Some(path), false) => path.to_string(),
        (None, _) => keys.join(" › "),
    };
    // All of Memory is not saved or deleted at once (that would replace every key); a new key
    // of it is.
    let writable = path.as_ref().is_some_and(|path| adding || !path.is_root());
    let can_delete = writable && !adding;
    let can_add = writable && !adding && object;
    let save = {
        let keys = keys.clone();
        move |event: SubmitEvent| {
            event.prevent_default();
            if !writable {
                return;
            }
            // Both fields are checked, so that both errors show at once.
            let mut keys = keys.clone();
            let mut key_ok = true;
            if adding {
                let key = state.key.get_untracked();
                let error = state.with_value(&keys, |parent| {
                    model::new_key_error(&key, parent.unwrap_or(&Value::Null))
                });
                key_error.set(error);
                key_ok = error.is_none();
                keys.push(key);
            }
            let value = serde_json::from_str::<Value>(&state.text.get_untracked());
            json_error.set(
                value
                    .as_ref()
                    .err()
                    .map(|error| format!("Not valid JSON: {error}.")),
            );
            if let (true, Ok(value), Ok(path)) = (key_ok, value, base.join(&keys)) {
                state.write(keys, &path, MemoryChange::Set(value));
            }
        }
    };
    let delete = {
        let keys = keys.clone();
        let path = path.clone().unwrap_or_default();
        move |_| {
            confirming.set(false);
            state.write(keys.clone(), &path, MemoryChange::Delete);
        }
    };
    let add_keys = keys.clone();
    let note = (path.is_none()).then(|| {
        view! {
            <p class=concatcp!(style::HELP, " mb-2")>
                "This value is read-only here: a key on its path is empty or has a dot, a \
                backslash or a line break, and a path cannot hold such a key. Edit its parent \
                instead."
            </p>
        }
    });
    let key_field = adding.then(|| {
        view! {
            <div class=concatcp!(style::FIELD, " mb-3")>
                <label for="memory-new-key" class=style::LABEL>"Key"</label>
                <input
                    id="memory-new-key"
                    class=concatcp!(style::INPUT, " w-full font-mono")
                    spellcheck="false"
                    autocomplete="off"
                    aria-invalid=move || key_error.with(Option::is_some).to_string()
                    aria-describedby="memory-new-key-error"
                    prop:value=move || state.key.get()
                    on:input=move |event| state.key.set(event_target_value(&event))
                />
                <p id="memory-new-key-error" class=style::FIELD_ERROR role="alert">
                    {move || key_error.get()}
                </p>
            </div>
        }
    });
    let path_title = path.as_ref().map(ToString::to_string).unwrap_or_default();
    view! {
        <section class=EDITOR aria-label="Value editor">
            <form class="flex flex-col" on:submit=save>
                <div class="flex items-start gap-2 mb-2">
                    <h3 class="flex-1 min-w-0 font-mono text-sm font-semibold break-all" title=path_title>
                        {title}
                    </h3>
                    <button
                        type="button"
                        class=concatcp!(style::ICON_BUTTON_GHOST_SM, " relative -mt-1 ", style::TIP_END)
                        aria-label="Close editor"
                        data-tip="Close editor"
                        on:click=move |_| state.draft.set(None)
                    >
                        <Icon kind=IconKind::Close />
                    </button>
                </div>
                {note}
                {key_field}
                <label for="memory-value" class=concatcp!(style::LABEL, " mb-1")>"Value (JSON)"</label>
                <textarea
                    id="memory-value"
                    class=TEXT_AREA
                    rows="8"
                    spellcheck="false"
                    autocomplete="off"
                    autocapitalize="off"
                    placeholder="{\"role\": \"harvester\"}"
                    readonly=path.is_none()
                    aria-invalid=move || json_error.with(Option::is_some).to_string()
                    aria-describedby="memory-value-error"
                    prop:value=move || state.text.get()
                    on:input=move |event| state.text.set(event_target_value(&event))
                ></textarea>
                <p id="memory-value-error" class=concatcp!(style::FIELD_ERROR, " mt-1") role="alert">
                    {move || json_error.get()}
                </p>
                <Show
                    when=move || confirming.get()
                    fallback=move || {
                        view! {
                            <div class="flex flex-wrap gap-2 mt-3">
                                <button type="submit" class=style::BUTTON_PRIMARY_SM disabled=!writable>"Save"</button>
                                {can_add.then(|| {
                                    let keys = add_keys.clone();
                                    view! {
                                        <button type="button" class=style::BUTTON_SM on:click=move |_| state.add(keys.clone())>
                                            <Icon kind=IconKind::Plus />
                                            "Add key"
                                        </button>
                                    }
                                })}
                                {can_delete.then(|| view! {
                                    <button type="button" class=style::BUTTON_DANGER_SM on:click=move |_| confirming.set(true)>
                                        <Icon kind=IconKind::Trash />
                                        "Delete"
                                    </button>
                                })}
                            </div>
                        }
                    }
                >
                    <div class="flex flex-wrap items-center gap-2 mt-3" role="group" aria-label="Confirm deletion">
                        <p class="basis-full text-sm">
                            "Delete " <code class="font-mono break-all">{path.as_ref().map(ToString::to_string)}</code> " from Memory?"
                        </p>
                        <button type="button" class=style::BUTTON_DANGER_SM on:click=delete.clone()>"Delete for good"</button>
                        <button type="button" class=style::BUTTON_SM on:click=move |_| confirming.set(false)>"Cancel"</button>
                    </div>
                </Show>
            </form>
        </section>
    }
}
