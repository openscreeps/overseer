//! The CPU and Memory usage of the signed-in account ([`crate::usage`]): the shared state
//! ([`UsageUi`]), the status bar's usage button ([`StatusUsage`]) and the Usage panel
//! ([`UsagePanel`]).
//!
//! The session keeps the usage for every consumer ([`overseer::Account::usage`]): the recent
//! runs, the account's CPU and the CPU of each shard. [`UsageUi::follow`] shows the usage of
//! the live session that is signed in. The server does not tell the shard of a run, so the
//! CPU limit of a run is the one of the shard on the map.

mod panel;

use futures_util::future::{AbortHandle, abortable};
use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::Overseer;
use overseer::account::Usage;
use overseer::account::usage::{RunCpu, Runs};
use overseer::model::account::cpu::ShardCpu;
use tokio::sync::watch;

use crate::route::Shard;
use crate::state::AppState;
use crate::ui::icon::{Icon, IconKind};
use crate::ui::world::MapState;
use crate::usage::{self, Level};

pub use panel::UsagePanel;

/// Where the Usage panel shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// Under the top bar (opened from the account menu).
    Top,
    /// Over the status bar (opened from its usage button).
    Bottom,
}

/// The usage of the live session and the Usage panel; `Copy`, provided as context.
#[derive(Clone, Copy)]
pub struct UsageUi {
    /// The Usage panel shows, and where.
    pub open: RwSignal<Option<Anchor>>,
    /// The usage of the live session as the session keeps it; empty without one.
    pub usage: RwSignal<Usage>,
    /// The recent runs of [`Self::usage`].
    pub runs: Memo<Runs>,
    /// Now, in milliseconds since the Unix epoch, updated every second.
    pub now: Signal<u64>,
    /// The session followed ([`crate::state::Session::epoch`]).
    epoch: StoredValue<Option<u64>>,
    /// Stops copying the usage of the session followed.
    task: StoredValue<Option<AbortHandle>>,
}

impl UsageUi {
    /// No usage yet, with the clock `now` (milliseconds since the Unix epoch).
    pub fn new(now: Signal<u64>) -> Self {
        let usage = RwSignal::new(Usage::default());
        Self {
            open: RwSignal::new(None),
            usage,
            runs: Memo::new(move |_| usage.with(|usage| usage.runs.clone())),
            now,
            epoch: StoredValue::new(None),
            task: StoredValue::new(None),
        }
    }

    /// The state provided by [`crate::ui::App`].
    pub fn expect() -> Self {
        expect_context()
    }

    /// Follows the usage of the live session for as long as the page lives.
    pub fn follow(self, state: AppState) {
        Effect::new(move |_| {
            let session = state.session.with(|session| {
                session
                    .as_ref()
                    .map(|session| (session.epoch(), session.overseer.clone()))
            });
            self.start(session);
        });
    }

    /// Follows session `epoch` (`None` without a session), unless it does already.
    fn start(self, session: Option<(u64, Overseer)>) {
        let epoch = session.as_ref().map(|(epoch, _)| *epoch);
        if self.epoch.get_value() == epoch {
            return;
        }
        self.epoch.set_value(epoch);
        if let Some(task) = self.task.try_update_value(Option::take).flatten() {
            task.abort();
        }
        self.usage.set(Usage::default());
        let Some(receiver) = session.and_then(|(_, overseer)| overseer.account().usage().ok())
        else {
            self.open.set(None);
            return;
        };
        let (future, handle) = abortable(self.mirror(receiver));
        self.task.set_value(Some(handle));
        spawn_local(async move {
            // `Err` only says it was aborted.
            let _ = future.await;
        });
    }

    /// Copies every value of `receiver` into [`Self::usage`] until the session ends.
    async fn mirror(self, mut receiver: watch::Receiver<Usage>) {
        loop {
            let usage = receiver.borrow_and_update().clone();
            self.usage.set(usage);
            if receiver.changed().await.is_err() {
                break;
            }
        }
    }

    /// The CPU of each shard where the server allocates CPU to each shard; empty elsewhere
    /// and while the account is unknown (reactive).
    pub fn shards(self) -> Vec<ShardCpu> {
        self.usage.with(|usage| usage.shards.clone())
    }

    /// Whether the caps and average ticks of the shards are known (reactive).
    pub fn shards_known(self) -> bool {
        self.usage.with(|usage| usage.shards_known)
    }

    /// The CPU limit of a run: the one of the shard on the map (reactive).
    pub fn cpu_limit(self, map: MapState) -> Option<f64> {
        let shown = shown_shard(map);
        self.usage.with(|usage| usage.cpu_limit(shown.as_deref()))
    }

    /// Whether no run arrived lately ([`Usage::is_idle`]; reactive, follows the clock).
    pub fn idle(self) -> bool {
        let now = self.now.get();
        self.usage.with(|usage| usage.is_idle(now))
    }

    /// Shows the Usage panel at `anchor`, or hides it while it shows.
    pub fn toggle(self, anchor: Anchor) {
        self.open
            .update(|open| *open = if open.is_some() { None } else { Some(anchor) });
    }
}

/// The name of the shard on the map; `None` on single-world servers (reactive).
pub fn shown_shard(map: MapState) -> Option<String> {
    map.shard
        .with(|shard| shard.as_ref().and_then(Shard::name).map(str::to_owned))
}

/// The text colour of a usage at `level`, `idle` or not: around the parts of a usage (a
/// `contents` box, so that it does not change the layout).
pub const fn tone(level: Level, idle: bool) -> &'static str {
    match (level, idle) {
        (_, true) => "contents text-muted",
        (Level::Ok, false) => "contents text-fg",
        (Level::Warn, false) => "contents text-warn",
        (Level::Error, false) => "contents text-error",
    }
}

/// The fill of a mini bar ([`MINI_BAR`]) of a usage at `level`, `idle` or not; its width is the
/// share of the limit.
const fn fill(level: Level, idle: bool) -> &'static str {
    match (level, idle) {
        (_, true) => "absolute inset-y-0 left-0 rounded-full bg-faint",
        (Level::Ok, false) => "absolute inset-y-0 left-0 rounded-full bg-ok",
        (Level::Warn, false) => "absolute inset-y-0 left-0 rounded-full bg-warn",
        (Level::Error, false) => "absolute inset-y-0 left-0 rounded-full bg-error",
    }
}

/// The usage slot of the status bar (an item of its [`crate::ui::style::FIT_ROW`]): at least
/// 4rem, room for the CPU alone, else the bar drops it whole; with more space it grows into
/// what the items of the bar leave (a size container) and puts the button at its right end.
const SLOT: &str = "@container flex grow basis-16 justify-end min-w-0";
/// The usage button: it shows as much as its slot has room for.
const BUTTON: &str = "inline-flex flex-none items-center gap-3 @max-[16rem]:gap-2 h-full px-1.5 \
    rounded-sm cursor-pointer hover:bg-hover aria-expanded:bg-hover";
/// The CPU part of the button.
const CPU_PART: &str = "inline-flex items-center gap-1.5";
/// The Memory part of the button: from a 6.5rem slot on.
const MEMORY_PART: &str = "hidden @min-[6.5rem]:inline-flex items-center gap-1.5";
/// A word label of the button: from a 16rem slot on.
const LABEL: &str = "hidden @min-[16rem]:inline";
/// The long Memory label: from a 28rem slot on.
const LABEL_LONG: &str = "hidden @min-[28rem]:inline";
/// The short Memory label: in a slot of 16rem to 28rem.
const LABEL_SHORT: &str = "hidden @min-[16rem]:@max-[28rem]:inline";
/// An icon in place of a word label: in a slot narrower than 16rem.
const PART_ICON: &str = "inline flex-none size-[1.15em] @min-[16rem]:hidden";
/// A mini bar: from a 16rem slot on.
const MINI_BAR: &str = "relative hidden @min-[16rem]:block flex-none w-10 @min-[28rem]:w-12 h-1.5 \
    rounded-full bg-panel-3 overflow-hidden";
/// The long CPU value (`4.2 / 20`): from a 28rem slot on.
const CPU_LONG: &str = "hidden @min-[28rem]:inline min-w-[8ch]";
/// The long Memory value (`38.1 KB / 2048 KB`): from a 28rem slot on.
const MEMORY_LONG: &str = "hidden @min-[28rem]:inline min-w-[15ch]";
/// A short value (`21%`): in a slot narrower than 28rem.
const SHORT: &str = "min-w-[4ch] @min-[28rem]:hidden";

/// The usage in the status bar: CPU and Memory of the latest run against their limits, as a
/// button that shows the Usage panel. Shows once a run arrived in the session.
#[component]
pub fn StatusUsage() -> impl IntoView {
    let ui = UsageUi::expect();
    let map = MapState::expect();
    let latest = Memo::new(move |_| ui.runs.with(|runs| runs.latest().copied()));
    let shown = Memo::new(move |_| latest.with(Option::is_some));
    let limit = Memo::new(move |_| ui.cpu_limit(map));
    let idle = Memo::new(move |_| ui.idle());
    let cpu = move || latest.get().map_or(RunCpu::Used(0.0), |run| run.cpu);
    let memory = move || latest.get().and_then(|run| run.memory);
    let cpu_level = move || Level::of_cpu(cpu(), limit.get());
    let cpu_width = move || match cpu() {
        RunCpu::Used(ms) => usage::bar_width(ms, limit.get()),
        RunCpu::LimitHit => 100.0,
    };
    let cpu_long = move || {
        if idle.get() {
            "idle".to_owned()
        } else {
            usage::cpu_long(cpu(), limit.get())
        }
    };
    let cpu_short = move || {
        if idle.get() {
            "idle".to_owned()
        } else {
            usage::cpu_short(cpu(), limit.get())
        }
    };
    let title = move || {
        latest
            .get()
            .map(|run| usage::summary(&run, limit.get(), idle.get()))
            .unwrap_or_default()
    };
    let memory_part = move || {
        memory().map(|bytes| {
            let level = usage::memory_level(bytes);
            view! {
                <span class=MEMORY_PART>
                    <span class=move || tone(level, idle.get())>
                        <span class=LABEL_LONG>"Memory"</span>
                        <span class=LABEL_SHORT>"Mem"</span>
                        <Icon kind=IconKind::Memory class=PART_ICON />
                        <span class=MINI_BAR aria-hidden="true">
                            <span
                                class=move || fill(level, idle.get())
                                style=format!("width: {:.1}%", usage::memory_bar(bytes))
                            ></span>
                        </span>
                        <span class=MEMORY_LONG>{usage::memory_long(bytes)}</span>
                        <span class=SHORT>{usage::memory_short(bytes)}</span>
                    </span>
                </span>
            }
        })
    };
    move || {
        shown.get().then(|| {
            view! {
                <span class=SLOT>
                    <button
                        type="button"
                        class=BUTTON
                        aria-label="CPU and Memory usage"
                        aria-haspopup="dialog"
                        aria-expanded=move || if ui.open.get().is_some() { "true" } else { "false" }
                        title=title
                        data-usage-toggle=""
                        on:click=move |_| ui.toggle(Anchor::Bottom)
                    >
                        <span class=CPU_PART>
                            <span class=move || tone(cpu_level(), idle.get())>
                                <span class=LABEL>"CPU"</span>
                                <Icon kind=IconKind::Cpu class=PART_ICON />
                                <span class=MINI_BAR aria-hidden="true">
                                    <span
                                        class=move || fill(cpu_level(), idle.get())
                                        style=move || format!("width: {:.1}%", cpu_width())
                                    ></span>
                                </span>
                                <span class=CPU_LONG>{cpu_long}</span>
                                <span class=SHORT>{cpu_short}</span>
                            </span>
                        </span>
                        {memory_part}
                    </button>
                </span>
            }
        })
    }
}
