//! Player badges on the world map and on creeps: which players' badges the viewers need
//! ([`wanted`]: owners and reservers of the rooms in view, nearest the centre first;
//! [`room_wants`]: the players of a live room, whose creeps carry their badge; [`merge`]:
//! both, live rooms first), and in browsers the loader (`Badges`, browser builds only) that
//! gets each badge's pixels to every viewer of the screen (`Viewer::set_badge`).
//!
//! A badge comes from the cache when it holds the player's badge with the same parameters
//! ([`badge_hash`], `terrain_cache::BadgeStore` in browser builds); otherwise it is fetched as SVG
//! (`Map::badge_svg`, `GET /api/user/badge-svg`), rasterized by the browser (SVG → image →
//! `OffscreenCanvas` of [`BADGE_SIZE`]² → premultiplied RGBA) and cached. A player whose badge
//! changes has another hash, so the new badge is fetched.
//!
//! # Pace
//!
//! One badge at a time, never blocking the map. Each request first asks the session's request
//! budget (`Client::budget_ready_at`, priority visible), shared with the world map: on
//! screeps.com (and its season) every request of the token counts against its limit of 120
//! per minute, and background loading together stays within 105 for visible work; private
//! servers, and a token whose rate limits are lifted, have no budget: requests follow each
//! other at once. Each player's badge is fetched at most once per world screen, failures
//! included.

use std::collections::{BTreeMap, HashSet};
use std::hash::{Hash, Hasher};

use overseer::map::{RoomCoord, RoomRect, RoomStat};
use overseer::model::account::UserBadge;
use overseer::model::world::RoomUser;

/// Side of a rasterized badge, in pixels: the viewer's badge size (no resampling there).
pub const BADGE_SIZE: u32 = overseer_view::BADGE_SIZE;
/// Bytes of a rasterized badge (RGBA).
pub const BADGE_BYTES: usize = (BADGE_SIZE * BADGE_SIZE * 4) as usize;
/// Most badges asked for at once (the nearest ones; the rest follow as the view moves).
const MAX_WANTED: usize = 256;

/// A stable 64-bit hash of a badge's parameters (FNV-1a of their JSON): the same badge hashes
/// the same in every session and build, a changed one differently.
pub fn badge_hash(badge: &UserBadge) -> u64 {
    let mut hasher = Fnv1a::default();
    serde_json::to_string(badge)
        .unwrap_or_default()
        .hash(&mut hasher);
    hasher.finish()
}

/// FNV-1a, 64 bits.
struct Fnv1a(u64);

impl Default for Fnv1a {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Hasher for Fnv1a {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

/// A badge the map needs.
#[derive(Clone, Debug, PartialEq)]
pub struct Want {
    /// The player's id (the viewer's badge key).
    pub user: String,
    /// The player's name (the badge request's).
    pub username: String,
    /// The badge's parameters hash ([`badge_hash`]).
    pub hash: u64,
}

/// The badges of the owners and reservers of `rooms` (with their overviews) in `view`,
/// nearest its centre first, each player once, at most [`MAX_WANTED`]. Players without a name
/// or badge are skipped.
pub fn wanted<'a>(
    rooms: impl IntoIterator<Item = (RoomCoord, &'a RoomStat)>,
    view: RoomRect,
) -> Vec<Want> {
    #[expect(clippy::cast_precision_loss, reason = "room coordinates are small")]
    let centre = (
        (view.min.x + view.max.x) as f32 / 2.0,
        (view.min.y + view.max.y) as f32 / 2.0,
    );
    let distance = |room: RoomCoord| {
        #[expect(clippy::cast_precision_loss, reason = "room coordinates are small")]
        let (dx, dy) = (room.x as f32 - centre.0, room.y as f32 - centre.1);
        dx.mul_add(dx, dy * dy)
    };
    let mut players: Vec<(f32, &str, &str, &UserBadge)> = Vec::new();
    for (room, stat) in rooms.into_iter().filter(|(room, _)| view.contains(*room)) {
        let owner = stat
            .owner
            .as_ref()
            .map(|owner| (&owner.id, &owner.username, &owner.badge));
        let reservation = stat
            .reservation
            .as_ref()
            .map(|reservation| (&reservation.id, &reservation.username, &reservation.badge));
        for (id, username, badge) in owner.into_iter().chain(reservation) {
            if let (Some(username), Some(badge)) = (username, badge) {
                players.push((distance(room), id, username, badge));
            }
        }
    }
    players.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut seen = HashSet::new();
    players
        .into_iter()
        .filter(|(_, id, _, _)| seen.insert(*id))
        .take(MAX_WANTED)
        .map(|(_, id, username, badge)| Want {
            user: id.to_owned(),
            username: username.to_owned(),
            hash: badge_hash(badge),
        })
        .collect()
}

/// The badges of the players of a live room (`RoomSnapshot::users`: owners of its objects,
/// its reserver and signer), each player once. Players without a badge (the Invader and
/// Source Keeper NPCs) are skipped.
pub fn room_wants(users: &BTreeMap<String, RoomUser>) -> Vec<Want> {
    users
        .values()
        .filter_map(|user| {
            Some(Want {
                user: user.id.clone(),
                username: user.username.clone(),
                hash: badge_hash(user.badge.as_ref()?),
            })
        })
        .collect()
}

/// What to load, in order: the players of the live rooms (`rooms`, close up on screen),
/// then those the map wants (`map`, [`wanted`]); each player once.
pub fn merge<'a>(rooms: impl IntoIterator<Item = &'a [Want]>, map: &'a [Want]) -> Vec<Want> {
    let mut seen = HashSet::new();
    rooms
        .into_iter()
        .flatten()
        .chain(map)
        .filter(|want| seen.insert(want.user.as_str()))
        .cloned()
        .collect()
}

/// Premultiplies straight-alpha RGBA `pixels` (as a canvas reads them) in place.
pub fn premultiply(pixels: &mut [u8]) {
    for [red, green, blue, alpha] in pixels.as_chunks_mut::<4>().0 {
        let alpha = u16::from(*alpha);
        for channel in [red, green, blue] {
            // Rounded `channel * alpha / 255`; at most 255.
            let scaled = (u16::from(*channel) * alpha + 127) / 255;
            *channel = u8::try_from(scaled).unwrap_or(u8::MAX);
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use loader::Badges;

/// The loader (browsers only).
#[cfg(target_arch = "wasm32")]
mod loader {
    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};
    use std::rc::Rc;

    use overseer::Overseer;
    use overseer::map::RoomCoord;
    use overseer::net::Priority;
    use overseer::net::rt;
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{
        Blob, BlobPropertyBag, HtmlImageElement, OffscreenCanvas,
        OffscreenCanvasRenderingContext2d, Url,
    };

    use super::{BADGE_BYTES, BADGE_SIZE, Want, merge, premultiply};
    use crate::terrain_cache::BadgeStore;

    /// Delivers a badge's pixels to a viewer.
    type Deliver = Rc<dyn Fn(&str, &[u8])>;

    /// What the loader knows.
    struct State {
        /// Players whose badge (with that hash) was delivered or tried.
        done: HashMap<String, u64>,
        /// The pixels of every badge delivered, for viewers that attach later.
        known: HashMap<String, Rc<[u8]>>,
        /// The viewers that get every badge, by attachment id.
        targets: Vec<(u64, Deliver)>,
        /// The id of the next attachment.
        next_target: u64,
        /// What the map's view wants ([`Badges::want`]).
        map: Vec<Want>,
        /// What each live room wants ([`Badges::want_room`]).
        rooms: Vec<(RoomCoord, Vec<Want>)>,
        /// Badges to get, first first.
        queue: VecDeque<Want>,
        /// The worker runs.
        running: bool,
        /// The loader was stopped (its screen left).
        stopped: bool,
    }

    /// The badge loader of one world screen or room window; cheap to clone.
    #[derive(Clone)]
    pub struct Badges {
        overseer: Overseer,
        server: Rc<str>,
        state: Rc<RefCell<State>>,
    }

    impl Badges {
        /// A loader for the server with base URL `server` (`overseer` connected to it),
        /// within the session's request budget.
        pub fn new(overseer: Overseer, server: &str) -> Self {
            Self {
                overseer,
                server: server.into(),
                state: Rc::new(RefCell::new(State {
                    done: HashMap::new(),
                    known: HashMap::new(),
                    targets: Vec::new(),
                    next_target: 0,
                    map: Vec::new(),
                    rooms: Vec::new(),
                    queue: VecDeque::new(),
                    running: false,
                    stopped: false,
                })),
            }
        }

        /// Whether `other` is this loader (a clone of it).
        pub fn is(&self, other: &Self) -> bool {
            Rc::ptr_eq(&self.state, &other.state)
        }

        /// Hands every badge to `deliver`: those delivered so far at once, then each new one.
        /// Returns the id that [`Self::detach`] takes.
        pub fn attach(&self, deliver: impl Fn(&str, &[u8]) + 'static) -> u64 {
            let deliver: Deliver = Rc::new(deliver);
            let (id, known) = {
                let mut state = self.state.borrow_mut();
                let id = state.next_target;
                state.next_target += 1;
                state.targets.push((id, Rc::clone(&deliver)));
                let known: Vec<(String, Rc<[u8]>)> = state
                    .known
                    .iter()
                    .map(|(user, pixels)| (user.clone(), Rc::clone(pixels)))
                    .collect();
                (id, known)
            };
            for (user, pixels) in known {
                deliver(&user, &pixels);
            }
            id
        }

        /// Stops handing badges to the viewer attached as `id`.
        pub fn detach(&self, id: u64) {
            self.state
                .borrow_mut()
                .targets
                .retain(|(target, _)| *target != id);
        }

        /// The badges the map needs now, nearest first (after those of the live rooms).
        pub fn want(&self, wants: Vec<Want>) {
            self.state.borrow_mut().map = wants;
            self.requeue();
        }

        /// The badges the players of the live room `room` need ([`super::room_wants`]).
        pub fn want_room(&self, room: RoomCoord, wants: Vec<Want>) {
            {
                let mut state = self.state.borrow_mut();
                match state.rooms.iter_mut().find(|(at, _)| *at == room) {
                    Some((_, held)) if *held == wants => return,
                    Some((_, held)) => *held = wants,
                    None => state.rooms.push((room, wants)),
                }
            }
            self.requeue();
        }

        /// The room `room` is no longer live: its players' badges are no longer needed.
        pub fn forget_room(&self, room: RoomCoord) {
            let removed = {
                let mut state = self.state.borrow_mut();
                let before = state.rooms.len();
                state.rooms.retain(|(at, _)| *at != room);
                state.rooms.len() != before
            };
            if removed {
                self.requeue();
            }
        }

        /// Replaces what was still queued by what the live rooms and the map need now.
        fn requeue(&self) {
            let start = {
                let mut state = self.state.borrow_mut();
                if state.stopped {
                    return;
                }
                let wanted = merge(
                    state.rooms.iter().map(|(_, wants)| wants.as_slice()),
                    &state.map,
                );
                let fresh: VecDeque<Want> = wanted
                    .into_iter()
                    .filter(|want| state.done.get(&want.user) != Some(&want.hash))
                    .collect();
                state.queue = fresh;
                let start = !state.running && !state.queue.is_empty();
                state.running |= start;
                start
            };
            if start {
                wasm_bindgen_futures::spawn_local(self.clone().run());
            }
        }

        /// Stops the loader: queued badges are dropped, nothing more is delivered.
        pub fn stop(&self) {
            let mut state = self.state.borrow_mut();
            state.stopped = true;
            state.queue.clear();
            state.targets.clear();
        }

        /// Gets queued badges one by one until none is left.
        async fn run(self) {
            let store = BadgeStore::open(&self.server).await.ok();
            loop {
                let want = {
                    let mut state = self.state.borrow_mut();
                    let next = if state.stopped {
                        None
                    } else {
                        state.queue.pop_front()
                    };
                    let Some(want) = next else {
                        state.running = false;
                        return;
                    };
                    state.done.insert(want.user.clone(), want.hash);
                    want
                };
                if let Some(store) = &store
                    && let Ok(Some(pixels)) = store.get(&want.user, want.hash).await
                {
                    self.deliver(&want.user, &pixels);
                    continue;
                }
                while let Some(at) = self.overseer.client().budget_ready_at(Priority::Visible, 0) {
                    rt::sleep_until(at).await;
                }
                // A badge that cannot be fetched or drawn is not tried again on this screen.
                let Ok(svg) = self.overseer.map().badge_svg(&want.username, false).await else {
                    continue;
                };
                let Ok(pixels) = rasterize(&svg).await else {
                    continue;
                };
                if let Some(store) = &store {
                    // Best effort: an unstored badge is fetched again next time.
                    let _ = store.put(&want.user, want.hash, &pixels).await;
                }
                self.deliver(&want.user, &pixels);
            }
        }

        /// Keeps `pixels` and hands them to every attached viewer, unless the loader was
        /// stopped.
        fn deliver(&self, user: &str, pixels: &[u8]) {
            let targets: Vec<Deliver> = {
                let mut state = self.state.borrow_mut();
                if state.stopped {
                    return;
                }
                state.known.insert(user.to_owned(), Rc::from(pixels));
                state
                    .targets
                    .iter()
                    .map(|(_, deliver)| Rc::clone(deliver))
                    .collect()
            };
            for deliver in targets {
                deliver(user, pixels);
            }
        }
    }

    /// Draws `svg` into [`BADGE_SIZE`]² premultiplied RGBA pixels.
    async fn rasterize(svg: &str) -> Result<Vec<u8>, JsValue> {
        let parts = js_sys::Array::of1(&JsValue::from_str(svg));
        let options = BlobPropertyBag::new();
        options.set_type("image/svg+xml");
        let blob = Blob::new_with_str_sequence_and_options(&parts, &options)?;
        let url = Url::create_object_url_with_blob(&blob)?;
        let image = HtmlImageElement::new()?;
        image.set_src(&url);
        let decoded = JsFuture::from(image.decode()).await;
        let _ = Url::revoke_object_url(&url);
        decoded?;
        let canvas = OffscreenCanvas::new(BADGE_SIZE, BADGE_SIZE)?;
        let context: OffscreenCanvasRenderingContext2d = canvas
            .get_context("2d")?
            .ok_or_else(|| JsValue::from_str("no 2d context"))?
            .dyn_into()?;
        let size = f64::from(BADGE_SIZE);
        context.draw_image_with_html_image_element_and_dw_and_dh(&image, 0.0, 0.0, size, size)?;
        let mut pixels = context.get_image_data(0.0, 0.0, size, size)?.data().0;
        if pixels.len() != BADGE_BYTES {
            return Err(JsValue::from_str("unexpected image size"));
        }
        premultiply(&mut pixels);
        Ok(pixels)
    }
}

/// A badge for tests: shape `shape`, otherwise fixed.
#[cfg(test)]
pub fn test_badge(shape: i64) -> UserBadge {
    use overseer::model::account::{UserBadgeColor, UserBadgeType};
    UserBadge {
        color1: UserBadgeColor::String("#ff0000".to_owned()),
        color2: UserBadgeColor::String("#00ff00".to_owned()),
        color3: UserBadgeColor::Integer(4),
        decoration: None,
        flip: false,
        param: Some(12.0),
        r#type: UserBadgeType::Integer(shape),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use overseer::map::{RoomCoord, RoomOwner, RoomRect, RoomReservation, RoomStat, RoomStatus};
    use overseer::model::world::RoomUser;

    use super::{Want, badge_hash, merge, premultiply, room_wants, test_badge, wanted};

    #[test]
    fn badge_hashes_follow_the_parameters_and_stay_stable() {
        let badge = test_badge(3);
        assert_eq!(badge_hash(&badge), badge_hash(&test_badge(3)));
        assert_ne!(badge_hash(&badge), badge_hash(&test_badge(4)));
        let mut flipped = test_badge(3);
        flipped.flip = true;
        assert_ne!(badge_hash(&badge), badge_hash(&flipped));
        // FNV-1a of the JSON: the same value in every build (keys of the badge cache).
        assert_eq!(badge_hash(&badge), badge_hash(&badge.clone()));
        assert_ne!(badge_hash(&badge), 0);
    }

    fn stat(owner: Option<(&str, i64)>, reserver: Option<(&str, i64)>) -> RoomStat {
        RoomStat {
            status: RoomStatus::Normal,
            status_until: None,
            owner: owner.map(|(name, shape)| RoomOwner {
                id: format!("id-{name}"),
                username: Some(name.to_owned()),
                level: 4,
                badge: Some(test_badge(shape)),
            }),
            reservation: reserver.map(|(name, shape)| RoomReservation {
                id: format!("id-{name}"),
                username: Some(name.to_owned()),
                badge: Some(test_badge(shape)),
            }),
            safe_mode: false,
            mineral: None,
            mineral_density: None,
            fetched_at: 0,
        }
    }

    #[test]
    fn nearest_players_first_each_once() {
        let room = |x, y| RoomCoord { x, y };
        let far = stat(Some(("far", 1)), Some(("near", 2)));
        let near = stat(Some(("near", 2)), None);
        let mid = stat(None, Some(("mid", 3)));
        let empty = stat(None, None);
        let mut nameless = stat(Some(("ghost", 1)), None);
        if let Some(owner) = nameless.owner.as_mut() {
            owner.username = None;
        }
        let wants = wanted(
            [
                (room(10, 0), &far),
                (room(1, 0), &near),
                (room(0, 4), &mid),
                (room(0, 0), &empty),
                (room(0, 1), &nameless),
                // Out of view.
                (room(11, 0), &stat(Some(("outside", 4)), None)),
            ],
            RoomRect::new(room(-10, -10), room(10, 10)),
        );
        let want = |name: &str, shape| Want {
            user: format!("id-{name}"),
            username: name.to_owned(),
            hash: badge_hash(&test_badge(shape)),
        };
        assert_eq!(wants, [want("near", 2), want("mid", 3), want("far", 1)]);
    }

    #[test]
    fn live_room_players_come_first_each_once() {
        let user = |id: &str, shape: Option<i64>| {
            (
                id.to_owned(),
                RoomUser {
                    id: id.to_owned(),
                    username: format!("name-{id}"),
                    badge: shape.map(test_badge),
                },
            )
        };
        // The NPCs have no badge: nothing to load for their creeps.
        let users = BTreeMap::from([
            user("2", None),
            user("3", None),
            user("me", Some(1)),
            user("foe", Some(2)),
        ]);
        let want = |id: &str, shape| Want {
            user: id.to_owned(),
            username: format!("name-{id}"),
            hash: badge_hash(&test_badge(shape)),
        };
        let room = room_wants(&users);
        assert_eq!(room, [want("foe", 2), want("me", 1)]);
        // The map's owners follow the live rooms' players; a player in both comes once, first.
        let map = [want("far", 3), want("me", 1)];
        let other = [want("me", 1), want("near", 4)];
        assert_eq!(
            merge([room.as_slice(), other.as_slice()], &map),
            [
                want("foe", 2),
                want("me", 1),
                want("near", 4),
                want("far", 3)
            ]
        );
    }

    #[test]
    fn pixels_are_premultiplied() {
        let mut pixels = [255, 128, 0, 255, 255, 128, 10, 128, 200, 100, 50, 0];
        premultiply(&mut pixels);
        assert_eq!(pixels, [255, 128, 0, 255, 128, 64, 5, 128, 0, 0, 0, 0]);
    }
}
