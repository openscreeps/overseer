//! The segments of the Memory tab: load a segment by number (`0` to `99`), edit its raw text,
//! save it. A segment write applies at once (no tick); a segment holds at most
//! [`SEGMENT_MAX_UNITS`] characters (UTF-16 units), which the library checks before it sends.

use leptos::ev::SubmitEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::memory::{SEGMENT_MAX_UNITS, SegmentError, segment_units};
use overseer::net::rt::SystemTime;

use super::TEXT_AREA;
use super::model;
use crate::session;
use crate::ui::account::Account;
use crate::ui::clock;
use crate::ui::style::{self, concatcp};

/// The segments; `Copy`.
#[derive(Clone, Copy)]
pub struct SegmentState {
    /// The session and the shard to read and write.
    account: Account,
    /// The text of the number field.
    number: RwSignal<String>,
    /// The segment in the editor.
    loaded: RwSignal<Option<u8>>,
    /// The editor's text.
    text: RwSignal<String>,
    /// A load or a save runs.
    busy: RwSignal<bool>,
    /// What the last load or save did.
    status: RwSignal<Option<String>>,
    /// Why the last load or save failed.
    error: RwSignal<Option<String>>,
    /// The number of the latest request: the result of an older one is dropped.
    requests: StoredValue<u64>,
}

impl SegmentState {
    /// The segments of `account` on its shard; none loaded.
    pub fn new(account: Account) -> Self {
        Self {
            account,
            number: RwSignal::new("0".to_owned()),
            loaded: RwSignal::new(None),
            text: RwSignal::new(String::new()),
            busy: RwSignal::new(false),
            status: RwSignal::new(None),
            error: RwSignal::new(None),
            requests: StoredValue::new(0),
        }
    }

    /// Starts a request: its number.
    fn start(self) -> u64 {
        let request = self.requests.get_value() + 1;
        self.requests.set_value(request);
        self.busy.set(true);
        self.status.set(None);
        self.error.set(None);
        request
    }

    /// Whether `request` is the latest request; ends it when it is.
    fn end(self, request: u64) -> bool {
        let latest = self.requests.try_get_value() == Some(request);
        if latest {
            self.busy.try_set(false);
        }
        latest
    }

    /// Loads the segment in the number field.
    fn load(self) {
        let Some(id) = model::parse_segment(&self.number.get_untracked()) else {
            self.error
                .set(Some("Type a segment number from 0 to 99.".to_owned()));
            return;
        };
        let request = self.start();
        let memory = self.account.memory();
        let shard = self.account.shard.get_untracked();
        spawn_local(async move {
            let result = memory.segment(id, shard.as_deref()).await;
            if !self.end(request) {
                return;
            }
            match result {
                Ok(data) => {
                    let status = if data.is_none() {
                        format!("Segment {id} is empty: nothing was written to it yet.")
                    } else {
                        format!("Loaded segment {id} at {}.", clock(SystemTime::now()))
                    };
                    self.text.try_set(data.unwrap_or_default());
                    self.loaded.try_set(Some(id));
                    self.status.try_set(Some(status));
                }
                Err(error) => {
                    self.error.try_set(Some(format!(
                        "Could not load segment {id}: {}",
                        session::error_message(&error)
                    )));
                }
            }
        });
    }

    /// Loads the segment in the editor again (else the one in the number field).
    pub fn reload(self) {
        if let Some(id) = self.loaded.get_untracked() {
            self.number.set(id.to_string());
        }
        self.load();
    }

    /// Forgets the segment of the previous shard.
    pub fn forget_shard(self) {
        self.requests.update_value(|requests| *requests += 1);
        self.busy.set(false);
        self.loaded.set(None);
        self.text.set(String::new());
        self.status.set(None);
        self.error.set(None);
    }

    /// Saves the editor's text into the segment in the editor.
    fn save(self) {
        let Some(id) = self.loaded.get_untracked() else {
            return;
        };
        let data = self.text.get_untracked();
        let request = self.start();
        let memory = self.account.memory();
        let shard = self.account.shard.get_untracked();
        spawn_local(async move {
            let result = memory.set_segment(id, &data, shard.as_deref()).await;
            if !self.end(request) {
                return;
            }
            match result {
                Ok(()) => {
                    self.status.try_set(Some(format!(
                        "Saved segment {id} at {}.",
                        clock(SystemTime::now())
                    )));
                }
                Err(overseer::Error::Segment(SegmentError::TooLong { .. })) => {
                    self.error.try_set(Some(format!(
                        "Segment {id} is too long: a segment holds at most {SEGMENT_MAX_UNITS} \
                         characters."
                    )));
                }
                Err(error) => {
                    self.error.try_set(Some(format!(
                        "Could not save segment {id}: {}",
                        session::error_message(&error)
                    )));
                }
            }
        });
    }
}

/// The segment number, Load, the segment's text and Save.
#[component]
pub fn SegmentsView(
    /// The segments.
    state: SegmentState,
) -> impl IntoView {
    let length = move || state.text.with(|text| segment_units(text));
    let save_label = move || match state.loaded.get() {
        Some(id) => format!("Save segment {id}"),
        None => "Save segment".to_owned(),
    };
    view! {
        <div class="flex flex-col flex-1 min-h-0 overflow-auto">
            <form
                class="flex flex-wrap items-end gap-2 flex-none px-3 py-2 border-b border-edge"
                on:submit=move |event: SubmitEvent| {
                    event.prevent_default();
                    state.load();
                }
            >
                <label class=concatcp!(style::FIELD, " text-sm")>
                    <span class=style::LABEL>"Segment"</span>
                    <input
                        type="number"
                        inputmode="numeric"
                        min="0"
                        max="99"
                        class=concatcp!(style::INPUT_SM, " w-24")
                        prop:value=move || state.number.get()
                        on:input=move |event| state.number.set(event_target_value(&event))
                    />
                </label>
                <button type="submit" class=style::BUTTON_SM disabled=move || state.busy.get()>
                    "Load"
                </button>
            </form>
            <form
                class="flex flex-col flex-1 gap-2 p-3"
                on:submit=move |event: SubmitEvent| {
                    event.prevent_default();
                    state.save();
                }
            >
                <p class="text-sm text-muted empty:hidden" role="status">
                    {move || state.status.get()}
                </p>
                <p class="text-sm text-error empty:hidden" role="alert">
                    {move || state.error.get()}
                </p>
                <textarea
                    class=concatcp!(TEXT_AREA, " flex-1 min-h-40")
                    aria-label="Segment text"
                    spellcheck="false"
                    autocomplete="off"
                    autocapitalize="off"
                    placeholder=move || {
                        if state.loaded.with(Option::is_some) {
                            "The segment is empty."
                        } else {
                            "Load a segment to edit its text."
                        }
                    }
                    disabled=move || state.loaded.with(Option::is_none)
                    prop:value=move || state.text.get()
                    on:input=move |event| state.text.set(event_target_value(&event))
                ></textarea>
                <div class="flex flex-wrap items-center gap-2">
                    <button
                        type="submit"
                        class=style::BUTTON_PRIMARY_SM
                        disabled=move || state.busy.get() || state.loaded.with(Option::is_none)
                    >
                        {save_label}
                    </button>
                    <span class="ml-auto text-xs text-muted tabular-nums">
                        {move || format!("{} of {SEGMENT_MAX_UNITS} characters", length())}
                    </span>
                </div>
            </form>
        </div>
    }
}
