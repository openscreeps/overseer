//! The room screen: room and shard picker, the `overseer-view` canvas and the objects of the
//! selected tile.

mod canvas;

use std::sync::Arc;

use leptos::prelude::*;
use overseer::RoomSnapshot;
use overseer::core::api::types::GameRoomObject;

use crate::state::{AppState, Screen};
use crate::ui::spawn_scoped;

/// A room on a shard (`None` on single-world servers).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomTarget {
    /// Shard name.
    pub shard: Option<String>,
    /// Room name, upper-case.
    pub room: String,
}

impl RoomTarget {
    /// Parses `shard/room` or `room` (a saved target).
    fn parse(saved: &str) -> Option<Self> {
        let (shard, room) = match saved.split_once('/') {
            Some((shard, room)) => (Some(shard.to_owned()), room),
            None => (None, saved),
        };
        let room = room_name(room)?;
        Some(Self { shard, room })
    }

    /// `shard/room` or `room`.
    fn label(&self) -> String {
        self.shard.as_ref().map_or_else(
            || self.room.clone(),
            |shard| format!("{shard}/{}", self.room),
        )
    }
}

/// `input` as a room name (`W8N3`, `E0S12`, upper-cased), if it is one.
fn room_name(input: &str) -> Option<String> {
    let name = input.trim().to_ascii_uppercase();
    let rest = name.strip_prefix(['W', 'E'])?;
    let split = rest.find(['N', 'S'])?;
    let (x, y) = (&rest[..split], &rest[split + 1..]);
    let digits = |part: &str| {
        !part.is_empty() && part.len() <= 3 && part.bytes().all(|b| b.is_ascii_digit())
    };
    (digits(x) && digits(y)).then_some(name)
}

/// The room screen.
#[component]
pub fn Room() -> impl IntoView {
    let state = AppState::expect();
    let Some(session) = state.session.get_untracked() else {
        return view! {
            <section class="panel">
                <p>"You are not signed in."</p>
                <button type="button" on:click=move |_| state.screen.set(Screen::Servers)>
                    "Servers"
                </button>
            </section>
        }
        .into_any();
    };
    let overseer = session.overseer;
    let shards: Vec<String> = overseer.world().shards().to_vec();
    let default_shard = overseer.world().shard().map(str::to_owned);
    let target = RwSignal::new(
        state
            .session_server()
            .and_then(|server| server.room)
            .and_then(|saved| RoomTarget::parse(&saved)),
    );
    if target.get_untracked().is_none() {
        let overseer = overseer.clone();
        spawn_scoped(async move {
            if let Ok(Some(start)) = overseer.account().start_room(None).await {
                target.set(Some(RoomTarget {
                    shard: start.shard,
                    room: start.room,
                }));
            }
        });
    }
    let snapshot = RwSignal::new(None::<Arc<RoomSnapshot>>);
    let selected = RwSignal::new(None::<(u8, u8)>);
    view! {
        <div class="room-screen">
            <RoomPicker target shards default_shard />
            <canvas::RoomCanvas overseer=overseer.clone() target snapshot selected />
            <Selection snapshot selected />
        </div>
    }
    .into_any()
}

/// Shard and room inputs.
#[component]
fn RoomPicker(
    target: RwSignal<Option<RoomTarget>>,
    shards: Vec<String>,
    default_shard: Option<String>,
) -> impl IntoView {
    let state = AppState::expect();
    let room = RwSignal::new(String::new());
    let shard = RwSignal::new(default_shard.unwrap_or_default());
    let error = RwSignal::new(None::<String>);
    // Follow the shown room (the default start room arrives later).
    Effect::new(move |_| {
        if let Some(current) = target.get() {
            room.set(current.room.clone());
            if let Some(name) = current.shard {
                shard.set(name);
            }
        }
    });
    let sharded = !shards.is_empty();
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let Some(name) = room_name(&room.get_untracked()) else {
            error.set(Some("Enter a room name such as W8N3".to_owned()));
            return;
        };
        error.set(None);
        let next = RoomTarget {
            shard: sharded.then(|| shard.get_untracked()),
            room: name,
        };
        let saved = next.label();
        if let Some(server) = state.session_server() {
            state.update_profiles(|profiles| {
                if let Some(server) = profiles.server_mut(&server.url) {
                    server.room = Some(saved);
                }
            });
        }
        target.set(Some(next));
    };
    let shard_options = shards
        .into_iter()
        .map(|name| view! { <option value=name.clone()>{name.clone()}</option> })
        .collect_view();
    view! {
        <form class="room-picker" on:submit=submit novalidate aria-label="Room">
            {sharded
                .then(|| {
                    view! {
                        <label for="room-shard">"Shard"</label>
                        <select id="room-shard" bind:value=shard>
                            {shard_options}
                        </select>
                    }
                })}
            <label for="room-name">"Room"</label>
            <input
                id="room-name"
                type="text"
                size="8"
                autocomplete="off"
                spellcheck="false"
                aria-invalid=move || error.with(Option::is_some).to_string()
                aria-describedby="room-name-error"
                bind:value=room
            />
            <button type="submit" class="primary">"Show"</button>
            <span id="room-name-error" class="error" role="alert">{move || error.get()}</span>
        </form>
    }
}

/// The objects on the selected tile.
#[component]
fn Selection(
    snapshot: RwSignal<Option<Arc<RoomSnapshot>>>,
    selected: RwSignal<Option<(u8, u8)>>,
) -> impl IntoView {
    let content = move || {
        let Some((x, y)) = selected.get() else {
            return view! { <p class="muted">"Click a tile to list its objects."</p> }.into_any();
        };
        let Some(snapshot) = snapshot.get() else {
            return view! { <p class="muted">"Waiting for the room…"</p> }.into_any();
        };
        let objects: Vec<&GameRoomObject> = snapshot
            .objects
            .iter()
            .filter(|object| object.x() == u64::from(x) && object.y() == u64::from(y))
            .collect();
        let count = match objects.len() {
            0 => "No objects".to_owned(),
            1 => "1 object".to_owned(),
            n => format!("{n} objects"),
        };
        let items = objects
            .into_iter()
            .map(|object| {
                let owner = object.user().map(|user| {
                    snapshot
                        .users
                        .get(user)
                        .map_or_else(|| user.to_owned(), |known| known.username.clone())
                });
                let json = serde_json::to_string_pretty(object).unwrap_or_default();
                let title = format!("{} at {x},{y}", object.kind());
                view! {
                    <li class="object">
                        <strong>{title}</strong>
                        <span class="muted">
                            {owner.map_or_else(|| "no owner".to_owned(), |owner| format!("owner {owner}"))}
                        </span>
                        <details>
                            <summary>{format!("JSON of {}", object.id())}</summary>
                            <pre>{json}</pre>
                        </details>
                    </li>
                }
            })
            .collect_view();
        view! {
            <h2 id="selection-title">{format!("Tile {x},{y}")}</h2>
            <p role="status">{count}</p>
            <ul class="object-list">{items}</ul>
        }
        .into_any()
    };
    view! {
        <aside class="panel selection" aria-label="Selected tile">
            {content}
        </aside>
    }
}
