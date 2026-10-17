//! Other players: profiles, rooms, badges and leaderboards.

use overseer_net::Api;
use overseer_net::api::UsersApi as _;
use overseer_net::model::account::UserRoomStatValues;
use overseer_net::model::users::{
    LeaderboardMode, LeaderboardPage, LeaderboardResult, LeaderboardSeason, PublicProfile, UserRoom,
};

use crate::rooms::StatInterval;
use crate::{Error, Overseer};

/// Other players: profiles, rooms, badges and leaderboards.
///
/// Created by [`Overseer::users`]; cheap to clone. These are public: they work anonymously.
/// Every operation fails with [`Error::Closed`] after [`Overseer::shutdown`] and with
/// [`Error::Net`] when the request fails (an unknown user is the server's `user not found`).
#[derive(Clone, Debug)]
pub struct Users {
    overseer: Overseer,
}

impl Users {
    pub(crate) fn new(overseer: Overseer) -> Self {
        Self { overseer }
    }

    /// The public profile of the user named `username`, case-insensitively
    /// (`GET /api/user/find?username`).
    ///
    /// # Errors
    ///
    /// See [`Users`].
    pub async fn find(&self, username: &str) -> Result<PublicProfile, Error> {
        Ok(self.api()?.find(Some(username), None).await?)
    }

    /// The public profile of the user with id `id` (`GET /api/user/find?id`).
    ///
    /// # Errors
    ///
    /// See [`Users`].
    pub async fn find_by_id(&self, id: &str) -> Result<PublicProfile, Error> {
        Ok(self.api()?.find(None, Some(id)).await?)
    }

    /// The rooms whose controller user `id` owns, with their shard on sharded servers
    /// (`GET /api/user/rooms`).
    ///
    /// # Errors
    ///
    /// See [`Users`].
    pub async fn rooms(&self, id: &str) -> Result<Vec<UserRoom>, Error> {
        Ok(self.api()?.rooms(id).await?)
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
