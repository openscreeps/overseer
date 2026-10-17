//! Market: the order index, orders per resource, the user's own orders, price statistics and
//! the credit history.

use std::collections::BTreeMap;

use crate::api::community::types as community;
use crate::api::official::types as official;
use crate::api::openscreeps::types as openscreeps;

/// Every contract's order index entries, orders (and their buy/sell type) and daily price
/// statistics are these types (codegen re-exports the official ones from the others).
pub use crate::api::official::types::{
    MarketOrder, MarketOrderIndexEntry, MarketOrderType, MarketStatsEntry,
};

/// Every contract's credit transaction kinds and the new orders, price changes and extensions
/// of fee entries are these types (codegen re-exports the official ones from the others).
pub use crate::api::official::types::{
    UserMoneyHistoryEntryType, UserMoneyHistoryMarketInfoChangeOrderPrice,
    UserMoneyHistoryMarketInfoExtendOrder, UserMoneyHistoryMarketInfoOrder,
    UserMoneyHistoryMarketInfoOrderType,
};

/// Every order of the user, active or not (`GET /api/game/market/my-orders`).
#[derive(Clone, Debug, PartialEq)]
pub enum MyOrders {
    /// screeps.com: the orders of each shard, keyed by shard name, with intershard (account
    /// resource) orders under `intershard`.
    Sharded(BTreeMap<String, Vec<MarketOrder>>),
    /// A single-world server (`OpenScreeps`, community): every order.
    Single(Vec<MarketOrder>),
}

impl MyOrders {
    /// Every order with the shard it belongs to: `None` on a single-world server, `intershard`
    /// for intershard orders.
    pub fn iter(&self) -> impl Iterator<Item = (Option<&str>, &MarketOrder)> {
        let (shards, single) = match self {
            Self::Sharded(shards) => (Some(shards), None),
            Self::Single(orders) => (None, Some(orders)),
        };
        let sharded = shards.into_iter().flatten().flat_map(|(shard, orders)| {
            orders
                .iter()
                .map(move |order| (Some(shard.as_str()), order))
        });
        sharded.chain(single.into_iter().flatten().map(|order| (None, order)))
    }

    /// The number of orders.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Sharded(shards) => shards.values().map(Vec::len).sum(),
            Self::Single(orders) => orders.len(),
        }
    }

    /// Whether the user has no order.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl From<official::MarketMyOrdersResponse> for MyOrders {
    fn from(reply: official::MarketMyOrdersResponse) -> Self {
        Self::Sharded(reply.shards)
    }
}

/// `From` the flat `my-orders` reply of the single-world contracts.
macro_rules! my_orders_from {
    ($($reply:ty),+) => {$(
        impl From<$reply> for MyOrders {
            fn from(reply: $reply) -> Self {
                Self::Single(reply.list)
            }
        }
    )+};
}

my_orders_from!(
    openscreeps::MarketMyOrdersResponse,
    community::MarketMyOrdersResponse
);

/// One page of the user's credit transactions (`GET /api/user/money-history`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MoneyHistory {
    /// The page, from 0.
    pub page: u64,
    /// Whether page `page + 1` exists.
    pub has_more: bool,
    /// The transactions, newest first, 20 per page.
    pub list: Vec<MoneyEntry>,
}

/// One credit transaction (`users.money`).
#[derive(Clone, Debug, PartialEq)]
pub struct MoneyEntry {
    /// The entry's id (`_id`).
    pub id: String,
    /// What the transaction was: a purchase, a sale or an order fee.
    pub kind: UserMoneyHistoryEntryType,
    /// The user's credits after the transaction.
    pub balance: f64,
    /// Credits gained (positive) or spent (negative).
    pub change: f64,
    /// When it happened (ISO 8601).
    pub date: String,
    /// The shard of the transaction, when reported.
    pub shard: Option<String>,
    /// The game time of the transaction, when reported.
    pub tick: Option<i64>,
    /// The id of the user it belongs to.
    pub user: String,
    /// The market details.
    pub market: MoneyMarket,
}

/// The market details of a credit transaction: a filled deal (resource, rooms, owner,
/// dealer, price, amount), a new order (`order`, fee entries), a price change or an extension.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MoneyMarket {
    /// The traded amount.
    pub amount: Option<f64>,
    /// The price per unit, in credits.
    pub price: Option<f64>,
    /// The traded resource.
    pub resource_type: Option<String>,
    /// The room of the order.
    pub room_name: Option<String>,
    /// The room of the dealer.
    pub target_room_name: Option<String>,
    /// The id of the order's creator.
    pub owner: Option<String>,
    /// The id of the user who executed the deal.
    pub dealer: Option<String>,
    /// The id of the counterparty (`OpenScreeps` only).
    pub another_user: Option<String>,
    /// The id of the related order (`OpenScreeps` only).
    pub order_id: Option<String>,
    /// Whether the counterparty was an NPC terminal.
    pub npc: Option<bool>,
    /// The order created (fee entries).
    pub order: Option<UserMoneyHistoryMarketInfoOrder>,
    /// A price change (fee entries).
    pub change_order_price: Option<UserMoneyHistoryMarketInfoChangeOrderPrice>,
    /// An extension of an order (fee entries).
    pub extend_order: Option<UserMoneyHistoryMarketInfoExtendOrder>,
}

/// `From` the money-history page and entry of each contract: the official one (shared by the
/// community contract) and the `OpenScreeps` one, whose market details differ (see
/// [`MoneyMarket`]'s conversions).
macro_rules! money_history_from {
    ($($module:ident),+) => {$(
        impl From<$module::UserMoneyHistoryResponse> for MoneyHistory {
            fn from(reply: $module::UserMoneyHistoryResponse) -> Self {
                Self {
                    page: reply.page,
                    has_more: reply.has_more,
                    list: reply.list.into_iter().map(MoneyEntry::from).collect(),
                }
            }
        }

        impl From<$module::UserMoneyHistoryEntry> for MoneyEntry {
            fn from(entry: $module::UserMoneyHistoryEntry) -> Self {
                Self {
                    id: entry.id,
                    kind: entry.r#type,
                    balance: entry.balance,
                    change: entry.change,
                    date: entry.date,
                    shard: entry.shard,
                    tick: entry.tick,
                    user: entry.user,
                    market: entry.market.into(),
                }
            }
        }
    )+};
}

money_history_from!(official, openscreeps);

impl From<official::UserMoneyHistoryMarketInfo> for MoneyMarket {
    fn from(info: official::UserMoneyHistoryMarketInfo) -> Self {
        Self {
            amount: info.amount,
            price: info.price,
            resource_type: info.resource_type,
            room_name: info.room_name,
            target_room_name: info.target_room_name,
            owner: info.owner,
            dealer: info.dealer,
            another_user: None,
            order_id: None,
            npc: info.npc,
            order: info.order,
            change_order_price: info.change_order_price,
            extend_order: info.extend_order,
        }
    }
}

/// `OpenScreeps` adds the counterparty (`anotherUser`) and the order id (`orderId`).
impl From<openscreeps::UserMoneyHistoryMarketInfo> for MoneyMarket {
    fn from(info: openscreeps::UserMoneyHistoryMarketInfo) -> Self {
        Self {
            amount: info.amount,
            price: info.price,
            resource_type: info.resource_type,
            room_name: info.room_name,
            target_room_name: info.target_room_name,
            owner: info.owner,
            dealer: info.dealer,
            another_user: info.another_user,
            order_id: info.order_id,
            npc: info.npc,
            order: info.order,
            change_order_price: info.change_order_price,
            extend_order: info.extend_order,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MarketOrderType, MoneyHistory, MyOrders, UserMoneyHistoryEntryType,
        UserMoneyHistoryMarketInfoChangeOrderPrice, UserMoneyHistoryMarketInfoExtendOrder,
        UserMoneyHistoryMarketInfoOrder, UserMoneyHistoryMarketInfoOrderType,
    };
    use crate::api::community::types as community;
    use crate::api::official::types as official;
    use crate::api::openscreeps::types as openscreeps;

    /// An active sell order as the servers list it.
    const ORDER: &str = r#"{"_id":"o1","active":true,"type":"sell","amount":80,"remainingAmount":80,"totalAmount":100,"price":0.5,"resourceType":"energy","roomName":"W1N1","created":120,"createdTimestamp":1760000000000,"user":"u1"}"#;

    #[test]
    fn my_orders_keep_screeps_com_shards_and_the_flat_list_of_single_worlds() {
        let official: official::MarketMyOrdersResponse = serde_json::from_str(&format!(
            r#"{{"ok":1,"shards":{{"shard3":[{ORDER}],"intershard":[]}}}}"#
        ))
        .unwrap();
        let sharded = MyOrders::from(official);
        assert_eq!(sharded.len(), 1);
        let (shard, order) = sharded.iter().next().unwrap();
        assert_eq!(shard, Some("shard3"));
        assert_eq!(order.r#type, MarketOrderType::Sell);
        let MyOrders::Sharded(shards) = &sharded else {
            panic!("{sharded:?}");
        };
        assert_eq!(shards["intershard"], []);

        let flat = format!(r#"{{"ok":1,"list":[{ORDER}]}}"#);
        let openscreeps: openscreeps::MarketMyOrdersResponse = serde_json::from_str(&flat).unwrap();
        let community: community::MarketMyOrdersResponse = serde_json::from_str(&flat).unwrap();
        for single in [MyOrders::from(openscreeps), MyOrders::from(community)] {
            let (shard, order) = single.iter().next().unwrap();
            assert_eq!(shard, None);
            assert_eq!(order.remaining_amount, 80);
            assert!(matches!(single, MyOrders::Single(orders) if orders.len() == 1));
        }
        assert!(MyOrders::Single(Vec::new()).is_empty());
    }

    #[test]
    fn money_history_converts_both_contracts_and_keeps_openscreeps_extras() {
        // A deal, a new order's fee, a price change and an extension.
        let page = r#"{"ok":1,"page":1,"hasMore":true,"list":[
            {"_id":"h1","date":"2026-10-14T12:00:00.000Z","tick":500,"user":"u1","type":"market.sell","balance":1040,"change":40,"shard":"shard0",
             "market":{"resourceType":"energy","roomName":"W1N1","targetRoomName":"W2N2","price":0.4,"amount":100,"owner":"u1","dealer":"u2","npc":false,
                       "anotherUser":"u2","orderId":"o1"}},
            {"_id":"h2","date":"2026-10-14T11:00:00.000Z","user":"u1","type":"market.fee","balance":1000,"change":-2.5,
             "market":{"order":{"type":"buy","resourceType":"H","price":2,"totalAmount":25,"roomName":"W1N1"}}},
            {"_id":"h3","date":"2026-10-14T10:00:00.000Z","user":"u1","type":"market.fee","balance":1002.5,"change":-1,
             "market":{"changeOrderPrice":{"orderId":"o1","oldPrice":0.4,"newPrice":0.5}}},
            {"_id":"h4","date":"2026-10-14T09:00:00.000Z","user":"u1","type":"market.fee","balance":1003.5,"change":-1,
             "market":{"extendOrder":{"orderId":"o1","addAmount":50}}}
        ]}"#;

        let official: official::UserMoneyHistoryResponse = serde_json::from_str(page).unwrap();
        let community: community::UserMoneyHistoryResponse = serde_json::from_str(page).unwrap();
        let openscreeps: openscreeps::UserMoneyHistoryResponse =
            serde_json::from_str(page).unwrap();
        let official = MoneyHistory::from(official);
        let openscreeps = MoneyHistory::from(openscreeps);
        assert_eq!(MoneyHistory::from(community), official);

        for history in [&official, &openscreeps] {
            assert_eq!((history.page, history.has_more), (1, true));
            let kinds: Vec<_> = history.list.iter().map(|entry| entry.kind).collect();
            assert_eq!(
                kinds,
                [
                    UserMoneyHistoryEntryType::MarketSell,
                    UserMoneyHistoryEntryType::MarketFee,
                    UserMoneyHistoryEntryType::MarketFee,
                    UserMoneyHistoryEntryType::MarketFee
                ]
            );
            let deal = &history.list[0];
            assert!((deal.change - 40.0).abs() < f64::EPSILON);
            assert_eq!(deal.shard.as_deref(), Some("shard0"));
            assert_eq!(deal.tick, Some(500));
            assert_eq!(deal.market.dealer.as_deref(), Some("u2"));
            assert_eq!(deal.market.target_room_name.as_deref(), Some("W2N2"));
            assert_eq!(
                history.list[1].market.order,
                Some(UserMoneyHistoryMarketInfoOrder {
                    r#type: Some(UserMoneyHistoryMarketInfoOrderType::Buy),
                    resource_type: Some("H".to_owned()),
                    price: Some(2.0),
                    total_amount: Some(25.0),
                    room_name: Some("W1N1".to_owned()),
                })
            );
            assert_eq!(
                history.list[2].market.change_order_price,
                Some(UserMoneyHistoryMarketInfoChangeOrderPrice {
                    order_id: "o1".to_owned(),
                    old_price: 0.4,
                    new_price: 0.5,
                })
            );
            assert_eq!(
                history.list[3].market.extend_order,
                Some(UserMoneyHistoryMarketInfoExtendOrder {
                    order_id: "o1".to_owned(),
                    add_amount: 50.0,
                })
            );
        }
        // Only the OpenScreeps contract has the counterparty and the order id.
        assert_eq!(official.list[0].market.another_user, None);
        assert_eq!(
            openscreeps.list[0].market.another_user.as_deref(),
            Some("u2")
        );
        assert_eq!(openscreeps.list[0].market.order_id.as_deref(), Some("o1"));
    }
}
