//! The editor of the Memory tab: the selected value as JSON, or a new key and its value; Save,
//! Delete (after a confirmation) and "Add key"; and the state of the latest write.
//!
//! The server queues a write: the player's code applies it on its next tick. After the server
//! takes the write, the editor reads the path again at most three times ([`POLLS`]), because
//! screeps.com limits the reads of Memory per day. When the new value (or its absence) shows,
//! the editor puts it into the loaded tree and into all of `Memory` of the account (the
//! console completes the new key); else it reports the write as late (Reload shows it).
//! Reload shows the rest of `Memory` again.

use std::time::Duration;

use leptos::ev::SubmitEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::net::rt;
use serde_json::Value;

use super::TEXT_AREA;
use super::model::{self, Keys};
use super::tree::TreeState;
use crate::session;
use crate::ui::icon::{Icon, IconKind};
use crate::ui::style::{self, concatcp};

/// The pauses before each read of a written path: reads at about 2, 5 and 10 seconds after the
/// server took the write (a few ticks). A write that does not show by then is reported as late.
const POLLS: [Duration; 3] = [
    Duration::from_secs(2),
    Duration::from_secs(3),
    Duration::from_secs(5),
];

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
    /// Writes `value` at `keys` (path `path`) under the loaded value, or deletes the value there
    /// for `None`; then waits until the player's code applied the write.
    fn write(self, keys: Keys, path: String, value: Option<Value>) {
        let write = self.writes.get_value() + 1;
        self.writes.set_value(write);
        let memory = self.account.memory();
        let shard = self.account.shard.get_untracked();
        let base = self.base();
        self.write.set(Write::Sending(path.clone()));
        spawn_local(async move {
            let sent = match &value {
                Some(value) => memory.set(&path, value.clone(), shard.as_deref()).await,
                None => memory.delete(&path, shard.as_deref()).await,
            };
            let current = move || self.writes.try_get_value() == Some(write);
            if !current() {
                return;
            }
            if let Err(error) = sent {
                let verb = if value.is_some() { "save" } else { "delete" };
                self.write.try_set(Write::Failed(format!(
                    "Could not {verb} {path}: {}",
                    session::error_message(&error)
                )));
                return;
            }
            self.write.try_set(Write::Waiting(path.clone()));
            for pause in POLLS {
                rt::sleep(pause).await;
                if !current() {
                    return;
                }
                let Ok(read) = memory.get(&path, shard.as_deref()).await else {
                    continue;
                };
                if !current() {
                    return;
                }
                if model::shows_write(&read, value.as_ref()) {
                    let done = if value.is_some() {
                        format!("{path} is now in Memory.")
                    } else {
                        format!("{path} is deleted from Memory.")
                    };
                    self.show_write(&base, shard.as_deref(), keys, value);
                    self.write.try_set(Write::Done(done));
                    return;
                }
            }
            self.write.try_set(Write::Late(path));
        });
    }

    /// Puts the applied write of `value` at `keys` into `Memory` ([`TreeState::apply_write`]),
    /// when the write was under path `base` on `shard`; when the tree still shows that value,
    /// selects a new key, and closes the editor of a deleted value.
    fn show_write(self, base: &str, shard: Option<&str>, keys: Keys, value: Option<Value>) {
        let deleted = value.is_none();
        self.apply_write(base, shard, &keys, value);
        let same_tree = self.account.shard.try_get_untracked().flatten().as_deref() == shard
            && self.base() == base;
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
        (Some(path), true) if path.is_empty() => "New key in Memory".to_owned(),
        (Some(path), true) => format!("New key in {path}"),
        (Some(path), false) => path.clone(),
        (None, _) => keys.join(" › "),
    };
    // All of Memory is not saved or deleted at once (that would replace every key); a new key
    // of it is.
    let writable = path.as_ref().is_some_and(|path| adding || !path.is_empty());
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
            if let (true, Ok(value), Some(path)) = (key_ok, value, model::path_of(&base, &keys)) {
                state.write(keys, path, Some(value));
            }
        }
    };
    let delete = {
        let keys = keys.clone();
        let path = path.clone().unwrap_or_default();
        move |_| {
            confirming.set(false);
            state.write(keys.clone(), path.clone(), None);
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
    let path_title = path.clone().unwrap_or_default();
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
                            "Delete " <code class="font-mono break-all">{path.clone()}</code> " from Memory?"
                        </p>
                        <button type="button" class=style::BUTTON_DANGER_SM on:click=delete.clone()>"Delete for good"</button>
                        <button type="button" class=style::BUTTON_SM on:click=move |_| confirming.set(false)>"Cancel"</button>
                    </div>
                </Show>
            </form>
        </section>
    }
}
