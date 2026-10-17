//! Players: the public profile page of a player (badge, name, levels and rooms, route
//! [`Route::User`]), the "Find player" search ([`FindPlayer`]), the rooms list of a player
//! ([`RoomList`], [`OwnRooms`]), the summary that the page shares with the own profile
//! ([`summary`]), and [`PlayerLink`]: a player's name as a link to the page.

mod rooms;
mod search;
pub mod summary;

use leptos::prelude::*;
use overseer::Overseer;
use overseer::model::users::PublicProfile;

pub use rooms::{OwnRooms, RoomList};
pub use search::FindPlayer;

use crate::route::Route;
use crate::session;
use crate::state::AppState;
use crate::ui::icon::{Icon, IconKind};
use crate::ui::spawn_scoped;
use crate::ui::style::{self, concatcp};
use summary::{HeadSkeleton, LevelRows, STATS, SummaryHead, load_badge, open_map};

/// The game's non-player characters: user id and name.
const NPCS: [(&str, &str); 2] = [("2", "Invader"), ("3", "Source Keeper")];

/// A name that links to the player's page: the text keeps its colour, the underline marks
/// the link. A finger gets a row-high target.
const LINK: &str = "underline underline-offset-2 decoration-from-font hover:decoration-2 \
    cursor-pointer pointer-coarse:inline-flex pointer-coarse:items-center \
    pointer-coarse:min-h-control-sm";
/// A [`LINK`] to the signed-in user's own page.
const LINK_OWN: &str = concatcp!(LINK, " text-ok");
/// The bar over the page: the way back and the search, wrapped on narrow screens.
const TOOLBAR: &str = "flex flex-wrap items-end justify-between gap-3";
/// A whole-panel state (not found, failure).
const STATE: &str = concatcp!(style::PANEL, " ", style::STATE_CARD);

/// A player as a room names it: the user id and the name to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Player {
    /// The user id.
    pub id: String,
    /// The username; the id itself when the room does not know the user.
    pub name: String,
}

/// The name of the non-player character with user id `id`; `None` for a player.
pub fn npc_name(id: &str) -> Option<&'static str> {
    NPCS.iter()
        .find(|(npc, _)| *npc == id)
        .map(|(_, name)| *name)
}

/// Whether user `id` named `name` has a profile page: not for the NPCs, and not for a user
/// whose name is unknown (`name` is the id).
fn has_page(id: &str, name: &str) -> bool {
    npc_name(id).is_none() && !name.trim().is_empty() && name != id
}

/// The name of `player`: a link to the player's page ([`Route::User`]). NPCs, unknown names
/// and popped-out windows show the name as plain text.
#[component]
pub fn PlayerLink(
    /// The player.
    player: Player,
    /// The signed-in user: the name shows in the "own" colour.
    #[prop(optional)]
    own: bool,
) -> impl IntoView {
    let Player { id, name } = player;
    let state = AppState::expect();
    let href = state
        .route
        .with_untracked(|route| route.server().map(str::to_owned))
        .filter(|_| !state.is_popout() && has_page(&id, &name))
        .map(|server| {
            Route::User {
                server,
                name: name.clone(),
            }
            .hash()
        });
    match href {
        Some(href) => {
            view! { <a class=if own { LINK_OWN } else { LINK } href=href>{name}</a> }.into_any()
        }
        None => view! { <span class=if own { "text-ok" } else { "" }>{name}</span> }.into_any(),
    }
}

/// What the lookup of a player gave.
#[derive(Clone, Debug)]
enum Lookup {
    /// The player's profile.
    Found(Box<PublicProfile>),
    /// No player has the name.
    Unknown,
    /// The lookup failed, with the message to show.
    Failed(String),
}

/// The page of the player named `name`: the way back to the map, the search, the player's
/// badge, name and levels, and the player's rooms.
#[component]
pub fn PlayerPage(
    /// The player's name, as the route gives it.
    name: String,
) -> impl IntoView {
    let state = AppState::expect();
    let Some(session) = state.session.get_untracked() else {
        return ().into_any();
    };
    let overseer = session.overseer;
    let lookup = RwSignal::new(None::<Lookup>);
    let load = {
        let overseer = overseer.clone();
        let name = name.clone();
        move || {
            let overseer = overseer.clone();
            let name = name.clone();
            lookup.set(None);
            spawn_scoped(async move {
                lookup.set(Some(match overseer.users().find(&name).await {
                    Ok(profile) => Lookup::Found(Box::new(profile)),
                    Err(error) if session::is_unknown_user(&error) => Lookup::Unknown,
                    Err(error) => Lookup::Failed(session::error_message(&error)),
                }));
            });
        }
    };
    load();
    let load = StoredValue::new_local(load);
    let server = state.session_server().map(|server| server.name);
    let body = move || match lookup.get() {
        None => view! {
            <section class=style::PANEL>
                <HeadSkeleton label="Loading the player…" />
            </section>
        }
        .into_any(),
        Some(Lookup::Unknown) => view! {
            <section class=STATE role="alert">
                <span class=style::STATE_ICON_WARN>
                    <Icon kind=IconKind::User class=style::STATE_ICON_SVG />
                </span>
                <h1 class=style::STATE_TITLE>"Player not found"</h1>
                <p class=concatcp!(style::STATE_TEXT, " wrap-anywhere")>
                    {format!(
                        "No player is called “{name}”{}. Check the name, then search again.",
                        server.as_ref().map(|server| format!(" on {server}")).unwrap_or_default(),
                    )}
                </p>
            </section>
        }
        .into_any(),
        Some(Lookup::Failed(error)) => view! {
            <section class=STATE role="alert">
                <span class=style::STATE_ICON_ERROR>
                    <Icon kind=IconKind::Alert class=style::STATE_ICON_SVG />
                </span>
                <h1 class=style::STATE_TITLE>"The player did not load"</h1>
                <p class=concatcp!(style::STATE_TEXT, " wrap-anywhere")>{error}</p>
                <button
                    type="button"
                    class=style::BUTTON
                    on:click=move |_| load.with_value(|load| load())
                >
                    <Icon kind=IconKind::Refresh />
                    "Try again"
                </button>
            </section>
        }
        .into_any(),
        Some(Lookup::Found(profile)) => {
            let user_id = profile.id.clone();
            view! {
                <Summary overseer=overseer.clone() profile=*profile server=server.clone() />
                <section class=style::PANEL aria-labelledby="player-rooms-title">
                    <h2 id="player-rooms-title" class=style::H2>"Rooms"</h2>
                    <RoomList overseer=overseer.clone() user_id empty="This player owns no room." />
                </section>
            }
            .into_any()
        }
    };
    view! {
        <div class=style::PAGE>
            <div class=TOOLBAR>
                <button type="button" class=style::BUTTON on:click=move |_| open_map(state)>
                    <Icon kind=IconKind::ArrowLeft />
                    "Back to the map"
                </button>
                <FindPlayer />
            </div>
            {body}
        </div>
    }
    .into_any()
}

/// A player's badge, name, server and levels.
#[component]
fn Summary(overseer: Overseer, profile: PublicProfile, server: Option<String>) -> impl IntoView {
    let PublicProfile {
        id,
        username,
        gcl,
        power,
        ..
    } = profile;
    let name = username.unwrap_or(id);
    let badge = {
        let name = name.clone();
        load_badge(async move { overseer.users().badge_svg(&name, false).await })
    };
    view! {
        <section class=style::PANEL aria-labelledby="player-title">
            <SummaryHead badge name title_id="player-title" server />
            <dl class=STATS>
                <LevelRows gcl power />
            </dl>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::has_page;

    #[test]
    fn npcs_and_unknown_names_have_no_page() {
        assert!(has_page("5a1b", "Tigga"));
        assert!(!has_page("2", "Invader"));
        assert!(!has_page("3", "Source Keeper"));
        assert!(!has_page("5a1b", "5a1b"), "the room does not know the name");
        assert!(!has_page("5a1b", " "));
    }
}
