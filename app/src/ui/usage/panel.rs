//! The Usage panel: the latest run's CPU and Memory against their limits, their average and
//! peak over the recent runs with a sparkline each, and the CPU and the average tick of each
//! shard where the server allocates CPU to each shard.

use leptos::html::{Button, Section};
use leptos::prelude::*;
use overseer::account::usage::{RUNS_KEPT, RunCpu, Runs};
use overseer::model::account::cpu::ShardCpu;
use overseer::model::memory::MEMORY_CAPACITY;
use wasm_bindgen::JsCast;
use web_sys::KeyboardEvent;

use crate::route::Route;
use crate::state::AppState;
use crate::tick;
use crate::ui::icon::{Icon, IconKind};
use crate::ui::style::{self, concatcp};
use crate::ui::usage::{Anchor, UsageUi, shown_shard, tone};
use crate::ui::world::MapState;
use crate::usage::{self, Level};

/// The width of a panel: at most 28rem, inside the window and its safe-area insets.
const WIDTH: &str = "w-[min(28rem,calc(100vw-1.5rem-var(--spacing-safe-l)-var(--spacing-safe-r)))]";
/// The panel under the top bar.
const PANEL_TOP: &str = concatcp!(
    style::FLOATING,
    " top-[calc(var(--spacing-topbar-safe)+0.5rem)] z-40 origin-top-right ",
    WIDTH
);
/// The panel over the status bar.
const PANEL_BOTTOM: &str = concatcp!(
    style::FLOATING,
    " bottom-[calc(var(--spacing-statusbar-safe)+0.5rem)] z-40 origin-bottom-right ",
    WIDTH
);
/// A part of the panel (CPU, Memory, shards).
const PART: &str = "mt-4 pt-4 border-t border-edge";
/// The head of a part: its name, then the latest value.
const PART_HEAD: &str = "flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1 mb-2";
/// The name of a part.
const PART_TITLE: &str = "font-semibold";
/// The figures of a part: average, peak, limit.
const FIGURES: &str = "grid grid-cols-3 gap-2 mt-2 text-sm tabular-nums";
/// The name of a figure.
const FIGURE_NAME: &str = "text-xs text-muted";
/// A warning under the shard table.
const SHARD_WARNING: &str =
    "mt-3 px-3 py-2 text-sm bg-warn-soft border-l-3 border-warn rounded-r-sm";
/// The width of a sparkline's box: one unit for each step between two runs.
#[expect(clippy::cast_precision_loss, reason = "the window holds a few runs")]
const SPARK_WIDTH: f64 = (RUNS_KEPT - 1) as f64;
/// The height of a sparkline's box.
const SPARK_HEIGHT: f64 = 32.0;

/// The meter bar of a usage at `level`, `idle` or not.
const fn meter_bar(level: Level, idle: bool) -> &'static str {
    match (level, idle) {
        (_, true) => style::METER_BAR_IDLE,
        (Level::Ok, false) => style::METER_BAR_HITS,
        (Level::Warn, false) => style::METER_BAR_WARN,
        (Level::Error, false) => style::METER_BAR_ERROR,
    }
}

/// The Usage panel, under the top bar or over the status bar ([`Anchor`]). Escape and a
/// pointer press outside it close it.
#[component]
pub fn UsagePanel() -> impl IntoView {
    let ui = UsageUi::expect();
    let map = MapState::expect();
    let root = NodeRef::<Section>::new();
    let close = NodeRef::<Button>::new();
    Effect::new(move |_| {
        if ui.open.get().is_some() {
            request_animation_frame(move || {
                if let Some(close) = close.get_untracked() {
                    let _ = close.focus();
                }
            });
        }
    });
    // The usage button toggles the panel itself: a press on it is not outside.
    let outside = window_event_listener(leptos::ev::pointerdown, move |event| {
        if ui.open.get_untracked().is_none() {
            return;
        }
        let Some(target) = event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
        else {
            return;
        };
        let toggle = target
            .closest("[data-usage-toggle]")
            .ok()
            .flatten()
            .is_some();
        let inside = root
            .get_untracked()
            .is_some_and(|root| root.contains(Some(&target)));
        if !toggle && !inside {
            ui.open.set(None);
        }
    });
    on_cleanup(move || outside.remove());
    let keydown = move |event: KeyboardEvent| {
        if event.key() == "Escape" {
            event.prevent_default();
            ui.open.set(None);
        }
    };
    let limit = Memo::new(move |_| ui.cpu_limit(map));
    let idle = Memo::new(move |_| ui.idle());
    let shards = Memo::new(move |_| ui.shards());
    let status = move || {
        let now = ui.now.get();
        ui.runs.with(|runs| match runs.latest() {
            None => "Waiting for the first run of your code…".to_owned(),
            Some(run) => {
                let ago = usage::ago(now.saturating_sub(run.at));
                if runs.is_idle(now) {
                    format!("{}The latest run arrived {ago}.", usage::idle_text())
                } else {
                    format!(
                        "Latest run {ago}. Average and peak of the last {} runs.",
                        runs.len()
                    )
                }
            }
        })
    };
    move || {
        ui.open.get().map(|anchor| {
            view! {
                <section
                    class=match anchor {
                        Anchor::Top => PANEL_TOP,
                        Anchor::Bottom => PANEL_BOTTOM,
                    }
                    node_ref=root
                    role="dialog"
                    aria-labelledby="usage-title"
                    on:keydown=keydown
                >
                    <div class=style::FLOATING_HEAD>
                        <h2 id="usage-title" class=style::FLOATING_TITLE>
                            <Icon kind=IconKind::Cpu class=style::ICON_MUTED />
                            "Usage"
                        </h2>
                        <button
                            type="button"
                            class=concatcp!(style::ICON_BUTTON_GHOST_SM, " relative ", style::TIP_LEFT)
                            node_ref=close
                            aria-label="Close usage"
                            data-tip="Close"
                            on:click=move |_| ui.open.set(None)
                        >
                            <Icon kind=IconKind::Close />
                        </button>
                    </div>
                    <p class=style::HELP role="status">{status}</p>
                    <CpuPart limit idle shards />
                    <MemoryPart idle />
                    <ShardPart shards />
                </section>
            }
        })
    }
}

/// The CPU of the latest run against `limit`, with the average, peak and limit hits of the
/// recent runs and their sparkline.
#[component]
fn CpuPart(
    limit: Memo<Option<f64>>,
    idle: Memo<bool>,
    shards: Memo<Vec<ShardCpu>>,
) -> impl IntoView {
    let ui = UsageUi::expect();
    let map = MapState::expect();
    let latest = Memo::new(move |_| ui.runs.with(|runs| runs.latest().map(|run| run.cpu)));
    let stats = Memo::new(move |_| ui.runs.with(Runs::cpu));
    let level = move || {
        latest
            .get()
            .map_or(Level::Ok, |cpu| Level::of_cpu(cpu, limit.get()))
    };
    let value = move || {
        latest.get().map_or_else(
            || "no run yet".to_owned(),
            |cpu| usage::cpu_sentence(cpu, limit.get()),
        )
    };
    let bar = move || {
        let limit = limit.get()?;
        let used = match latest.get()? {
            RunCpu::Used(ms) => ms.min(limit),
            RunCpu::LimitHit => limit,
        };
        Some(view! {
            <progress
                class=meter_bar(level(), idle.get())
                max=limit.to_string()
                value=used.to_string()
                aria-label="CPU of the latest run"
            ></progress>
        })
    };
    let used = move || stats.get().and_then(|stats| stats.used);
    let average = move || {
        used().map_or_else(
            || "none".to_owned(),
            |used| format!("{:.2} ms", used.average),
        )
    };
    let peak = move || match stats.get() {
        Some(stats) if stats.limit_hits > 0 => "limit hit".to_owned(),
        _ => used().map_or_else(|| "none".to_owned(), |used| format!("{:.2} ms", used.peak)),
    };
    let limit_text = move || {
        limit.get().map_or_else(
            || "unknown".to_owned(),
            |limit| format!("{} CPU", usage::cpu_amount(limit)),
        )
    };
    let hits = move || {
        let stats = stats.get()?;
        (stats.limit_hits > 0).then(|| {
            view! {
                <p class="mt-2 text-sm text-error">
                    {format!(
                        "{} of the last {} runs hit the CPU limit.",
                        stats.limit_hits,
                        ui.runs.with(Runs::len),
                    )}
                </p>
            }
        })
    };
    // Where the server allocates CPU to each shard, say which limit the bar uses.
    let limit_note = move || {
        let shown = shown_shard(map);
        let note = shards.with(|shards| {
            if shards.is_empty() {
                return None;
            }
            Some(
                shards
                    .iter()
                    .find(|shard| shown.as_deref() == Some(shard.name.as_str()))
                    .map_or_else(
                        || "The limit is the CPU of the account.".to_owned(),
                        |shard| {
                            format!(
                                "The limit is the usable CPU of {}, the shard on the map.",
                                shard.name
                            )
                        },
                    ),
            )
        })?;
        Some(view! { <p class=concatcp!(style::HELP, " mt-2")>{note}</p> })
    };
    let peak_value = move || used().map(|used| used.peak);
    let top = Signal::derive(move || usage::sparkline_top(limit.get(), peak_value()));
    let values = Signal::derive(move || ui.runs.with(Runs::cpu_series));
    view! {
        <section class=PART aria-labelledby="usage-cpu-title">
            <div class=PART_HEAD>
                <h3 id="usage-cpu-title" class=PART_TITLE>"CPU"</h3>
                <span class=move || tone(level(), idle.get())>
                    <span class="text-sm tabular-nums">{value}</span>
                </span>
            </div>
            {bar}
            <Sparkline
                values
                top
                limit=Signal::from(limit)
                scale=Signal::derive(move || format!("{:.1} ms", top.get()))
                label="CPU of the recent runs against the limit"
            />
            <dl class=FIGURES>
                <div>
                    <dt class=FIGURE_NAME>"Average"</dt>
                    <dd>{average}</dd>
                </div>
                <div>
                    <dt class=FIGURE_NAME>"Peak"</dt>
                    <dd>{peak}</dd>
                </div>
                <div>
                    <dt class=FIGURE_NAME>"Limit"</dt>
                    <dd>{limit_text}</dd>
                </div>
            </dl>
            {hits}
            {limit_note}
        </section>
    }
}

/// The Memory size of the latest run against the limit, with the average and peak of the
/// recent runs and their sparkline.
#[component]
fn MemoryPart(idle: Memo<bool>) -> impl IntoView {
    let ui = UsageUi::expect();
    let latest = Memo::new(move |_| {
        ui.runs
            .with(|runs| runs.latest().and_then(|run| run.memory))
    });
    let stats = Memo::new(move |_| ui.runs.with(Runs::memory));
    let level = move || latest.get().map_or(Level::Ok, usage::memory_level);
    let value = move || {
        latest
            .get()
            .map_or_else(|| "not reported".to_owned(), usage::memory_sentence)
    };
    let bar = move || {
        let bytes = latest.get()?;
        Some(view! {
            <progress
                class=meter_bar(level(), idle.get())
                max=MEMORY_CAPACITY.to_string()
                value=bytes.min(MEMORY_CAPACITY).to_string()
                aria-label="Memory of the latest run"
            ></progress>
        })
    };
    let average = move || {
        stats.get().map_or_else(
            || "none".to_owned(),
            |stats| usage::memory_amount(stats.average),
        )
    };
    let peak = move || {
        stats.get().map_or_else(
            || "none".to_owned(),
            |stats| usage::memory_amount(stats.peak),
        )
    };
    #[expect(clippy::cast_precision_loss, reason = "2 MiB is exact")]
    let memory_limit = MEMORY_CAPACITY as f64;
    let top = Signal::derive(move || {
        usage::sparkline_top(Some(memory_limit), stats.get().map(|stats| stats.peak))
    });
    let values = Signal::derive(move || ui.runs.with(Runs::memory_series));
    view! {
        <section class=PART aria-labelledby="usage-memory-title">
            <div class=PART_HEAD>
                <h3 id="usage-memory-title" class=PART_TITLE>"Memory"</h3>
                <span class=move || tone(level(), idle.get())>
                    <span class="text-sm tabular-nums">{value}</span>
                </span>
            </div>
            {bar}
            <Sparkline
                values
                top
                limit=Signal::derive(move || Some(memory_limit))
                scale=Signal::derive(move || usage::memory_amount(top.get()))
                label="Memory of the recent runs against the limit"
            />
            <dl class=FIGURES>
                <div>
                    <dt class=FIGURE_NAME>"Average"</dt>
                    <dd>{average}</dd>
                </div>
                <div>
                    <dt class=FIGURE_NAME>"Peak"</dt>
                    <dd>{peak}</dd>
                </div>
                <div>
                    <dt class=FIGURE_NAME>"Limit"</dt>
                    <dd>{format!("{} KB", usage::cpu_amount(usage::kilobytes(MEMORY_CAPACITY)))}</dd>
                </div>
            </dl>
        </section>
    }
}

/// A sparkline of the recent runs' `values` with `top` at its top edge (named by `scale` in a
/// corner) and the `limit` as a dashed line while it is on the scale; runs that hit the limit
/// (infinite values) get a mark at the top edge.
#[component]
fn Sparkline(
    values: Signal<Vec<f64>>,
    top: Signal<f64>,
    limit: Signal<Option<f64>>,
    scale: Signal<String>,
    label: &'static str,
) -> impl IntoView {
    let points = move || {
        values.with(|values| usage::sparkline(values, top.get(), SPARK_WIDTH, SPARK_HEIGHT))
    };
    let limit_line = move || {
        let top = top.get();
        limit.get().filter(|limit| *limit <= top).map(|limit| {
            let y = usage::sparkline_y(limit, top, SPARK_HEIGHT);
            view! {
                <line
                    x1="0"
                    x2=SPARK_WIDTH.to_string()
                    y1=y.to_string()
                    y2=y.to_string()
                    class="stroke-faint"
                    stroke-width="1"
                    stroke-dasharray="4 3"
                    vector-effect="non-scaling-stroke"
                ></line>
            }
        })
    };
    let hits = move || {
        values.with(|values| {
            let count = values.len();
            values
                .iter()
                .enumerate()
                .filter(|(_, value)| value.is_infinite())
                .map(|(index, _)| {
                    let x = usage::sparkline_x(index, count, SPARK_WIDTH).to_string();
                    view! {
                        <line
                            x1=x.clone()
                            x2=x
                            y1="0"
                            y2="8"
                            class="stroke-error"
                            stroke-width="2"
                            vector-effect="non-scaling-stroke"
                        ></line>
                    }
                })
                .collect_view()
        })
    };
    view! {
        <div class="relative mt-2">
            <svg
                class="block w-full h-10 bg-sunken rounded-sm"
                viewBox=format!("0 0 {SPARK_WIDTH} {SPARK_HEIGHT}")
                preserveAspectRatio="none"
                role="img"
                aria-label=label
            >
                {limit_line}
                <polyline
                    points=points
                    class="fill-none stroke-info"
                    stroke-width="1.5"
                    stroke-linejoin="round"
                    vector-effect="non-scaling-stroke"
                ></polyline>
                {hits}
            </svg>
            <span
                class="absolute top-0.5 left-1 px-0.5 text-xs leading-none text-faint tabular-nums \
                    bg-sunken rounded-xs"
                aria-hidden="true"
            >
                {scale}
            </span>
        </div>
    }
}

/// The CPU of each shard where the server allocates CPU to each shard: the assigned CPU, the
/// shard's cap and the usable CPU, with the shard's average tick, the shard on the map marked,
/// and the way to the allocation on the profile page. Nothing elsewhere.
#[component]
fn ShardPart(shards: Memo<Vec<ShardCpu>>) -> impl IntoView {
    let state = AppState::expect();
    let ui = UsageUi::expect();
    let map = MapState::expect();
    let info_known = Memo::new(move |_| ui.shards_known());
    let shown = Memo::new(move |_| shown_shard(map));
    let open_profile = move |_| {
        let slug = state
            .session_server()
            .and_then(|server| state.slug(&server.url));
        if let Some(server) = slug {
            ui.open.set(None);
            state.navigate(Route::Profile { server });
        }
    };
    let rows = move || {
        shards
            .get()
            .into_iter()
            .map(|shard| {
                let on_map = shown.with(|shown| shown.as_deref() == Some(shard.name.as_str()));
                let cap = match (shard.cap, info_known.get()) {
                    (Some(cap), _) => usage::cpu_amount(cap),
                    (None, true) => "none".to_owned(),
                    (None, false) => "unknown".to_owned(),
                };
                let average_tick = shard
                    .average_tick
                    .map_or_else(|| "unknown".to_owned(), tick::compact);
                let usable_class = if shard.over_cap() {
                    concatcp!(style::TABLE_CELL, " text-warn")
                } else {
                    style::TABLE_CELL
                };
                view! {
                    <tr>
                        <th scope="row" class=style::TABLE_ROW>
                            {shard.name.clone()}
                            {on_map.then(|| view! { <span class="ml-1.5 text-xs text-muted">"on the map"</span> })}
                        </th>
                        <td class=style::TABLE_CELL>{usage::cpu_amount(shard.assigned)}</td>
                        <td class=style::TABLE_CELL>{cap}</td>
                        <td class=usable_class>{usage::cpu_amount(shard.usable())}</td>
                        <td class=style::TABLE_CELL>{average_tick}</td>
                    </tr>
                }
            })
            .collect_view()
    };
    let over_cap = move || {
        shards
            .get()
            .into_iter()
            .filter(ShardCpu::over_cap)
            .map(|shard| {
                view! {
                    <p class=SHARD_WARNING>
                        {format!(
                            "{} lets you use at most {} CPU: {} of the {} CPU you assign to it stay unused.",
                            shard.name,
                            usage::cpu_amount(shard.cap.unwrap_or_default()),
                            usage::cpu_amount(shard.assigned - shard.usable()),
                            usage::cpu_amount(shard.assigned),
                        )}
                    </p>
                }
            })
            .collect_view()
    };
    move || {
        (!shards.with(Vec::is_empty)).then(|| {
            view! {
                <section class=PART aria-labelledby="usage-shards-title">
                    <h3 id="usage-shards-title" class=concatcp!(PART_TITLE, " mb-1")>"Shards"</h3>
                    <p class=style::HELP>
                        "The server reports the usage of the latest run without naming its shard."
                    </p>
                    <table class="w-full mt-2 border-collapse text-sm tabular-nums">
                        <thead>
                            <tr>
                                <th scope="col" class=style::TABLE_COLUMN>"Shard"</th>
                                <th scope="col" class=style::TABLE_COLUMN>"Assigned"</th>
                                <th scope="col" class=style::TABLE_COLUMN>"Cap"</th>
                                <th scope="col" class=style::TABLE_COLUMN>"Usable"</th>
                                <th scope="col" class=style::TABLE_COLUMN>"Average tick"</th>
                            </tr>
                        </thead>
                        <tbody>{rows}</tbody>
                    </table>
                    {over_cap}
                    <div class=style::ACTIONS>
                        <button type="button" class=style::BUTTON_SM on:click=open_profile>
                            "Change the CPU allocation"
                        </button>
                    </div>
                </section>
            }
        })
    }
}
