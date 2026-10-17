//! Players: the public profile page of a player (badge, name, levels and rooms, route
//! [`Route::User`]), the "Find player" search ([`FindPlayer`]), the rooms list of a player
//! ([`RoomList`], [`OwnRooms`]), and [`PlayerLink`]: a player's name as a link to the page.

mod rooms;
mod search;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use leptos::prelude::*;
use overseer::Overseer;
use overseer::model::users::PublicProfile;

pub use rooms::{OwnRooms, RoomList};
pub use search::FindPlayer;

use crate::levels;
use crate::route::Route;
use crate::session;
use crate::state::AppState;
use crate::ui::icon::{Icon, IconKind};
use crate::ui::profile::{
    BADGE, BADGE_SKELETON, HEAD, LevelBar, STAT_NAME, STAT_VALUE, STATS, open_map,
};
use crate::ui::spawn_scoped;
use crate::ui::style::{self, concatcp};

/// The user ids of the game's non-player characters: Invader and Source Keeper.
const NPC_IDS: [&str; 2] = ["2", "3"];

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

/// Whether user `id` named `name` has a profile page: not for the NPCs, and not for a user
/// whose name is unknown (`name` is the id).
fn has_page(id: &str, name: &str) -> bool {
    !NPC_IDS.contains(&id) && !name.trim().is_empty() && name != id
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

/// Whether the server's error `message` of a profile lookup says that no such user exists
/// (`user not found`; community servers fail with a `TypeError` instead).
fn is_unknown_user(message: &str) -> bool {
    message.contains("not found") || message.starts_with("TypeError")
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
                    Err(error) => {
                        let message = session::error_message(&error);
                        if is_unknown_user(&message) {
                            Lookup::Unknown
                        } else {
                            Lookup::Failed(message)
                        }
                    }
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
                <div class=HEAD role="status">
                    <span class=BADGE_SKELETON aria-hidden="true"></span>
                    <span class=style::SKELETON_LINE_WIDE aria-hidden="true"></span>
                    <span class="sr-only">"Loading the player…"</span>
                </div>
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
    let username = username.unwrap_or(id);
    // `None` while it loads; `Some(None)` when the server draws none.
    let badge = RwSignal::new(None::<Option<String>>);
    {
        let username = username.clone();
        spawn_scoped(async move {
            let svg = overseer.users().badge_svg(&username, false).await;
            badge.set(Some(svg.ok().map(|svg| {
                format!("data:image/svg+xml;base64,{}", STANDARD.encode(svg))
            })));
        });
    }
    let alt = format!("Badge of {username}");
    let gcl = levels::gcl(gcl.unwrap_or(0.0));
    let power = levels::power(power.unwrap_or(0.0));
    let badge = move || match badge.get() {
        None => view! { <span class=BADGE_SKELETON aria-hidden="true"></span> }.into_any(),
        Some(None) => view! { <span class=BADGE aria-hidden="true"></span> }.into_any(),
        Some(Some(src)) => {
            view! { <img class=BADGE src=src alt=alt.clone() width="64" height="64" /> }.into_any()
        }
    };
    view! {
        <section class=style::PANEL aria-labelledby="player-title">
            <div class=HEAD>
                {badge}
                <div class="min-w-0">
                    <h1 id="player-title" class="text-xl font-semibold leading-tight wrap-anywhere">
                        {username}
                    </h1>
                    {server.map(|server| view! { <p class="mt-1 text-muted">{server}</p> })}
                </div>
            </div>
            <dl class=STATS>
                <dt class=STAT_NAME>"Global Control Level"</dt>
                <dd class=STAT_VALUE>
                    <LevelBar label="GCL" level=gcl />
                </dd>
                <dt class=STAT_NAME>"Power level"</dt>
                <dd class=STAT_VALUE>
                    <LevelBar label="Power level" level=power />
                </dd>
            </dl>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::{has_page, is_unknown_user};

    #[test]
    fn npcs_and_unknown_names_have_no_page() {
        assert!(has_page("5a1b", "Tigga"));
        assert!(!has_page("2", "Invader"));
        assert!(!has_page("3", "Source Keeper"));
        assert!(!has_page("5a1b", "5a1b"), "the room does not know the name");
        assert!(!has_page("5a1b", " "));
    }

    #[test]
    fn unknown_users() {
        assert!(is_unknown_user("user not found"));
        assert!(is_unknown_user(
            "TypeError: Cannot read properties of null (reading '_id')"
        ));
        assert!(!is_unknown_user("HTTP transport failed"));
        assert!(!is_unknown_user("unauthorized"));
    }
}
