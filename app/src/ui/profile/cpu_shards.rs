//! The CPU limit of each shard on the own profile page, on servers with the CPU allocation
//! ([`Subsystem::CpuShards`]): one row per shard (a slider, − and + buttons and a number
//! field), the CPU that is left to give, and the time of the next allowed change.
//!
//! The draft text of each shard's field is the one source of truth: the slider and the buttons
//! read it and write it. No control takes more than the CPU that the other shards leave of the
//! account's CPU ([`CpuAllocation::remaining_for`]), so the total cannot pass it while the
//! user edits. The rules of a valid allocation are the model's ([`CpuAllocation::validate`]).
//!
//! Some shards cap the CPU that a player can use (the session's usage knows the caps,
//! [`overseer::Account::usage`]; screeps.com's shard3 allows 20). A row shows that cap and
//! warns when the shard gets more, but the form does not block such an allocation: the
//! server accepts it, and a wrong cap must not block the form.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::Overseer;
use overseer::model::account::Profile;
use overseer::model::account::cpu::{self, AllocationError, CpuAllocation};
use overseer::model::platform::Subsystem;
use wasm_bindgen::JsValue;

use crate::session;
use crate::ui::form::{FormError, FormStatus};
use crate::ui::icon::{Icon, IconKind};
use crate::ui::style::{self, concatcp};
use crate::ui::usage::UsageUi;

/// How often the clock of the cooldown advances.
const TICK: Duration = Duration::from_secs(15);

/// The shard rows, stacked. The rows follow the width of this container, not of the window:
/// the panel can be narrow on a wide screen.
const ROWS: &str = "@container flex flex-col gap-3";
/// A shard row: the name and the number field, then the controls. In a container of 26rem or
/// more they share one line, and the name column has a fixed width, so every slider has the
/// same length.
const ROW: &str = "grid items-center gap-x-4 gap-y-1 @min-[26rem]:grid-cols-[11rem_minmax(0,1fr)]";
/// The name of a shard, its cap and its number field.
const HEAD: &str = "flex items-center justify-between gap-2 min-w-0";
/// The name of a shard and its cap.
const NAME: &str = "flex flex-col min-w-0";
/// A shard's name.
const ROW_LABEL: &str = concatcp!(style::LABEL, " truncate");
/// A shard's cap under its name.
const CAP: &str = "text-xs text-muted tabular-nums";
/// The −, the slider and the + of a row.
const CONTROLS: &str = "flex items-center gap-2 min-w-0";
/// A shard's slider: as long as the row allows, as tall as a small control (a finger-size
/// target on a coarse pointer).
const SLIDER: &str = "flex-1 min-w-0 h-control-sm m-0 accent-accent cursor-pointer";
/// A shard's slider while the shard gets more CPU than it can use.
const SLIDER_OVER: &str = "flex-1 min-w-0 h-control-sm m-0 accent-warn cursor-pointer";
/// A shard's number field.
const NUMBER: &str = concatcp!(style::INPUT_SM, " flex-none w-18 text-right tabular-nums");
/// The warning of a row whose shard gets more CPU than it can use (hidden while empty).
const OVER: &str = "col-span-full text-sm text-warn empty:hidden";
/// The total and the CPU left to give.
const TOTALS: &str = "flex flex-wrap items-baseline gap-x-3 gap-y-1";
/// The total while it matches the account's CPU.
const TOTAL: &str = "font-medium tabular-nums";
/// The total while it does not match the account's CPU.
const TOTAL_WRONG: &str = "font-medium tabular-nums text-error";
/// The CPU left to give, while some is left.
const LEFT: &str = "text-sm text-muted tabular-nums";
/// The CPU left to give, once all of it is given.
const LEFT_DONE: &str = "text-sm text-ok";
/// The CPU left to give, while the shards get more than the account's CPU.
const LEFT_OVER: &str = "text-sm text-error tabular-nums";

/// Why the form cannot save the allocation.
#[derive(Clone, Debug, PartialEq)]
enum Blocker {
    /// The field of this shard does not hold a whole number.
    NotANumber(String),
    /// The allocation breaks a rule of the server.
    Rule(AllocationError),
}

/// A step of the − or + button of a shard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// 1 CPU less.
    Less,
    /// 1 CPU more.
    More,
}

/// The CPU of a field: a whole number; a blank field reads as 0.
fn parse_limit(text: &str) -> Option<u64> {
    let text = text.trim();
    if text.is_empty() {
        Some(0)
    } else {
        text.parse().ok()
    }
}

/// The sum of the `fields` (shard, text), or the first shard whose field does not hold a
/// whole number.
fn total(fields: &[(String, String)]) -> Result<u64, String> {
    fields.iter().try_fold(0_u64, |sum, (shard, text)| {
        parse_limit(text)
            .map(|cpu| sum.saturating_add(cpu))
            .ok_or_else(|| shard.clone())
    })
}

/// The limits to send for the `fields` (shard, text), by shard, when every field holds a
/// whole number and the allocation keeps the account's `rules` at `now`. Otherwise the first
/// reason that blocks the change.
fn allocation(
    fields: &[(String, String)],
    rules: CpuAllocation,
    now: f64,
) -> Result<BTreeMap<String, u64>, Blocker> {
    total(fields).map_err(Blocker::NotANumber)?;
    let allocation: BTreeMap<String, u64> = fields
        .iter()
        .map(|(shard, text)| (shard.clone(), parse_limit(text).unwrap_or(0)))
        .collect();
    rules.validate(&allocation, now).map_err(Blocker::Rule)?;
    Ok(allocation)
}

/// What the form tells about `blocker` under the Save button; `None` for the cooldown, which
/// the line of the next allowed change tells.
fn blocker_text(blocker: &Blocker) -> Option<String> {
    match blocker {
        Blocker::NotANumber(shard) => Some(format!("Enter a whole number of CPU for {shard}.")),
        Blocker::Rule(AllocationError::UnknownCpu) => {
            Some("The server does not report your CPU limit.".to_owned())
        }
        Blocker::Rule(AllocationError::Total { total, cpu }) => Some(format!(
            "The shards get {total} CPU in total. Give them exactly {cpu} CPU."
        )),
        Blocker::Rule(AllocationError::Cooldown { .. }) => None,
    }
}

/// The text of the field of `shard` among the field texts `drafts`, by shard: an absent entry
/// reads as 0.
fn field_text(drafts: &BTreeMap<String, String>, shard: &str) -> String {
    drafts.get(shard).cloned().unwrap_or_else(|| "0".to_owned())
}

/// The CPU of each field text of `drafts` (by shard). A field that does not hold a whole
/// number counts as 0.
fn draft_cpu(drafts: &BTreeMap<String, String>) -> BTreeMap<String, u64> {
    drafts
        .iter()
        .map(|(shard, text)| (shard.clone(), parse_limit(text).unwrap_or(0)))
        .collect()
}

/// The CPU of `shard` among the field texts `drafts`: absent, or not a whole number, reads
/// as 0.
fn cpu_of(drafts: &BTreeMap<String, String>, shard: &str) -> u64 {
    drafts
        .get(shard)
        .and_then(|text| parse_limit(text))
        .unwrap_or(0)
}

/// The end of every slider: the account's CPU ([`CpuAllocation::total`]), or, when the
/// server does not report it, the CPU of the field texts `drafts` (by shard).
fn scale(rules: CpuAllocation, drafts: &BTreeMap<String, String>) -> u64 {
    rules.total().unwrap_or_else(|| {
        draft_cpu(drafts)
            .values()
            .fold(0, |sum, cpu| sum.saturating_add(*cpu))
    })
}

/// The most CPU `shard` can get: what the other shards of the field texts `drafts` leave of
/// the account's CPU ([`CpuAllocation::remaining_for`]); its own CPU when the server does not
/// report the account's CPU.
fn shard_max(drafts: &BTreeMap<String, String>, shard: &str, rules: CpuAllocation) -> u64 {
    rules
        .remaining_for(&draft_cpu(drafts), shard)
        .unwrap_or_else(|| cpu_of(drafts, shard))
}

/// The text that a shard's field keeps when the user enters `text`: a whole number past
/// `max` (the most CPU the shard can get) becomes `max`; other text stays as it is.
fn typed(text: &str, max: u64) -> String {
    let digits = text.trim();
    let past = match parse_limit(digits) {
        Some(cpu) => cpu > max,
        // A whole number too large for a u64.
        None => !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()),
    };
    if past {
        max.to_string()
    } else {
        text.to_owned()
    }
}

/// The CPU after a `step` from `cpu`: 1 less, down to 0, or 1 more, up to `max` (the most
/// CPU the shard can get). A step past an end gives `cpu` back.
fn stepped(cpu: u64, step: Step, max: u64) -> u64 {
    match step {
        Step::Less => cpu.saturating_sub(1),
        Step::More if cpu < max => cpu + 1,
        Step::More => cpu,
    }
}

/// What the line next to the total tells about the shards' `total` against the account's CPU
/// `limit`, and its class.
fn left(total: u64, limit: u64) -> (String, &'static str) {
    match total.cmp(&limit) {
        Ordering::Less => (format!("{} CPU left to give", limit - total), LEFT),
        Ordering::Equal => ("All CPU given".to_owned(), LEFT_DONE),
        Ordering::Greater => (format!("{} CPU too many", total - limit), LEFT_OVER),
    }
}

/// The warning for `shard` when it gets more `cpu` than its `cap` lets it use
/// ([`cpu::usable`]).
fn over_cap(shard: &str, cpu: u64, cap: Option<f64>) -> Option<String> {
    #[expect(clippy::cast_precision_loss, reason = "CPU amounts are far below 2^52")]
    let cpu = cpu as f64;
    let usable = cpu::usable(cpu, cap);
    (usable < cpu).then(|| format!("Only {usable} of {cpu} CPU usable on {shard}."))
}

/// `at` (milliseconds since the Unix epoch) as the browser's local date and time.
fn local_time(at: f64) -> String {
    String::from(
        js_sys::Date::new(&JsValue::from_f64(at)).to_locale_string("default", &JsValue::UNDEFINED),
    )
}

/// The CPU limit of each shard, with a form to change it. Shows nothing on a server without
/// the CPU allocation ([`Subsystem::CpuShards`]), and nothing while fewer than 2 shards are
/// known (the server's shards and the shards of the allocation): one shard always gets all
/// the CPU. After a change the session publishes the new profile `me`.
#[component]
pub fn CpuShards(
    overseer: Overseer,
    me: RwSignal<Option<Result<Arc<Profile>, String>>>,
) -> impl IntoView {
    if !overseer.world().supports(Subsystem::CpuShards) {
        return ().into_any();
    }
    let world_shards = overseer.world().shards().to_vec();
    let profile = move || me.get().and_then(Result::ok);
    // The server's shards, then the shards that only the allocation names.
    let shards = Memo::new(move |_| match profile() {
        Some(me) => cpu::shard_order(&world_shards, &me.cpu_shards),
        None => world_shards.clone(),
    });
    let values = RwSignal::new(BTreeMap::<String, String>::new());
    // Fill the fields from the profile whenever it (re)loads.
    Effect::new(move |_| {
        if let Some(me) = profile() {
            values.set(
                me.cpu_shards
                    .iter()
                    .map(|(shard, cpu)| (shard.clone(), cpu.to_string()))
                    .collect(),
            );
        }
    });
    // The caps are extra information: without them (a failure, a server without the shard
    // information), the rows show no cap.
    let usage = UsageUi::expect();
    let caps = Memo::new(move |_| {
        usage.usage.with(|usage| {
            usage
                .shards
                .iter()
                .filter_map(|shard| shard.cap.map(|cap| (shard.name.clone(), cap)))
                .collect::<BTreeMap<_, _>>()
        })
    });
    let now = RwSignal::new(js_sys::Date::now());
    if let Ok(handle) = set_interval_with_handle(move || now.set(js_sys::Date::now()), TICK) {
        on_cleanup(move || handle.clear());
    }
    let fields = move || {
        shards.with(|shards| {
            values.with(|values| {
                shards
                    .iter()
                    .map(|shard| (shard.clone(), field_text(values, shard)))
                    .collect::<Vec<_>>()
            })
        })
    };
    let rules = Memo::new(move |_| {
        profile().map_or_else(CpuAllocation::default, |me| CpuAllocation::of(&me))
    });
    let limit = move || rules.get().cpu;
    let updated = move || rules.get().changed;
    let scale = Memo::new(move |_| values.with(|values| scale(rules.get(), values)));
    let check = Memo::new(move |_| allocation(&fields(), rules.get(), now.get()));
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let saved = RwSignal::new(None::<String>);
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let Ok(cpu) = check.get_untracked() else {
            return;
        };
        let overseer = overseer.clone();
        busy.set(true);
        error.set(None);
        saved.set(None);
        spawn_local(async move {
            let result = overseer.account().set_cpu_shards(cpu).await;
            busy.set(false);
            match result {
                Ok(()) => saved.set(Some("CPU allocation saved.".to_owned())),
                Err(failure) => error.set(Some(session::error_message(&failure))),
            }
        });
    };
    let total_line = move || match (total(&fields()), limit()) {
        (Ok(total), Some(limit)) => format!("Total: {total} of {limit} CPU"),
        (Ok(total), None) => format!("Total: {total} CPU"),
        (Err(_), _) => "Total: not known".to_owned(),
    };
    let total_class = move || {
        if matches!(
            check.get(),
            Err(Blocker::Rule(AllocationError::Total { .. }) | Blocker::NotANumber(_))
        ) {
            TOTAL_WRONG
        } else {
            TOTAL
        }
    };
    let left_line = move || match (total(&fields()), limit()) {
        (Ok(total), Some(_)) => {
            let (text, class) = left(total, scale.get());
            Some(view! { <span class=class>{text}</span> })
        }
        _ => None,
    };
    let next_line = move || match (rules.get().next_change_at(now.get()), updated()) {
        (Some(next), _) => Some(format!("Next change allowed: {}", local_time(next))),
        (None, Some(updated)) => Some(format!(
            "Last change: {}. You can change it now.",
            local_time(updated)
        )),
        (None, None) => None,
    };
    view! {
        <section
            class=style::PANEL
            aria-labelledby="cpu-shards-title"
            hidden=move || shards.with(Vec::len) < 2
        >
            <h2 id="cpu-shards-title" class=style::H2>"CPU per shard"</h2>
            <form class=style::FORM on:submit=submit novalidate aria-busy=move || busy.get().to_string()>
                <p class=style::HELP>
                    "Give each shard a part of your CPU. Give all of your CPU limit. \
                    After a change, the server allows the next change after 12 hours."
                </p>
                <div class=ROWS>
                    <For each=move || shards.get() key=Clone::clone let(shard)>
                        <ShardRow shard values scale rules caps />
                    </For>
                </div>
                <p class=TOTALS aria-live="polite">
                    <span class=total_class>{total_line}</span>
                    {left_line}
                </p>
                <p class=style::HELP>{next_line}</p>
                <p id="cpu-shards-blocker" class=style::FIELD_ERROR aria-live="polite">
                    {move || check.get().err().as_ref().and_then(blocker_text)}
                </p>
                <FormError error />
                <FormStatus message=saved />
                <button
                    type="submit"
                    class=style::BUTTON_PRIMARY
                    aria-describedby="cpu-shards-blocker"
                    disabled=move || busy.get() || check.with(Result::is_err)
                >
                    "Save CPU allocation"
                </button>
            </form>
        </section>
    }
    .into_any()
}

/// The row of one shard's CPU limit: its name and cap, a slider from 0 to `scale`, − and +
/// buttons, and a number field. All of them edit the shard's entry of `values` (absent reads
/// as 0), and none gives the shard more than the other shards leave of the account's CPU
/// (`rules`). The row warns when the shard gets more CPU than its cap among `caps` lets it
/// use.
#[component]
fn ShardRow(
    shard: String,
    values: RwSignal<BTreeMap<String, String>>,
    scale: Memo<u64>,
    rules: Memo<CpuAllocation>,
    caps: Memo<BTreeMap<String, f64>>,
) -> impl IntoView {
    let id = format!("cpu-shard-{shard}");
    let label_id = format!("{id}-label");
    let ticks_id = format!("{id}-cap");
    let over_id = format!("{id}-over");
    let text = {
        let shard = shard.clone();
        move || values.with(|values| field_text(values, &shard))
    };
    let invalid = {
        let text = text.clone();
        move || parse_limit(&text()).is_none().then_some("true")
    };
    let cpu = {
        let shard = shard.clone();
        Memo::new(move |_| values.with(|values| cpu_of(values, &shard)))
    };
    let max = {
        let shard = shard.clone();
        Memo::new(move |_| values.with(|values| shard_max(values, &shard, rules.get())))
    };
    let cap = {
        let shard = shard.clone();
        Memo::new(move |_| caps.with(|caps| caps.get(&shard).copied()))
    };
    let over = {
        let shard = shard.clone();
        Memo::new(move |_| over_cap(&shard, cpu.get(), cap.get()))
    };
    let set = {
        let shard = shard.clone();
        Callback::new(move |text: String| {
            values.update(|values| {
                values.insert(shard.clone(), text);
            });
        })
    };
    let enter = move |event| set.run(typed(&event_target_value(&event), max.get_untracked()));
    let nudge =
        move |step| set.run(stepped(cpu.get_untracked(), step, max.get_untracked()).to_string());
    let stuck = move |step| stepped(cpu.get(), step, max.get()) == cpu.get();
    // The slider's end first, then its value: a value past the old end would stop there. The
    // draft (not `cpu`) triggers it, so that a clamped move puts the thumb back even when the
    // CPU stays the same.
    let slider = NodeRef::<leptos::html::Input>::new();
    {
        let shard = shard.clone();
        Effect::new(move |_| {
            let value = values.with(|values| cpu_of(values, &shard));
            let end = scale.get();
            if let Some(slider) = slider.get() {
                slider.set_max(&end.to_string());
                slider.set_value(&value.to_string());
            }
        });
    }
    view! {
        <div class=ROW>
            <div class=HEAD>
                <div class=NAME>
                    <label id=label_id.clone() for=id.clone() class=ROW_LABEL title=shard.clone()>
                        {shard.clone()}
                    </label>
                    {move || cap.get().map(|cap| view! { <span class=CAP>{format!("max {cap} usable")}</span> })}
                </div>
                <input
                    id=id
                    class=NUMBER
                    type="text"
                    inputmode="numeric"
                    autocomplete="off"
                    prop:value=text
                    aria-invalid=invalid
                    aria-describedby=over_id.clone()
                    on:input=enter
                />
            </div>
            <div class=CONTROLS>
                <button
                    type="button"
                    class=style::ICON_BUTTON_SM
                    aria-label=format!("Less CPU for {shard}")
                    disabled=move || stuck(Step::Less)
                    on:click=move |_| nudge(Step::Less)
                >
                    <Icon kind=IconKind::Minus />
                </button>
                <input
                    node_ref=slider
                    class=move || if over.with(Option::is_some) { SLIDER_OVER } else { SLIDER }
                    type="range"
                    min="0"
                    step="1"
                    list=ticks_id.clone()
                    aria-labelledby=label_id
                    on:input=enter
                />
                <datalist id=ticks_id>
                    {move || cap.get().map(|cap| view! { <option value=cap.to_string()></option> })}
                </datalist>
                <button
                    type="button"
                    class=style::ICON_BUTTON_SM
                    aria-label=format!("More CPU for {shard}")
                    disabled=move || stuck(Step::More)
                    on:click=move |_| nudge(Step::More)
                >
                    <Icon kind=IconKind::Plus />
                </button>
            </div>
            <p id=over_id class=OVER aria-live="polite">{move || over.get()}</p>
        </div>
    }
}

#[cfg(test)]
mod tests;
