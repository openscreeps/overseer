//! Other players: profiles, rooms, badges and leaderboards.

use overseer_net::core::api::{endpoints, types};

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
    pub async fn find(&self, username: &str) -> Result<types::UserPublicProfile, Error> {
        let find = endpoints::UserFind {
            username: Some(username.to_owned()),
            id: None,
        };
        Ok(self.overseer.send(&find).await?.user)
    }

    /// The public profile of the user with id `id` (`GET /api/user/find?id`).
    ///
    /// # Errors
    ///
    /// See [`Users`].
    pub async fn find_by_id(&self, id: &str) -> Result<types::UserPublicProfile, Error> {
        let find = endpoints::UserFind {
            username: None,
            id: Some(id.to_owned()),
        };
        Ok(self.overseer.send(&find).await?.user)
    }

    /// The rooms whose controller user `id` owns, per shard on sharded servers
    /// (`GET /api/user/rooms`).
    ///
    /// # Errors
    ///
    /// See [`Users`].
    pub async fn rooms(&self, id: &str) -> Result<types::UserRoomsResponse, Error> {
        let rooms = endpoints::UserRooms {
            id: id.to_owned(),
            reservation: None,
        };
        self.overseer.send(&rooms).await
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
    ) -> Result<types::UserRoomStatValues, Error> {
        let stats = endpoints::UserStats {
            id: id.to_owned(),
            interval: interval.minutes(),
        };
        Ok(self.overseer.send(&stats).await?.stats)
    }

    /// The badge of the user named `username` as an SVG document, optionally with a border
    /// (`GET /api/user/badge-svg`).
    ///
    /// # Errors
    ///
    /// See [`Users`].
    pub async fn badge_svg(&self, username: &str, border: bool) -> Result<String, Error> {
        let badge = endpoints::UserBadgeSvg {
            username: username.to_owned(),
            border: border.then_some(types::UserBadgeSvgBorder::V1),
        };
        self.overseer.send(&badge).await
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
        mode: types::LeaderboardMode,
        season: Option<&str>,
        offset: u64,
        limit: u64,
    ) -> Result<types::LeaderboardListResponse, Error> {
        let list = endpoints::LeaderboardList {
            mode,
            season: season.map(str::to_owned),
            limit: Some(limit),
            offset: Some(offset),
        };
        self.overseer.send(&list).await
    }

    /// The rank and score of the user named `username` in the `mode` ranking of `season`
    /// (`YYYY-MM`), or in every season when `None` (`GET /api/leaderboard/find`).
    ///
    /// # Errors
    ///
    /// See [`Users`]; an unranked user is the server's `result not found`.
    pub async fn leaderboard_find(
        &self,
        mode: types::LeaderboardMode,
        username: &str,
        season: Option<&str>,
    ) -> Result<types::LeaderboardFindResponse, Error> {
        let find = endpoints::LeaderboardFind {
            mode,
            username: username.to_owned(),
            season: season.map(str::to_owned),
        };
        self.overseer.send(&find).await
    }

    /// The leaderboard seasons, newest (the current one) first
    /// (`GET /api/leaderboard/seasons`).
    ///
    /// # Errors
    ///
    /// See [`Users`].
    pub async fn leaderboard_seasons(&self) -> Result<Vec<types::LeaderboardSeason>, Error> {
        Ok(self
            .overseer
            .send(&endpoints::LeaderboardSeasons::default())
            .await?
            .seasons)
    }
}
