//! The drawer's Memory tab: the signed-in user's `Memory` on a shard, and its raw segments.
//!
//! - Tree ([`tree`]): all of `Memory` (`Memory::get` with an empty path), or the value at the
//!   path in the path field. Objects and arrays expand from the loaded value, [`model::PAGE`]
//!   children at a time ("Show more" shows more). The status line tells when the tree loaded.
//! - Editor ([`editor`]): the selected value as JSON. Save, Delete and "Add key" queue a write
//!   that the player's code applies on its next tick: the tab reads the path again (at most
//!   three times) and puts the new value into the tree, or reports the write as late. A key with
//!   a dot, a backslash or a line break cannot be a part of a path ([`model`]): values under such
//!   a key are read-only.
//! - Segments ([`segments`]): load a segment by number, edit its raw text, save it (at once).
//!
//! The drawer makes one [`MemoryState`] and keeps it: the tab keeps its tree, its selection and
//! its drafts while another tab shows. On a sharded server a shard selector chooses the shard.

mod editor;
mod model;
mod segments;
mod tree;

use leptos::prelude::*;
use overseer::{Memory, Overseer};

use self::segments::{SegmentState, SegmentsView};
use self::tree::{TreeState, TreeView};
use crate::ui::icon::{Icon, IconKind};
use crate::ui::style::{self, concatcp};

/// The bar of the tab: views, shard and reload.
const BAR: &str = "flex flex-wrap items-center gap-x-1 gap-y-2 flex-none py-2 pr-2 pl-3 border-b \
    border-edge";
/// A view button of the bar: a small segment.
const VIEW_BUTTON: &str = concatcp!(
    style::BTN,
    " ",
    style::BTN_DEFAULT,
    " min-h-control-sm px-3 text-sm ",
    style::SEGMENT_SHAPE
);
/// A quiet small icon button of the bar, its tooltip below at its right edge.
const BAR_BUTTON: &str = concatcp!(
    style::ICON_BUTTON_GHOST_SM,
    " relative ml-auto ",
    style::TIP_END
);
/// A small button in the tree and the editor.
const SMALL_BUTTON: &str = concatcp!(
    style::BTN,
    " ",
    style::BTN_DEFAULT,
    " min-h-control-sm px-3 text-sm"
);
/// A small main action.
const SMALL_PRIMARY: &str = concatcp!(
    style::BTN,
    " ",
    style::BTN_PRIMARY,
    " min-h-control-sm px-3 text-sm"
);
/// A small destructive action.
const SMALL_DANGER: &str = concatcp!(
    style::BTN,
    " ",
    style::BTN_DANGER,
    " min-h-control-sm px-3 text-sm"
);
/// A text area for JSON or raw text: as wide as its panel, monospaced.
const TEXT_AREA: &str = "w-full min-w-0 px-2 py-1.5 font-mono text-sm leading-normal text-fg \
    bg-sunken border border-edge rounded-md resize-y hover:border-edge-strong \
    focus-visible:outline-offset-0 focus-visible:border-focus placeholder:text-faint \
    aria-invalid:border-error";

/// What the tab shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    /// The tree and the editor.
    Tree,
    /// The segments.
    Segments,
}

/// The Memory tab of a session; `Copy`. The tree and the segments are stored out of line: the
/// handle stays small.
#[derive(Clone, Copy)]
pub struct MemoryState {
    /// The server's shards (empty on single-world servers).
    shards: StoredValue<Vec<String>>,
    /// The shard to read and write.
    shard: RwSignal<Option<String>>,
    /// What the tab shows.
    view: RwSignal<View>,
    /// The tree and the editor.
    tree: StoredValue<TreeState>,
    /// The segments.
    segments: StoredValue<SegmentState>,
}

impl MemoryState {
    /// The Memory of `overseer`'s user on `shard` (`None`: the default shard). Loads nothing
    /// until the tab shows.
    pub fn new(overseer: &Overseer, shard: Option<String>) -> Self {
        let memory: StoredValue<Memory, LocalStorage> = StoredValue::new_local(overseer.memory());
        let shard = RwSignal::new(shard);
        Self {
            shards: StoredValue::new(overseer.world().shards().to_vec()),
            shard,
            view: RwSignal::new(View::Tree),
            tree: StoredValue::new(TreeState::new(memory, shard)),
            segments: StoredValue::new(SegmentState::new(memory, shard)),
        }
    }

    /// Reads and writes `shard` from now on: loads the tree again there.
    fn set_shard(self, shard: String) {
        self.shard.set(Some(shard));
        self.tree.get_value().change_shard();
        self.segments.get_value().change_shard();
    }
}

/// The Memory tab: the bar, then the tree and the editor, or the segments.
#[component]
pub fn MemoryPanel(
    /// The tab's state.
    state: MemoryState,
) -> impl IntoView {
    let views = [(View::Tree, "Tree"), (View::Segments, "Segments")]
        .into_iter()
        .map(|(view, label)| {
            view! {
                <button
                    type="button"
                    class=VIEW_BUTTON
                    aria-pressed=move || (state.view.get() == view).to_string()
                    on:click=move |_| state.view.set(view)
                >
                    {label}
                </button>
            }
        })
        .collect_view();
    let shards = move || {
        let shards = state.shards.get_value();
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
                    class="min-h-control-sm min-w-0 max-w-32 px-3 text-sm text-fg bg-sunken \
                        border border-edge rounded-md transition-[border-color] \
                        duration-(--duration-fast) ease-standard hover:border-edge-strong \
                        focus-visible:outline-offset-0 focus-visible:border-focus"
                    aria-label="Shard"
                    on:change=move |event| state.set_shard(event_target_value(&event))
                >
                    {options}
                </select>
            }
        })
    };
    let tree = state.tree.get_value();
    let segments = state.segments.get_value();
    let reload = move |_| match state.view.get_untracked() {
        View::Tree => tree.reload(),
        View::Segments => segments.reload(),
    };
    let body = move || match state.view.get() {
        View::Tree => view! { <TreeView state=tree /> }.into_any(),
        View::Segments => view! { <SegmentsView state=segments /> }.into_any(),
    };
    view! {
        <section class="flex flex-col flex-1 min-h-0" aria-label="Memory">
            <div class=BAR role="toolbar" aria-label="Memory tools">
                <div class=style::SEGMENTED role="group" aria-label="Show">
                    {views}
                </div>
                {shards}
                <button
                    type="button"
                    class=BAR_BUTTON
                    aria-label="Reload"
                    data-tip="Reload"
                    on:click=reload
                >
                    <Icon kind=IconKind::Refresh />
                </button>
            </div>
            {body}
        </section>
    }
}

/// `HH:MM:SS` of the local time now.
fn clock() -> String {
    let date = js_sys::Date::new_0();
    format!(
        "{:02}:{:02}:{:02}",
        date.get_hours(),
        date.get_minutes(),
        date.get_seconds()
    )
}
