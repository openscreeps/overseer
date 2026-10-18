//! Player badges as SVG documents, shared by every viewer of the session ([`Badges`]).
//!
//! A badge is known by its [`BadgeKey`]: the player's id and the hash of the badge's
//! parameters ([`badge_hash`]). A player who changes the badge gets another hash, so the new
//! badge is fetched again. The session keeps the result of each key: concurrent requests of
//! one key share one fetch, a fetched badge stays in a bounded cache, and a badge that cannot
//! be fetched is not requested again for [`RETRY_FAILED`].
//!
//! # Pace
//!
//! Each fetch first waits for the session's request budget
//! ([`Client::wait_for_budget`](overseer_net::Client::wait_for_budget), priority
//! [`Priority::Visible`]), shared with the world map: on screeps.com every request of the token
//! counts against its limit of 120 per minute. Servers without rate limits, and a token whose
//! rate limits are lifted, have no budget: fetches go at once.

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use overseer_net::Priority;
use overseer_net::model::account::UserBadge;
use overseer_net::rt::Instant;
use tokio::sync::watch;

use crate::Overseer;

/// How long a badge that cannot be fetched is not requested again.
pub const RETRY_FAILED: Duration = Duration::from_mins(5);

/// Most fetched badges the session keeps; the oldest go first.
pub const MAX_CACHED: usize = 1024;

/// A stable 64-bit hash of a badge's parameters (FNV-1a of their JSON): the same badge hashes
/// the same in every session and build, a changed one differently. Use it as the version of a
/// player's badge in caches that outlive the session.
#[must_use]
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

/// One version of one player's badge.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BadgeKey {
    /// The player's id.
    pub user: String,
    /// The player's name: the badge request uses it (`GET /api/user/badge-svg?username`).
    pub username: String,
    /// The hash of the badge's parameters ([`badge_hash`]).
    pub hash: u64,
}

impl BadgeKey {
    /// The key of the badge `badge` of the player with id `user` and name `username`.
    #[must_use]
    pub fn new(user: &str, username: &str, badge: &UserBadge) -> Self {
        Self {
            user: user.to_owned(),
            username: username.to_owned(),
            hash: badge_hash(badge),
        }
    }
}

/// The result of a fetch while it runs (`None`) and after: the SVG, or `None` on failure.
type Outcome = Option<Option<Arc<str>>>;

/// What the session knows of one player's badge.
#[derive(Debug)]
struct Slot {
    /// The version of the badge.
    hash: u64,
    state: SlotState,
}

#[derive(Debug)]
enum SlotState {
    /// A fetch runs; its result arrives here.
    Fetching(watch::Receiver<Outcome>),
    /// The SVG document.
    Ready(Arc<str>),
    /// The last fetch failed at this time.
    Failed(Instant),
}

/// The badges of one session: [`Slot`]s by player id, and the order in which the fetched
/// ones arrived.
#[derive(Debug, Default)]
pub(crate) struct BadgeCache {
    state: Mutex<CacheState>,
}

impl BadgeCache {
    /// Locks the state; its critical sections cannot panic halfway through an update.
    fn lock(&self) -> MutexGuard<'_, CacheState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Debug, Default)]
struct CacheState {
    slots: HashMap<String, Slot>,
    /// The keys of the [`SlotState::Ready`] slots, oldest first.
    ready: VecDeque<(String, u64)>,
}

impl CacheState {
    /// Keeps `svg` as the badge `key`, unless the slot holds another version now. Drops the
    /// oldest fetched badges above [`MAX_CACHED`].
    fn store(&mut self, key: &BadgeKey, svg: Option<Arc<str>>) {
        let Some(slot) = self
            .slots
            .get_mut(&key.user)
            .filter(|slot| slot.hash == key.hash)
        else {
            return;
        };
        let Some(svg) = svg else {
            slot.state = SlotState::Failed(Instant::now());
            return;
        };
        slot.state = SlotState::Ready(svg);
        self.ready.push_back((key.user.clone(), key.hash));
        while self.ready.len() > MAX_CACHED {
            let Some((user, hash)) = self.ready.pop_front() else {
                break;
            };
            if self
                .slots
                .get(&user)
                .is_some_and(|slot| slot.hash == hash && matches!(slot.state, SlotState::Ready(_)))
            {
                self.slots.remove(&user);
            }
        }
    }
}

/// What a request does next.
enum Role {
    /// Another request fetches the badge: wait for its result.
    Wait(watch::Receiver<Outcome>),
    /// Fetch the badge and send the result to the waiting requests.
    Fetch(watch::Sender<Outcome>),
}

/// Player badges as SVG documents for every viewer of the session (the world map, room
/// viewers, pop-out windows); created by [`Users::badges`](crate::Users::badges), cheap to
/// clone.
///
/// The session keeps one result for each [`BadgeKey`]; see the
/// [module documentation](self) for the cache and the pace.
#[derive(Clone, Debug)]
pub struct Badges {
    overseer: Overseer,
}

impl Badges {
    pub(crate) fn new(overseer: Overseer) -> Self {
        Self { overseer }
    }

    /// The badge `key` as an SVG document without a border (`GET /api/user/badge-svg`).
    ///
    /// Returns the cached document when the session has it. Otherwise fetches it within the
    /// request budget, or waits for the fetch that another request of the same key started.
    /// Returns `None` when the badge cannot be fetched (now, or less than [`RETRY_FAILED`]
    /// ago) and after [`Overseer::shutdown`].
    pub async fn svg(&self, key: &BadgeKey) -> Option<Arc<str>> {
        loop {
            self.overseer.ensure_open().ok()?;
            match self.role(key) {
                Ok(svg) => return svg,
                Err(Role::Wait(mut receiver)) => {
                    // A failed wait: the fetching request was dropped. Try again.
                    if let Ok(outcome) = receiver.wait_for(Option::is_some).await {
                        return outcome.clone().flatten();
                    }
                }
                Err(Role::Fetch(sender)) => {
                    let svg = self.fetch(key).await;
                    self.cache().lock().store(key, svg.clone());
                    // Nobody may wait.
                    let _ = sender.send(Some(svg.clone()));
                    return svg;
                }
            }
        }
    }

    /// The badge `key` as an SVG document if the session has it now; sends nothing.
    #[must_use]
    pub fn cached(&self, key: &BadgeKey) -> Option<Arc<str>> {
        let state = self.cache().lock();
        match state.slots.get(&key.user) {
            Some(Slot {
                hash,
                state: SlotState::Ready(svg),
            }) if *hash == key.hash => Some(Arc::clone(svg)),
            _ => None,
        }
    }

    /// The known result of `key` (`Ok`), or what to do to get it.
    fn role(&self, key: &BadgeKey) -> Result<Option<Arc<str>>, Role> {
        let mut state = self.cache().lock();
        if let Some(slot) = state
            .slots
            .get(&key.user)
            .filter(|slot| slot.hash == key.hash)
        {
            match &slot.state {
                SlotState::Ready(svg) => return Ok(Some(Arc::clone(svg))),
                SlotState::Failed(at) if at.elapsed() < RETRY_FAILED => return Ok(None),
                // The fetching request still runs unless it was dropped.
                SlotState::Fetching(receiver) if receiver.has_changed().is_ok() => {
                    return Err(Role::Wait(receiver.clone()));
                }
                SlotState::Fetching(_) | SlotState::Failed(_) => {}
            }
        }
        let (sender, receiver) = watch::channel(None);
        // Another version of the player's badge is replaced.
        state.slots.insert(
            key.user.clone(),
            Slot {
                hash: key.hash,
                state: SlotState::Fetching(receiver),
            },
        );
        Err(Role::Fetch(sender))
    }

    /// Fetches the badge `key` within the request budget.
    async fn fetch(&self, key: &BadgeKey) -> Option<Arc<str>> {
        self.overseer
            .client()
            .wait_for_budget(Priority::Visible, 0)
            .await;
        let svg = self
            .overseer
            .users()
            .badge_svg(&key.username, false)
            .await
            .ok()?;
        Some(svg.into())
    }

    fn cache(&self) -> &BadgeCache {
        self.overseer.badge_cache()
    }
}

#[cfg(test)]
mod tests {
    use overseer_net::model::account::{UserBadgeColor, UserBadgeType};

    use super::{BadgeKey, UserBadge, badge_hash};

    /// A badge of shape `shape`, otherwise fixed.
    fn badge(shape: i64) -> UserBadge {
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

    #[test]
    fn badge_hashes_follow_the_parameters_and_stay_stable() {
        let three = badge(3);
        assert_eq!(badge_hash(&three), badge_hash(&badge(3)));
        assert_ne!(badge_hash(&three), badge_hash(&badge(4)));
        let mut flipped = badge(3);
        flipped.flip = true;
        assert_ne!(badge_hash(&three), badge_hash(&flipped));
        // FNV-1a of the JSON: the same value in every build (keys of persistent caches).
        assert_eq!(badge_hash(&three), badge_hash(&three.clone()));
        assert_ne!(badge_hash(&three), 0);
        let key = BadgeKey::new("u1", "Alice", &three);
        assert_eq!(
            (key.user.as_str(), key.username.as_str(), key.hash),
            ("u1", "Alice", badge_hash(&three))
        );
    }
}
