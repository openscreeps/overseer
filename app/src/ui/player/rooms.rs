//! The rooms whose controller a player owns (`Users::rooms`), by shard on sharded servers.
//! Each room is a link that opens the world map on it.

use leptos::prelude::*;
use overseer::Overseer;
use overseer::model::users::UserRoom;

use super::FindPlayer;
use crate::route::{Route, Shard, parse_room};
use crate::session;
use crate::state::AppState;
use crate::ui::icon::{Icon, IconKind};
use crate::ui::spawn_scoped;
use crate::ui::style;

/// The rooms of one shard, wrapped.
const LIST: &str = "flex flex-wrap gap-2";
/// A room: a chip that opens the map on it, row-high (a finger's size on a coarse pointer).
const ROOM: &str = "inline-flex items-center gap-2 min-h-row px-3 font-mono text-sm tabular-nums \
    text-fg bg-panel-2 border border-edge rounded-full transition-colors \
    duration-(--duration-fast) ease-standard hover:bg-panel-3 hover:border-edge-strong";
/// The name of a shard over its rooms.
const SHARD: &str = "mt-3 first:mt-0 mb-2 text-sm font-semibold text-muted";
/// The part under the own rooms with the search for other players, set off by a rule.
const OWN_SEARCH: &str = "mt-5 pt-4 border-t border-edge";

/// The rooms, grouped by shard: the shards by name (`None`, a single world, first), the rooms
/// of each shard by name, each once.
pub fn by_shard(mut rooms: Vec<UserRoom>) -> Vec<(Option<String>, Vec<String>)> {
    rooms.sort_unstable();
    rooms.dedup();
    let mut groups: Vec<(Option<String>, Vec<String>)> = Vec::new();
    for UserRoom { shard, room } in rooms {
        match groups.last_mut() {
            Some((last, names)) if *last == shard => names.push(room),
            _ => groups.push((shard, vec![room])),
        }
    }
    groups
}

/// The rooms of user `user_id`, by shard on sharded servers; `empty` shows when the user owns
/// none. A failure shows with a button to load again.
#[component]
pub fn RoomList(
    overseer: Overseer,
    /// The user id.
    #[prop(into)]
    user_id: String,
    /// The message when the user owns no room.
    empty: &'static str,
) -> impl IntoView {
    let state = AppState::expect();
    let rooms = RwSignal::new(None::<Result<Vec<(Option<String>, Vec<String>)>, String>>);
    let load = move || {
        let overseer = overseer.clone();
        let user_id = user_id.clone();
        rooms.set(None);
        spawn_scoped(async move {
            let result = overseer.users().rooms(&user_id).await;
            rooms.set(Some(
                result
                    .map(by_shard)
                    .map_err(|error| session::error_message(&error)),
            ));
        });
    };
    load();
    let load = StoredValue::new_local(load);
    let server = state
        .route
        .with_untracked(|route| route.server().map(str::to_owned));
    move || match rooms.get() {
        None => view! {
            <p role="status">
                <span class=style::SKELETON_LINE_WIDE aria-hidden="true"></span>
                <span class="sr-only">"Loading the rooms…"</span>
            </p>
        }
        .into_any(),
        Some(Err(error)) => view! {
            <div class="flex flex-wrap items-center gap-3" role="alert">
                <p class="min-w-0 text-error wrap-anywhere">{error}</p>
                <button
                    type="button"
                    class=style::BUTTON
                    on:click=move |_| load.with_value(|load| load())
                >
                    <Icon kind=IconKind::Refresh />
                    "Try again"
                </button>
            </div>
        }
        .into_any(),
        Some(Ok(groups)) if groups.is_empty() => view! {
            <p class=style::EMPTY_STATE>
                <Icon kind=IconKind::Room class=style::EMPTY_ICON />
                {empty}
            </p>
        }
        .into_any(),
        Some(Ok(groups)) => {
            let sharded = groups.iter().any(|(shard, _)| shard.is_some());
            groups
                .into_iter()
                .map(|(shard, names)| {
                    let label = shard
                        .clone()
                        .map_or_else(|| "Rooms".to_owned(), |shard| format!("Rooms on {shard}"));
                    let heading = shard
                        .clone()
                        .filter(|_| sharded)
                        .map(|shard| view! { <h3 class=SHARD>{shard}</h3> });
                    let items = names
                        .into_iter()
                        .map(|name| room_item(server.as_deref(), shard.as_deref(), name))
                        .collect_view();
                    view! {
                        {heading}
                        <ul class=LIST aria-label=label>{items}</ul>
                    }
                })
                .collect_view()
                .into_any()
        }
    }
}

/// A room of the list: a link that opens the map of server `server` on room `name` of
/// `shard`, or the name alone when it is not a world room.
fn room_item(server: Option<&str>, shard: Option<&str>, name: String) -> impl IntoView + use<> {
    let href = server.zip(parse_room(&name)).map(|(server, room)| {
        Route::World {
            server: server.to_owned(),
            shard: Some(Shard::of(shard)),
            room: Some(room),
        }
        .hash()
    });
    let item = match href {
        Some(href) => view! {
            <a class=ROOM href=href>
                <Icon kind=IconKind::Room />
                {name}
            </a>
        }
        .into_any(),
        None => view! { <span class="font-mono text-sm">{name}</span> }.into_any(),
    };
    view! { <li>{item}</li> }
}

/// The signed-in user's rooms, with the search for other players, for the profile page.
#[component]
pub fn OwnRooms(overseer: Overseer) -> impl IntoView {
    let user_id = overseer.user_id().map(str::to_owned);
    user_id.map(|user_id| {
        view! {
            <section class=style::PANEL aria-labelledby="own-rooms-title">
                <h2 id="own-rooms-title" class=style::H2>"Your rooms"</h2>
                <RoomList overseer user_id empty="You own no room yet." />
                <div class=OWN_SEARCH>
                    <FindPlayer />
                </div>
            </section>
        }
    })
}

#[cfg(test)]
mod tests {
    use overseer::model::users::UserRoom;

    use super::by_shard;

    fn room(shard: Option<&str>, room: &str) -> UserRoom {
        UserRoom {
            shard: shard.map(str::to_owned),
            room: room.to_owned(),
        }
    }

    #[test]
    fn groups_rooms_by_shard() {
        let rooms = vec![
            room(Some("shard2"), "W1N1"),
            room(Some("shard0"), "E5S5"),
            room(Some("shard2"), "E1N1"),
            room(Some("shard0"), "E5S5"),
        ];
        assert_eq!(
            by_shard(rooms),
            [
                (Some("shard0".to_owned()), vec!["E5S5".to_owned()]),
                (
                    Some("shard2".to_owned()),
                    vec!["E1N1".to_owned(), "W1N1".to_owned()]
                ),
            ],
            "shards and rooms by name, each room once"
        );
    }

    #[test]
    fn a_single_world_is_one_group() {
        assert_eq!(
            by_shard(vec![room(None, "W8N3"), room(None, "W2N1")]),
            [(None::<String>, vec!["W2N1".to_owned(), "W8N3".to_owned()])]
        );
        assert_eq!(by_shard(Vec::new()), Vec::new());
    }
}
