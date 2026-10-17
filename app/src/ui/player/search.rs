//! "Find player": a name field that opens the page of the player with that name. The page
//! tells when no player has the name.

use leptos::ev::SubmitEvent;
use leptos::prelude::*;

use crate::route::Route;
use crate::state::AppState;
use crate::ui::icon::{Icon, IconKind};
use crate::ui::style::{self, concatcp};

/// The search, at most as wide as a narrow panel: the field grows, the button keeps its width,
/// the error goes under both.
const FORM: &str = "flex flex-wrap items-end gap-2 min-w-0 w-full max-w-md";
/// The field: label over input, at least as wide as a long name.
const FIELD: &str = "flex flex-col gap-1 flex-1 min-w-48";
/// The name input, as wide as its field.
const INPUT: &str = concatcp!(style::INPUT, " w-full");
/// The error, on a line of its own.
const ERROR: &str = concatcp!(style::FIELD_ERROR, " basis-full");

/// The "Find player" field and its button. The field's element ids are fixed: show it once
/// per page.
#[component]
pub fn FindPlayer() -> impl IntoView {
    let state = AppState::expect();
    let query = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let submit = move |event: SubmitEvent| {
        event.prevent_default();
        let name = query.get_untracked().trim().to_owned();
        if name.is_empty() {
            error.set(Some("Enter the name of a player.".to_owned()));
            return;
        }
        let Some(server) = state
            .route
            .with_untracked(|route| route.server().map(str::to_owned))
        else {
            return;
        };
        error.set(None);
        query.set(String::new());
        state.navigate(Route::User { server, name });
    };
    view! {
        <form class=FORM role="search" aria-label="Find player" on:submit=submit novalidate>
            <div class=FIELD>
                <label for="find-player" class=style::LABEL>"Find player"</label>
                <input
                    id="find-player"
                    class=INPUT
                    type="search"
                    placeholder="Player name"
                    autocomplete="off"
                    autocapitalize="none"
                    spellcheck="false"
                    enterkeyhint="search"
                    aria-invalid=move || error.with(Option::is_some).to_string()
                    aria-describedby="find-player-error"
                    bind:value=query
                    on:input=move |_| error.set(None)
                />
            </div>
            <button type="submit" class=style::BUTTON>
                <Icon kind=IconKind::Search />
                "Find"
            </button>
            <p id="find-player-error" class=ERROR role="alert">
                {move || error.get()}
            </p>
        </form>
    }
}
