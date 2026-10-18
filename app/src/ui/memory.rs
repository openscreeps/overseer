//! The drawer's Memory tab: the signed-in user's `Memory` on a shard, and its raw segments.
//!
//! - Tree ([`tree`]): all of `Memory`, or the value at the path in the path field, from the
//!   session's snapshots (`overseer::Memory::snapshot`). Objects and arrays expand from the
//!   loaded value, [`model::PAGE`] children at a time ("Show more" shows more). The status line
//!   tells when the value was read. All of `Memory` is the [`Account`]'s: the console completes
//!   its keys.
//! - Editor ([`editor`]): the selected value as JSON. Save, Delete and "Add key" queue a write
//!   that the player's code applies on its next tick: the library reads the path again (at
//!   most three times) and puts the new value into the snapshots, or the tab reports the write
//!   as late. A key with a dot, a backslash or a line break cannot be a key of a path
//!   (`overseer::memory::MemoryPath`): values under such a key are read-only.
//! - Segments ([`segments`]): load a segment by number, edit its raw text, save it (at once).
//!
//! The drawer makes one [`MemoryState`] and keeps it: the tab keeps its tree, its selection and
//! its drafts while another tab shows. On a sharded server a shard selector chooses the shard:
//! the [`Account`]'s, which the console uses too. When the shard changes, the tab forgets what
//! it showed of the old shard; the tree loads the new one when it shows.

mod editor;
mod model;
mod segments;
mod tree;

use leptos::prelude::*;

use self::segments::{SegmentState, SegmentsView};
use self::tree::{TreeState, TreeView};
use crate::ui::account::{Account, ShardSelect};
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
    " ",
    style::BTN_SM,
    " ",
    style::SEGMENT_SHAPE
);
/// A quiet small icon button of the bar, its tooltip below at its right edge.
const BAR_BUTTON: &str = concatcp!(
    style::ICON_BUTTON_GHOST_SM,
    " relative ml-auto ",
    style::TIP_END
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
    /// The session and the shard to read and write.
    account: Account,
    /// What the tab shows.
    view: RwSignal<View>,
    /// The tree and the editor.
    tree: StoredValue<TreeState>,
    /// The segments.
    segments: StoredValue<SegmentState>,
}

impl MemoryState {
    /// The Memory of `account`'s user on its shard. Loads nothing until the tab shows.
    pub fn new(account: Account) -> Self {
        let tree = TreeState::new(account);
        let segments = SegmentState::new(account);
        // Another shard: what the tab showed belongs to the old one.
        Effect::new(move |previous: Option<Option<String>>| {
            let shard = account.shard.get();
            if previous.is_some_and(|previous| previous != shard) {
                tree.forget_shard();
                segments.forget_shard();
            }
            shard
        });
        Self {
            account,
            view: RwSignal::new(View::Tree),
            tree: StoredValue::new(tree),
            segments: StoredValue::new(segments),
        }
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
                <ShardSelect account=state.account />
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
