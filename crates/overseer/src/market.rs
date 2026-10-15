//! The market: orders, statistics and money history.

use overseer_net::core::api::{endpoints, types};

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
    ) -> Result<Vec<types::MarketOrderIndexEntry>, Error> {
        let index = endpoints::MarketOrdersIndex {
            shard: self.overseer.resolve_shard(shard),
        };
        Ok(self.overseer.send(&index).await?.list)
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
    ) -> Result<Vec<types::MarketOrder>, Error> {
        let orders = endpoints::MarketOrders {
            resource_type: Some(resource.to_owned()),
            shard: self.overseer.resolve_shard(shard),
        };
        Ok(self.overseer.send(&orders).await?.list)
    }

    /// Every order of the user, active or not, grouped by shard (intershard orders under
    /// `intershard`) via `GET /api/game/market/my-orders`.
    ///
    /// # Errors
    ///
    /// See [`Market`].
    pub async fn my_orders(&self) -> Result<types::MarketMyOrdersResponse, Error> {
        self.overseer.send(&endpoints::MarketMyOrders).await
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
    ) -> Result<Vec<types::MarketStatsEntry>, Error> {
        let stats = endpoints::MarketStats {
            resource_type: Some(resource.to_owned()),
            shard: self.overseer.resolve_shard(shard),
        };
        Ok(self.overseer.send(&stats).await?.stats)
    }

    /// Page `page` (from 0) of the user's credit transactions, 20 per page and newest first;
    /// `has_more` tells whether page `page + 1` exists (`GET /api/user/money-history`).
    ///
    /// # Errors
    ///
    /// See [`Market`].
    pub async fn money_history(&self, page: u64) -> Result<types::UserMoneyHistoryResponse, Error> {
        let history = endpoints::UserMoneyHistory { page: Some(page) };
        self.overseer.send(&history).await
    }
}
