//! Other players: profiles, rooms, badges and leaderboards.
//!
//! Viewers get badges through [`Badges`] ([`Users::badges`]): one session-wide cache and pace
//! for every viewer.

mod badges;

pub(crate) use badges::BadgeCache;
pub use badges::{BadgeKey, Badges, MAX_CACHED, RETRY_FAILED, badge_hash};

use overseer_net::Api;
use overseer_net::api::UsersApi as _;
use overseer_net::model::account::UserRoomStatValues;
use overseer_net::model::users::{
    LeaderboardMode, LeaderboardPage, LeaderboardResult, LeaderboardSeason, PublicProfile,
    RoomHoldings, UserRoom,
};

use crate::rooms::StatInterval;
use crate::{Error, Overseer};

/// Other players: profiles, rooms, badges and leaderboards.
///
/// Created by [`Overseer::users`]; cheap to clone. These are public: they work anonymously.
/// Every operation fails with [`Error::Closed`] after [`Overseer::shutdown`] and with
/// [`Error::Net`] when the request fails.
#[derive(Clone, Debug)]
pub struct Users {
    overseer: Overseer,
}

/// The reply of `GET /api/user/find` from `api`, with an unknown user as `None`. No contract
/// sends a status or code for it, only an error string: `user not found` on `OpenScreeps`
/// servers, an error string with `not found` on screeps.com (its contract does not give the
/// exact string); community servers fail with an HTTP 200 `TypeError: …` (the handler reads
/// the missing user). A failure without the server's string (transport, decoding) is never
/// an unknown user.
fn found(
    api: &Api,
    reply: Result<PublicProfile, overseer_net::Error>,
) -> Result<Option<PublicProfile>, Error> {
    match reply {
        Ok(profile) => Ok(Some(profile)),
        Err(error) => {
            let unknown = error.server_message().is_some_and(|message| match api {
                Api::Official(_) => message.contains("not found"),
                Api::OpenScreeps(_) => message == "user not found",
                Api::Community(_) => message.starts_with("TypeError"),
            });
            if unknown { Ok(None) } else { Err(error.into()) }
        }
    }
}

impl Users {
    pub(crate) fn new(overseer: Overseer) -> Self {
        Self { overseer }
    }

    /// The public profile of the user named `username`, case-insensitively, or `None` when no
    /// user has the name (`GET /api/user/find?username`).
    ///
    /// # Errors
    ///
    /// See [`Users`].
    pub async fn find(&self, username: &str) -> Result<Option<PublicProfile>, Error> {
        let api = self.api()?;
        found(api, api.find(Some(username), None).await)
    }

    /// The public profile of the user with id `id`, or `None` when no user has the id
    /// (`GET /api/user/find?id`).
    ///
    /// # Errors
    ///
    /// See [`Users`].
    pub async fn find_by_id(&self, id: &str) -> Result<Option<PublicProfile>, Error> {
        let api = self.api()?;
        found(api, api.find(None, Some(id)).await)
    }

    /// The rooms whose controller user `id` owns, with their shard on sharded servers
    /// (`GET /api/user/rooms`). The list does not show a shard without owned rooms, and it
    /// does not tell such a shard from a shard that the reply does not report. Use
    /// [`Self::room_holdings`] for that difference and for the reserved rooms.
    ///
    /// # Errors
    ///
    /// See [`Users`].
    pub async fn rooms(&self, id: &str) -> Result<Vec<UserRoom>, Error> {
        Ok(self.api()?.rooms(id, false).await?.into())
    }

    /// The rooms that user `id` holds, as the server reports them (`GET /api/user/rooms`).
    /// With `reservations`, the request asks for the reserved rooms too. Only sharded servers
    /// report reserved rooms: a single-world server gives [`RoomHoldings::SingleWorld`].
    ///
    /// Each call sends a request; nothing is kept in a cache. The holdings are not proof
    /// that the user has no creeps or other objects in a room. Single-world servers give an
    /// empty list for an unknown `id`.
    ///
    /// # Errors
    ///
    /// See [`Users`]. A failed request is an error, never an empty report.
    pub async fn room_holdings(&self, id: &str, reservations: bool) -> Result<RoomHoldings, Error> {
        Ok(self.api()?.rooms(id, reservations).await?)
    }

    /// The totals of each room statistic of user `id` over `interval`
    /// (`GET /api/user/stats`), keyed by statistic name (`energyHarvested`, `creepsLost`, …).
    ///
    /// # Errors
    ///
    /// See [`Users`].
    pub async fn stats(
        &self,
        id: &str,
        interval: StatInterval,
    ) -> Result<UserRoomStatValues, Error> {
        Ok(self.api()?.stats(id, interval.minutes()).await?)
    }

    /// The badge of the user named `username` as an SVG document, optionally with a border
    /// (`GET /api/user/badge-svg`).
    ///
    /// # Errors
    ///
    /// See [`Users`].
    pub async fn badge_svg(&self, username: &str, border: bool) -> Result<String, Error> {
        Ok(self.api()?.badge_svg(username, border).await?)
    }

    /// Player badges as SVG documents through the session's shared cache and request
    /// budget: what viewers use ([`Badges`]).
    #[must_use]
    pub fn badges(&self) -> Badges {
        Badges::new(self.overseer.clone())
    }

    /// A page of the `mode` ranking of `season` (`YYYY-MM`; `None`: the current season):
    /// `limit` positions from position `offset` (from 0), the total count and the listed
    /// users (`GET /api/leaderboard/list`). The official server serves at most 20 per page.
    ///
    /// # Errors
    ///
    /// See [`Users`].
    pub async fn leaderboard(
        &self,
        mode: LeaderboardMode,
        season: Option<&str>,
        offset: u64,
        limit: u64,
    ) -> Result<LeaderboardPage, Error> {
        Ok(self.api()?.leaderboard(mode, season, offset, limit).await?)
    }

    /// The rank and score of the user named `username` in the `mode` ranking: in `season`
    /// (`YYYY-MM`; one entry), or in every season when `None` (`GET /api/leaderboard/find`).
    ///
    /// # Errors
    ///
    /// See [`Users`]; an unranked user is the server's `result not found`.
    pub async fn leaderboard_find(
        &self,
        mode: LeaderboardMode,
        username: &str,
        season: Option<&str>,
    ) -> Result<Vec<LeaderboardResult>, Error> {
        Ok(self.api()?.leaderboard_find(mode, username, season).await?)
    }

    /// The leaderboard seasons, newest (the current one) first
    /// (`GET /api/leaderboard/seasons`).
    ///
    /// # Errors
    ///
    /// See [`Users`].
    pub async fn leaderboard_seasons(&self) -> Result<Vec<LeaderboardSeason>, Error> {
        Ok(self.api()?.leaderboard_seasons().await?)
    }

    /// The server's operations, or [`Error::Closed`].
    fn api(&self) -> Result<&Api, Error> {
        self.overseer.ensure_open()?;
        Ok(self.overseer.api())
    }
}
