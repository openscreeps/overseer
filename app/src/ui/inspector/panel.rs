//! The inspector panel: the selected object's header, fields, tile mates and raw JSON.

use std::sync::Arc;

use leptos::prelude::*;
use overseer::RoomSnapshot;
use overseer::map::PortalDestination;

use super::colours::{part_class, resource_class};
use super::fields::inspect;
use super::{Context, Inspected, Value, number, title};
use crate::ui::style;
use crate::ui::style::concatcp;

/// What the inspector shows; recomputed with each room update, and an update that changes
/// nothing shown changes nothing on screen.
#[derive(Clone, Debug, PartialEq)]
enum Shown {
    /// No object is selected.
    Nothing,
    /// The room is not live: its objects are unknown.
    NotLive,
    /// The selected object left the room.
    Gone,
    /// The selected object.
    Object(Box<Detail>),
}

/// Which of [`Shown`] it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Nothing,
    NotLive,
    Gone,
    Object,
}

impl Shown {
    const fn state(&self) -> State {
        match self {
            Self::Nothing => State::Nothing,
            Self::NotLive => State::NotLive,
            Self::Gone => State::Gone,
            Self::Object(_) => State::Object,
        }
    }

    fn detail(&self) -> Option<&Detail> {
        match self {
            Self::Object(detail) => Some(detail),
            _ => None,
        }
    }
}

/// The selected object as shown.
#[derive(Clone, Debug, PartialEq)]
struct Detail {
    inspected: Inspected,
    /// Its tile (`12,34`).
    position: String,
    /// The tile's other objects: `(id, title)`.
    neighbours: Vec<(String, String)>,
    /// Its document, pretty-printed.
    json: String,
}

/// An empty inspector's message.
const EMPTY: &str = "flex flex-col items-center gap-2 p-4 text-sm text-muted text-center";

/// The selected object of the room, its tile's other objects, and its raw JSON.
///
/// While an object is selected the panel is built once and each part follows the room's
/// updates in place, so an opened "Raw JSON" stays open and focus stays where it is; a field
/// changes on screen only when its value does.
#[component]
pub fn Inspector(
    /// The room's live state.
    #[prop(into)]
    snapshot: Signal<Option<Arc<RoomSnapshot>>>,
    /// The selected object id.
    selected: RwSignal<Option<String>>,
    /// The signed-in user id.
    user: Option<String>,
    /// Selects another object (an object of the same tile).
    on_select: Callback<String>,
    /// Takes the user through a portal (its destination is a link; plain text without).
    #[prop(optional_no_strip)]
    go_to: Option<Callback<PortalDestination>>,
) -> impl IntoView {
    let user = StoredValue::new(user);
    let shown = Memo::new(move |_| {
        let Some(id) = selected.get() else {
            return Shown::Nothing;
        };
        snapshot.with(|snapshot| {
            let Some(snapshot) = snapshot else {
                return Shown::NotLive;
            };
            let Some(object) = snapshot.objects.iter().find(|object| object.id() == id) else {
                return Shown::Gone;
            };
            user.with_value(|me| {
                let context = Context {
                    game_time: snapshot.game_time,
                    me: me.as_deref(),
                    users: &snapshot.users,
                };
                let (x, y) = (object.x(), object.y());
                Shown::Object(Box::new(Detail {
                    inspected: inspect(object, &context),
                    position: format!("{x},{y}"),
                    neighbours: snapshot
                        .objects
                        .iter()
                        .filter(|other| other.x() == x && other.y() == y && other.id() != id)
                        .map(|other| (other.id().to_owned(), title(other)))
                        .collect(),
                    json: serde_json::to_string_pretty(object).unwrap_or_default(),
                }))
            })
        })
    });
    let state = Memo::new(move |_| shown.with(Shown::state));
    let content = move || match state.get() {
        State::Nothing => view! {
            <p class=EMPTY>"Select an object in the list or on the map."</p>
        }
        .into_any(),
        State::NotLive => view! {
            <p class=EMPTY>"The room is not live: its objects are unknown."</p>
        }
        .into_any(),
        State::Gone => view! { <p class=EMPTY role="status">"The object is gone."</p> }.into_any(),
        State::Object => detail_view(shown, on_select, go_to).into_any(),
    };
    // A message alone takes only its height; an object takes the room the list leaves.
    view! {
        <section
            class="flex-[1_1_55%] has-[>p]:flex-none min-h-0 overflow-auto p-3 bg-panel-2 border-t \
                border-edge"
            aria-label="Inspector"
        >
            {content}
        </section>
    }
}

/// The selected object's header, fields, tile mates and raw JSON, each part following
/// `shown` in place.
fn detail_view(
    shown: Memo<Shown>,
    on_select: Callback<String>,
    go_to: Option<Callback<PortalDestination>>,
) -> impl IntoView {
    let part = move |read: fn(&Detail) -> String| {
        Memo::new(move |_| shown.with(|shown| shown.detail().map(read).unwrap_or_default()))
    };
    let heading = part(|detail| detail.inspected.title.clone());
    let kind = part(|detail| detail.inspected.kind.clone());
    let position = part(|detail| detail.position.clone());
    let json = part(|detail| detail.json.clone());
    let owner = Memo::new(move |_| {
        shown.with(|shown| {
            let inspected = &shown.detail()?.inspected;
            Some((inspected.owner.clone()?, inspected.own))
        })
    });
    let neighbours = Memo::new(move |_| {
        shown.with(|shown| {
            shown
                .detail()
                .map(|detail| detail.neighbours.clone())
                .unwrap_or_default()
        })
    });
    let fields = Memo::new(move |_| {
        shown.with(|shown| {
            shown
                .detail()
                .map(|detail| detail.inspected.fields.clone())
                .unwrap_or_default()
        })
    });
    // Fields keyed by place and label: each keeps its elements while its value is unchanged.
    let field_keys = move || {
        fields.with(|fields| {
            fields
                .iter()
                .enumerate()
                .map(|(index, field)| (index, field.label))
                .collect::<Vec<_>>()
        })
    };
    let field = move |(index, label): (usize, &'static str)| {
        let value = Memo::new(move |_| {
            fields.with(|fields| {
                fields
                    .get(index)
                    .filter(|field| field.label == label)
                    .map(|field| field.value.clone())
            })
        });
        view! {
            <dt class="text-muted">{label}</dt>
            <dd class="min-w-0 tabular-nums wrap-anywhere">
                {move || value.get().map(|value| value_view(label, value, go_to))}
            </dd>
        }
    };
    // The signed-in user's own objects are marked by colour.
    let owner = move || {
        owner.get().map(|(owner, own)| {
            let class = if own { "text-ok" } else { "" };
            view! { <span class=class>{owner}</span> }
        })
    };
    let neighbours = move || {
        let neighbours = neighbours.get();
        (!neighbours.is_empty()).then(|| {
            let chips = neighbours
                .into_iter()
                .map(|(id, title)| {
                    view! {
                        <li>
                            <button type="button" class=CHIP on:click=move |_| on_select.run(id.clone())>
                                {title}
                            </button>
                        </li>
                    }
                })
                .collect_view();
            view! {
                <div class="flex flex-wrap items-center gap-2 mt-3 text-sm">
                    <span class="text-muted">"Also here"</span>
                    <ul class="flex flex-wrap gap-1">{chips}</ul>
                </div>
            }
        })
    };
    view! {
        <header>
            <h3 class="text-lg font-semibold leading-tight wrap-anywhere">{move || heading.get()}</h3>
            <p class="flex flex-wrap gap-x-3 gap-y-1 mt-1 mb-3 text-sm text-muted">
                <span>{move || kind.get()}</span>
                {owner}
                <span class="font-mono text-xs tabular-nums text-faint" title="Position">
                    {move || position.get()}
                </span>
            </p>
        </header>
        <dl class="grid grid-cols-[max-content_minmax(0,1fr)] gap-x-4 gap-y-2 items-baseline text-sm">
            <For each=field_keys key=|key| *key children=field />
        </dl>
        {neighbours}
        <details class="mt-3 text-sm">
            <summary class="text-muted cursor-pointer hover:text-fg">"Raw JSON"</summary>
            <pre class="max-h-72 mt-2 p-2 overflow-auto font-mono text-xs bg-sunken rounded-md">{move || json.get()}</pre>
        </details>
    }
}

/// A tile mate's chip: selects it.
const CHIP: &str = "inline-flex items-center justify-center gap-2 min-h-control-sm px-3 text-sm \
    font-normal leading-tight whitespace-nowrap text-fg bg-panel-2 border border-edge rounded-full \
    cursor-pointer transition-colors duration-(--duration-fast) ease-standard \
    enabled:hover:bg-panel-3 enabled:hover:border-edge-strong";

/// A field's value; a portal's destination is a link through it when `go_to` takes the user
/// there.
fn value_view(
    label: &'static str,
    value: Value,
    go_to: Option<Callback<PortalDestination>>,
) -> AnyView {
    match value {
        Value::Text(text) => view! { <span>{text}</span> }.into_any(),
        Value::Destination { text, to } => match go_to {
            Some(go_to) => {
                let name = format!("Go to {text}");
                view! {
                    <button
                        type="button"
                        class=concatcp!(style::LINK_BUTTON, " font-medium")
                        aria-label=name.clone()
                        title=name
                        on:click=move |_| go_to.run(to.clone())
                    >
                        {text}
                    </button>
                }
                .into_any()
            }
            None => view! { <span>{text}</span> }.into_any(),
        },
        Value::Meter { value, max, tone } => {
            let text = format!("{} / {}", number(value), number(max));
            view! {
                <span class=style::METER>
                    <progress
                        class=tone.class()
                        max=max.to_string()
                        value=value.min(max).to_string()
                        aria-label=label
                    ></progress>
                    <span class=style::METER_TEXT>{text}</span>
                </span>
            }
            .into_any()
        }
        Value::Store {
            resources,
            used,
            capacity,
        } => {
            let total = capacity.map_or_else(
                || number(used),
                |capacity| format!("{} / {}", number(used), number(capacity)),
            );
            let items = resources
                .into_iter()
                .map(|(resource, amount)| {
                    let class = resource_class(&resource);
                    view! {
                        <li class="flex items-center gap-2">
                            <span class=class aria-hidden="true"></span>
                            <span>{resource}</span>
                            <span class="ml-auto">{number(amount)}</span>
                        </li>
                    }
                })
                .collect_view();
            view! {
                <span class="flex flex-col gap-1">
                    <span class="text-muted">{total}</span>
                    <ul class="grid grid-cols-[repeat(auto-fill,minmax(7rem,1fr))] gap-x-3 gap-y-1">
                        {items}
                    </ul>
                </span>
            }
            .into_any()
        }
        Value::Body(parts) => {
            let count = parts.len();
            let items = parts
                .into_iter()
                .map(|part| {
                    let name = match &part.boost {
                        Some(boost) => format!("{} ({boost})", part.kind),
                        None => part.kind.clone(),
                    };
                    let class = part_class(&part.kind);
                    let title = name.clone();
                    view! {
                        <li
                            class=class
                            data-dead=!part.alive
                            data-boosted=part.boost.is_some()
                            title=title
                        >
                            <span class="sr-only">{name}</span>
                        </li>
                    }
                })
                .collect_view();
            view! {
                <span>
                    <ul class="flex flex-wrap gap-[3px]" aria-label=format!("{count} body parts")>
                        {items}
                    </ul>
                </span>
            }
            .into_any()
        }
    }
}
