//! The market: orders, statistics and money history.

use overseer_net::api::MarketApi as _;
use overseer_net::model::market::{
    MarketOrder, MarketOrderIndexEntry, MarketStatsEntry, MoneyHistory, MyOrders,
};

use crate::{Error, Overseer};

/// The market: orders, statistics and money history.
///
/// Created by [`Overseer::market`]; cheap to clone. Every operation needs a signed-in user
/// and fails with [`Error::Anonymous`] otherwise, with [`Error::Closed`] after
/// [`Overseer::shutdown`] and with [`Error::Net`] when the request fails. Prices are in
/// credits. `shard: None` names the default shard; single-world servers ignore `shard`.
#[derive(Clone, Debug)]
pub struct Market {
    overseer: Overseer,
}

impl Market {
    pub(crate) fn new(overseer: Overseer) -> Self {
        Self { overseer }
    }

    /// One entry per resource type with active orders on `shard`: order count and buy/sell
    /// split (`GET /api/game/market/orders-index`).
    ///
    /// # Errors
    ///
    /// See [`Market`].
    pub async fn orders_index(
        &self,
        shard: Option<&str>,
    ) -> Result<Vec<MarketOrderIndexEntry>, Error> {
        let api = self.overseer.user_api()?;
        Ok(api.orders_index(self.overseer.resolve_shard(shard)).await?)
    }

    /// The active buy and sell orders for `resource` (for example `energy`, `H` or `pixel`)
    /// on `shard` (`GET /api/game/market/orders`).
    ///
    /// # Errors
    ///
    /// See [`Market`].
    pub async fn orders(
        &self,
        resource: &str,
        shard: Option<&str>,
    ) -> Result<Vec<MarketOrder>, Error> {
        let api = self.overseer.user_api()?;
        let shard = self.overseer.resolve_shard(shard);
        Ok(api.orders(resource.to_owned(), shard).await?)
    }

    /// Every order of the user, active or not (`GET /api/game/market/my-orders`): grouped by
    /// shard on screeps.com (intershard orders under `intershard`), one list on single-world
    /// servers.
    ///
    /// # Errors
    ///
    /// See [`Market`].
    pub async fn my_orders(&self) -> Result<MyOrders, Error> {
        Ok(self.overseer.user_api()?.my_orders().await?)
    }

    /// Daily price statistics of `resource` on `shard`, newest first
    /// (`GET /api/game/market/stats`).
    ///
    /// # Errors
    ///
    /// See [`Market`].
    pub async fn stats(
        &self,
        resource: &str,
        shard: Option<&str>,
    ) -> Result<Vec<MarketStatsEntry>, Error> {
        let api = self.overseer.user_api()?;
        let shard = self.overseer.resolve_shard(shard);
        Ok(api.stats(resource.to_owned(), shard).await?)
    }

    /// Page `page` (from 0) of the user's credit transactions, 20 per page and newest first;
    /// `has_more` tells whether page `page + 1` exists (`GET /api/user/money-history`).
    ///
    /// # Errors
    ///
    /// See [`Market`].
    pub async fn money_history(&self, page: u64) -> Result<MoneyHistory, Error> {
        Ok(self.overseer.user_api()?.money_history(page).await?)
    }
}
