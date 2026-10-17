//! Users area: other players' public profiles, rooms, statistics and badges, and the
//! leaderboards. Every operation is public: it works without signing in.

use overseer_core::api::{community, official, openscreeps};
use overseer_core::model::account::UserRoomStatValues;
use overseer_core::model::users::{
    LeaderboardMode, LeaderboardPage, LeaderboardResult, LeaderboardSeason, PublicProfile, UserRoom,
};

use super::{Api, Community, Official, OpenScreeps, delegate};
use crate::Error;

/// Other players and the leaderboards.
pub trait UsersApi {
    /// The public profile of the user named `username`, case-insensitively, or of the user
    /// with id `id` (`GET /api/user/find`; `id` wins when both are given).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send); an unknown user is the
    /// server's `user not found` (on community servers an HTTP 200 `TypeError: …`).
    fn find(
        &self,
        username: Option<&str>,
        id: Option<&str>,
    ) -> impl Future<Output = Result<PublicProfile, Error>>;

    /// The rooms whose controller user `id` owns (`GET /api/user/rooms`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn rooms(&self, id: &str) -> impl Future<Output = Result<Vec<UserRoom>, Error>>;

    /// The totals of each room statistic of user `id` over the last `interval` minutes (8,
    /// 180 or 1440), keyed by statistic name (`GET /api/user/stats`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn stats(
        &self,
        id: &str,
        interval: u64,
    ) -> impl Future<Output = Result<UserRoomStatValues, Error>>;

    /// The badge of the user named `username` as an SVG document, with a black ring when
    /// `border` (`GET /api/user/badge-svg`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn badge_svg(
        &self,
        username: &str,
        border: bool,
    ) -> impl Future<Output = Result<String, Error>>;

    /// A page of the `mode` ranking of `season` (`YYYY-MM`; `None`: the current one): `limit`
    /// positions from position `offset` (`GET /api/leaderboard/list`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn leaderboard(
        &self,
        mode: LeaderboardMode,
        season: Option<&str>,
        offset: u64,
        limit: u64,
    ) -> impl Future<Output = Result<LeaderboardPage, Error>>;

    /// The rankings of the user named `username` in the `mode` ranking: the one of `season`
    /// (`YYYY-MM`), or one per season when `None` (`GET /api/leaderboard/find`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send); an unranked user is the
    /// server's `result not found`.
    fn leaderboard_find(
        &self,
        mode: LeaderboardMode,
        username: &str,
        season: Option<&str>,
    ) -> impl Future<Output = Result<Vec<LeaderboardResult>, Error>>;

    /// The leaderboard seasons, newest (the current one) first
    /// (`GET /api/leaderboard/seasons`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn leaderboard_seasons(&self) -> impl Future<Output = Result<Vec<LeaderboardSeason>, Error>>;
}

impl UsersApi for Api {
    async fn find(&self, username: Option<&str>, id: Option<&str>) -> Result<PublicProfile, Error> {
        delegate!(self, api => api.find(username, id).await)
    }

    async fn rooms(&self, id: &str) -> Result<Vec<UserRoom>, Error> {
        delegate!(self, api => api.rooms(id).await)
    }

    async fn stats(&self, id: &str, interval: u64) -> Result<UserRoomStatValues, Error> {
        delegate!(self, api => api.stats(id, interval).await)
    }

    async fn badge_svg(&self, username: &str, border: bool) -> Result<String, Error> {
        delegate!(self, api => api.badge_svg(username, border).await)
    }

    async fn leaderboard(
        &self,
        mode: LeaderboardMode,
        season: Option<&str>,
        offset: u64,
        limit: u64,
    ) -> Result<LeaderboardPage, Error> {
        delegate!(self, api => api.leaderboard(mode, season, offset, limit).await)
    }

    async fn leaderboard_find(
        &self,
        mode: LeaderboardMode,
        username: &str,
        season: Option<&str>,
    ) -> Result<Vec<LeaderboardResult>, Error> {
        delegate!(self, api => api.leaderboard_find(mode, username, season).await)
    }

    async fn leaderboard_seasons(&self) -> Result<Vec<LeaderboardSeason>, Error> {
        delegate!(self, api => api.leaderboard_seasons().await)
    }
}

/// [`UsersApi`] for `$api`, sending `$contract`'s endpoints; the replies differ in shape only,
/// which their conversions handle.
macro_rules! users_api {
    ($api:ty, $contract:ident) => {
        impl UsersApi for $api {
            async fn find(
                &self,
                username: Option<&str>,
                id: Option<&str>,
            ) -> Result<PublicProfile, Error> {
                let find = $contract::endpoints::UserFind {
                    username: username.map(str::to_owned),
                    id: id.map(str::to_owned),
                };
                Ok(self.client.send(&find).await?.body.user.into())
            }

            async fn rooms(&self, id: &str) -> Result<Vec<UserRoom>, Error> {
                let rooms = $contract::endpoints::UserRooms {
                    id: id.to_owned(),
                    reservation: None,
                };
                Ok(self.client.send(&rooms).await?.body.into())
            }

            async fn stats(&self, id: &str, interval: u64) -> Result<UserRoomStatValues, Error> {
                let stats = $contract::endpoints::UserStats {
                    id: id.to_owned(),
                    interval,
                };
                Ok(self.client.send(&stats).await?.body.stats)
            }

            async fn badge_svg(&self, username: &str, border: bool) -> Result<String, Error> {
                let badge = $contract::endpoints::UserBadgeSvg {
                    username: username.to_owned(),
                    border: border.then_some($contract::types::UserBadgeSvgBorder::V1),
                };
                Ok(self.client.send(&badge).await?.body)
            }

            async fn leaderboard(
                &self,
                mode: LeaderboardMode,
                season: Option<&str>,
                offset: u64,
                limit: u64,
            ) -> Result<LeaderboardPage, Error> {
                let list = $contract::endpoints::LeaderboardList {
                    mode,
                    season: season.map(str::to_owned),
                    limit: Some(limit),
                    offset: Some(offset),
                };
                Ok(self.client.send(&list).await?.body.into())
            }

            async fn leaderboard_find(
                &self,
                mode: LeaderboardMode,
                username: &str,
                season: Option<&str>,
            ) -> Result<Vec<LeaderboardResult>, Error> {
                let find = $contract::endpoints::LeaderboardFind {
                    mode,
                    username: username.to_owned(),
                    season: season.map(str::to_owned),
                };
                Ok(self.client.send(&find).await?.body.into())
            }

            async fn leaderboard_seasons(&self) -> Result<Vec<LeaderboardSeason>, Error> {
                let seasons = $contract::endpoints::LeaderboardSeasons::default();
                Ok(self.client.send(&seasons).await?.body.seasons)
            }
        }
    };
}

users_api!(Official, official);
users_api!(OpenScreeps, openscreeps);
users_api!(Community, community);
