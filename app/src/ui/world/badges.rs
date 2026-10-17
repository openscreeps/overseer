//! Player badges for the viewers of a screen ([`crate::badges`]): one loader for each world
//! screen or room window ([`open`]), kept in [`MapState::badges`]. Every viewer of the screen
//! shows what the loader gets ([`show_on`]): the map draws owners' badges on rooms, and every
//! viewer draws a creep's owner's badge on the creep. The players of the live rooms come first
//! ([`follow`]), then the owners and reservers of the rooms in view while the map draws their
//! badges ([`BadgeWants`]).

use leptos::prelude::*;
use overseer::Overseer;
use overseer::map::{DetailState, RoomRect, WorldEvent};
use overseer_view::Viewer;

use super::MapState;
use crate::badges::{BADGE_SIZE, Badges, room_wants, wanted};
use crate::profiles::Server;
use crate::ui::surface::Surface;

/// Opens the badge loader of `server` (`overseer` connected to it) into `map.badges` for the
/// screen being built. The loader stops when the screen goes.
pub fn open(map: MapState, overseer: &Overseer, server: &Server) {
    let loader = Badges::new(overseer.clone(), &server.url);
    map.badges.set_value(Some(loader.clone()));
    let own = StoredValue::new_local(loader);
    on_cleanup(move || {
        let Some(loader) = own.try_get_value() else {
            return;
        };
        loader.stop();
        // A screen that took over keeps its own loader.
        if map
            .badges
            .try_with_value(|held| held.as_ref().is_some_and(|held| held.is(&loader)))
            == Some(true)
        {
            map.badges.set_value(None);
        }
    });
}

/// Shows the badges of the screen's loader on the viewer of `surface` once the viewer exists,
/// while the calling component lives.
pub fn show_on(map: MapState, surface: Surface) {
    let attached = StoredValue::new_local(None::<(Badges, u64)>);
    Effect::new(move |_| {
        if !surface.ready.get() || attached.with_value(Option::is_some) {
            return;
        }
        let Some(loader) = map.badges.get_value() else {
            return;
        };
        let id = loader.attach(move |user, pixels| {
            surface.with_viewer(|viewer| viewer.set_badge(user, pixels, BADGE_SIZE));
        });
        attached.set_value(Some((loader, id)));
    });
    on_cleanup(move || {
        if let Some(Some((loader, id))) = attached.try_get_value() {
            loader.detach(id);
        }
    });
}

/// Asks the screen's loader for the badges of the players of a live room (`event` comes from
/// a feed subscription). A room that is no longer live drops them.
pub fn follow(map: MapState, event: &WorldEvent) {
    map.badges.with_value(|loader| {
        let Some(loader) = loader else {
            return;
        };
        match event {
            WorldEvent::Detail(room, DetailState::Live(snapshot)) => {
                loader.want_room(*room, room_wants(&snapshot.users));
            }
            WorldEvent::Detail(room, DetailState::Refused(_)) | WorldEvent::DetailGone(room) => {
                loader.forget_room(*room);
            }
            _ => {}
        }
    });
}

/// Asks the screen's loader for the badges of the owners and reservers in view, nearest the
/// centre first, whenever the map's interest goes to the feed (one camera change drives both)
/// and whenever what the world knows changes while badges are drawn; for none while badges
/// are too small to be drawn.
#[derive(Clone, Copy)]
pub struct BadgeWants {
    /// The rooms in view and whether badges are drawn, as the feed was last told.
    view: RwSignal<Option<(RoomRect, bool)>>,
}

impl BadgeWants {
    /// The badges of the rooms in view of the map of `map`, from what its world knows.
    pub fn new(map: MapState) -> Self {
        let wants = Self {
            view: RwSignal::new(None),
        };
        Effect::new(move |_| {
            let Some((rooms, visible)) = wants.view.get() else {
                return;
            };
            let in_view = if visible {
                map.knowledge(|knowledge| {
                    let stats = knowledge.stats_in(rooms);
                    wanted(stats.iter().map(|(room, stat)| (*room, &**stat)), rooms)
                })
                .unwrap_or_default()
            } else {
                Vec::new()
            };
            map.badges.with_value(|loader| {
                if let Some(loader) = loader {
                    loader.want(in_view);
                }
            });
        });
        wants
    }

    /// The feed was told about the view of `viewer`: the badges follow it.
    pub fn follow(self, viewer: &Viewer) {
        let view = Some((viewer.visible_rooms(), viewer.badges_visible()));
        if self
            .view
            .try_get_untracked()
            .is_some_and(|before| before != view)
        {
            self.view.set(view);
        }
    }
}
