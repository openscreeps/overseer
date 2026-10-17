//! The CPU limit of each shard on the own profile page, on servers with the CPU allocation
//! ([`Subsystem::CpuShards`]): one number field per shard, the total against the account's
//! CPU, and the time of the next allowed change.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::Overseer;
use overseer::model::account::Profile;
use overseer::model::platform::Subsystem;
use wasm_bindgen::JsValue;

use crate::session;
use crate::ui::form::{FormError, FormStatus};
use crate::ui::style::{self, concatcp};

/// The time the server makes the account wait between two changes of the allocation: 12
/// hours, in milliseconds.
const COOLDOWN_MS: f64 = 12.0 * 60.0 * 60.0 * 1000.0;

/// How often the clock of the cooldown advances.
const TICK: Duration = Duration::from_secs(15);

/// The shard fields: side by side, as many as fit, so they stack on narrow screens.
const FIELDS: &str = "grid grid-cols-[repeat(auto-fill,minmax(6.5rem,1fr))] gap-3";
/// A shard's number field, as wide as its column.
const INPUT: &str = concatcp!(style::INPUT, " w-full min-w-0 tabular-nums");
/// The total while it matches the account's CPU.
const TOTAL: &str = "font-medium tabular-nums";
/// The total while it does not match the account's CPU.
const TOTAL_WRONG: &str = "font-medium tabular-nums text-error";

/// Why the form cannot save the allocation.
#[derive(Clone, Debug, PartialEq)]
enum Blocker {
    /// The field of this shard does not hold a whole number.
    NotANumber(String),
    /// The server does not report the account's CPU.
    UnknownLimit,
    /// The limits do not total the account's CPU.
    Total {
        /// The sum of the fields.
        total: u32,
        /// The account's CPU.
        limit: f64,
    },
    /// The previous change is too recent. The next change is allowed at this time, in
    /// milliseconds since the Unix epoch.
    Cooldown(f64),
}

/// The CPU of a field: a whole number; a blank field reads as 0.
fn parse_limit(text: &str) -> Option<u32> {
    let text = text.trim();
    if text.is_empty() {
        Some(0)
    } else {
        text.parse().ok()
    }
}

/// The sum of the `fields` (shard, text), or the first shard whose field does not hold a
/// whole number.
fn total(fields: &[(String, String)]) -> Result<u32, String> {
    fields.iter().try_fold(0_u32, |sum, (shard, text)| {
        parse_limit(text)
            .map(|cpu| sum.saturating_add(cpu))
            .ok_or_else(|| shard.clone())
    })
}

/// When the next change is allowed, in milliseconds since the Unix epoch: 12 hours after the
/// previous change `updated`. `None` when that time is not after `now`, or when no change is
/// known.
fn next_change(updated: Option<f64>, now: f64) -> Option<f64> {
    updated
        .map(|updated| updated + COOLDOWN_MS)
        .filter(|next| *next > now)
}

/// The limits to send for the `fields` (shard, text), by shard, when the account's CPU
/// `limit` is known, the fields total it, and the cooldown after the previous change
/// `updated` is over at `now`. Otherwise the first reason that blocks the change.
fn allocation(
    fields: &[(String, String)],
    limit: Option<f64>,
    updated: Option<f64>,
    now: f64,
) -> Result<BTreeMap<String, u64>, Blocker> {
    let total = total(fields).map_err(Blocker::NotANumber)?;
    let limit = limit.ok_or(Blocker::UnknownLimit)?;
    if (f64::from(total) - limit).abs() >= 0.5 {
        return Err(Blocker::Total { total, limit });
    }
    if let Some(next) = next_change(updated, now) {
        return Err(Blocker::Cooldown(next));
    }
    Ok(fields
        .iter()
        .map(|(shard, text)| {
            let cpu = parse_limit(text).map_or(0, u64::from);
            (shard.clone(), cpu)
        })
        .collect())
}

/// What the form tells about `blocker` under the Save button; `None` for the cooldown, which
/// the line of the next allowed change tells.
fn blocker_text(blocker: &Blocker) -> Option<String> {
    match blocker {
        Blocker::NotANumber(shard) => Some(format!("Enter a whole number of CPU for {shard}.")),
        Blocker::UnknownLimit => Some("The server does not report your CPU limit.".to_owned()),
        Blocker::Total { total, limit } => Some(format!(
            "The shards get {total} CPU in total. Give them exactly {limit} CPU."
        )),
        Blocker::Cooldown(_) => None,
    }
}

/// The text of the field of `shard` among the field texts `values`, by shard: an absent entry
/// reads as 0.
fn field_text(values: &BTreeMap<String, String>, shard: &str) -> String {
    values.get(shard).cloned().unwrap_or_else(|| "0".to_owned())
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
/// the CPU. `reload` fetches the profile `me` again after a change.
#[component]
pub fn CpuShards(
    overseer: Overseer,
    me: RwSignal<Option<Result<Arc<Profile>, String>>>,
    reload: Callback<()>,
) -> impl IntoView {
    if !overseer.world().supports(Subsystem::CpuShards) {
        return ().into_any();
    }
    let world_shards = overseer.world().shards().to_vec();
    let profile = move || me.get().and_then(Result::ok);
    // The server's shards, then the shards that only the allocation names.
    let shards = Memo::new(move |_| {
        let mut shards = world_shards.clone();
        if let Some(me) = profile() {
            for shard in me.cpu_shards.keys() {
                if !shards.contains(shard) {
                    shards.push(shard.clone());
                }
            }
        }
        shards
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
    let limit = move || profile().and_then(|me| me.cpu);
    let updated = move || profile().and_then(|me| me.cpu_shards_updated);
    let check = Memo::new(move |_| allocation(&fields(), limit(), updated(), now.get()));
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
                Ok(()) => {
                    saved.set(Some("CPU allocation saved.".to_owned()));
                    reload.run(());
                }
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
            Err(Blocker::Total { .. } | Blocker::NotANumber(_))
        ) {
            TOTAL_WRONG
        } else {
            TOTAL
        }
    };
    let next_line = move || match (next_change(updated(), now.get()), updated()) {
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
                    "Give each shard a part of your CPU. The parts must total your CPU limit. \
                    After a change, the server allows the next change after 12 hours."
                </p>
                <div class=FIELDS>
                    <For each=move || shards.get() key=Clone::clone let(shard)>
                        <ShardField shard values />
                    </For>
                </div>
                <p class=total_class aria-live="polite">{total_line}</p>
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

/// The number field of one shard's CPU limit, bound to its entry of `values` (absent reads
/// as 0).
#[component]
fn ShardField(shard: String, values: RwSignal<BTreeMap<String, String>>) -> impl IntoView {
    let id = format!("cpu-shard-{shard}");
    let value = {
        let shard = shard.clone();
        move || values.with(|values| field_text(values, &shard))
    };
    let invalid = {
        let value = value.clone();
        move || parse_limit(&value()).is_none().then_some("true")
    };
    let on_input = {
        let shard = shard.clone();
        move |event| {
            let text = event_target_value(&event);
            values.update(|values| {
                values.insert(shard.clone(), text);
            });
        }
    };
    view! {
        <div class=style::FIELD>
            <label for=id.clone() class=style::LABEL>{shard}</label>
            <input
                id=id
                class=INPUT
                type="text"
                inputmode="numeric"
                autocomplete="off"
                prop:value=value
                aria-invalid=invalid
                on:input=on_input
            />
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::{Blocker, COOLDOWN_MS, allocation, field_text, next_change, total};

    /// The fields of `pairs` (shard, text).
    fn fields(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(shard, text)| ((*shard).to_owned(), (*text).to_owned()))
            .collect()
    }

    /// One hour, in milliseconds.
    const HOUR: f64 = 60.0 * 60.0 * 1000.0;

    #[test]
    fn a_shard_without_a_value_reads_as_zero() {
        let values = [("shard0".to_owned(), "20".to_owned())]
            .into_iter()
            .collect();
        assert_eq!(field_text(&values, "shard0"), "20");
        assert_eq!(field_text(&values, "shard1"), "0");
    }

    #[test]
    fn totals_read_blank_fields_as_zero_and_name_a_wrong_field() {
        assert_eq!(
            total(&fields(&[("shard0", "20"), ("shard1", " 5 ")])),
            Ok(25)
        );
        assert_eq!(total(&fields(&[("shard0", ""), ("shard1", "0")])), Ok(0));
        assert_eq!(
            total(&fields(&[
                ("shard0", "20"),
                ("shard1", "2.5"),
                ("shard2", "x")
            ])),
            Err("shard1".to_owned())
        );
        assert_eq!(
            total(&fields(&[("shard0", "-1")])),
            Err("shard0".to_owned())
        );
    }

    #[test]
    fn the_next_change_comes_12_hours_after_the_previous_one() {
        let now = 100.0 * HOUR;
        assert_eq!(next_change(None, now), None);
        assert_eq!(next_change(Some(now - HOUR), now), Some(now + 11.0 * HOUR));
        assert_eq!(next_change(Some(now - COOLDOWN_MS), now), None);
        assert_eq!(next_change(Some(now - 13.0 * HOUR), now), None);
    }

    #[test]
    fn an_allocation_saves_only_when_it_totals_the_cpu_after_the_cooldown() {
        let now = 100.0 * HOUR;
        let even = fields(&[("shard0", "20"), ("shard1", ""), ("shard3", "80")]);
        let expected = [
            ("shard0".to_owned(), 20),
            ("shard1".to_owned(), 0),
            ("shard3".to_owned(), 80),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            allocation(&even, Some(100.0), Some(now - 13.0 * HOUR), now),
            Ok(expected)
        );
        assert!(allocation(&even, Some(100.0), None, now).is_ok());
        assert_eq!(
            allocation(&even, Some(120.0), None, now),
            Err(Blocker::Total {
                total: 100,
                limit: 120.0
            })
        );
        assert_eq!(
            allocation(&even, None, None, now),
            Err(Blocker::UnknownLimit)
        );
        assert_eq!(
            allocation(&even, Some(100.0), Some(now - HOUR), now),
            Err(Blocker::Cooldown(now + 11.0 * HOUR))
        );
        let wrong = fields(&[("shard0", "20"), ("shard3", "eighty")]);
        assert_eq!(
            allocation(&wrong, Some(100.0), None, now),
            Err(Blocker::NotANumber("shard3".to_owned()))
        );
    }
}
