//! Market area: the order index, orders per resource, the user's own orders, price statistics
//! and the credit history.

use overseer_core::api::{community, official, openscreeps};
use overseer_core::model::market::{
    MarketOrder, MarketOrderIndexEntry, MarketStatsEntry, MoneyHistory, MyOrders,
};

use super::{Api, Community, Official, OpenScreeps, delegate};
use crate::Error;

/// The market, as the signed-in user sees it. Prices are in credits; `shard` names the shard
/// on screeps.com and is ignored by single-world servers.
pub trait MarketApi {
    /// One entry per resource type with active orders on `shard`: order count and buy/sell
    /// split (`GET /api/game/market/orders-index`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn orders_index(
        &self,
        shard: Option<String>,
    ) -> impl Future<Output = Result<Vec<MarketOrderIndexEntry>, Error>>;

    /// The active buy and sell orders for `resource` (for example `energy`, `H` or `pixel`)
    /// on `shard` (`GET /api/game/market/orders`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn orders(
        &self,
        resource: String,
        shard: Option<String>,
    ) -> impl Future<Output = Result<Vec<MarketOrder>, Error>>;

    /// Every order of the user, active or not (`GET /api/game/market/my-orders`): grouped by
    /// shard on screeps.com, one list on single-world servers.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn my_orders(&self) -> impl Future<Output = Result<MyOrders, Error>>;

    /// Daily price statistics of `resource` on `shard`, newest first
    /// (`GET /api/game/market/stats`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn stats(
        &self,
        resource: String,
        shard: Option<String>,
    ) -> impl Future<Output = Result<Vec<MarketStatsEntry>, Error>>;

    /// Page `page` (from 0) of the user's credit transactions, 20 per page and newest first
    /// (`GET /api/user/money-history`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn money_history(&self, page: u64) -> impl Future<Output = Result<MoneyHistory, Error>>;
}

impl MarketApi for Api {
    async fn orders_index(
        &self,
        shard: Option<String>,
    ) -> Result<Vec<MarketOrderIndexEntry>, Error> {
        delegate!(self, api => api.orders_index(shard).await)
    }

    async fn orders(
        &self,
        resource: String,
        shard: Option<String>,
    ) -> Result<Vec<MarketOrder>, Error> {
        delegate!(self, api => api.orders(resource, shard).await)
    }

    async fn my_orders(&self) -> Result<MyOrders, Error> {
        delegate!(self, api => api.my_orders().await)
    }

    async fn stats(
        &self,
        resource: String,
        shard: Option<String>,
    ) -> Result<Vec<MarketStatsEntry>, Error> {
        delegate!(self, api => api.stats(resource, shard).await)
    }

    async fn money_history(&self, page: u64) -> Result<MoneyHistory, Error> {
        delegate!(self, api => api.money_history(page).await)
    }
}

/// `impl MarketApi` for the per-contract API `$api` sending the endpoints of `$contract`;
/// every contract has the market's endpoints with the same parameters.
macro_rules! market_api {
    ($api:ty, $contract:ident) => {
        impl MarketApi for $api {
            async fn orders_index(
                &self,
                shard: Option<String>,
            ) -> Result<Vec<MarketOrderIndexEntry>, Error> {
                let index = $contract::endpoints::MarketOrdersIndex { shard };
                Ok(self.client.send(&index).await?.body.list)
            }

            async fn orders(
                &self,
                resource: String,
                shard: Option<String>,
            ) -> Result<Vec<MarketOrder>, Error> {
                let orders = $contract::endpoints::MarketOrders {
                    resource_type: Some(resource),
                    shard,
                };
                Ok(self.client.send(&orders).await?.body.list)
            }

            async fn my_orders(&self) -> Result<MyOrders, Error> {
                let reply = self
                    .client
                    .send(&$contract::endpoints::MarketMyOrders)
                    .await?;
                Ok(reply.body.into())
            }

            async fn stats(
                &self,
                resource: String,
                shard: Option<String>,
            ) -> Result<Vec<MarketStatsEntry>, Error> {
                let stats = $contract::endpoints::MarketStats {
                    resource_type: Some(resource),
                    shard,
                };
                Ok(self.client.send(&stats).await?.body.stats)
            }

            async fn money_history(&self, page: u64) -> Result<MoneyHistory, Error> {
                let history = $contract::endpoints::UserMoneyHistory { page: Some(page) };
                Ok(self.client.send(&history).await?.body.into())
            }
        }
    };
}

market_api!(Official, official);
market_api!(OpenScreeps, openscreeps);
market_api!(Community, community);
