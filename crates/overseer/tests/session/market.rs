//! The market: request shapes on a sharded server.

use overseer::core::api::types::MarketOrderType;

use crate::server::{Request, Server};
use crate::{connect, routes};

/// `GET /api/version` of a sharded server (default shard `shard0`).
const SHARDED_VERSION: &str = r#"{"ok":1,"protocol":14,"users":1,"serverData":{"customObjectTypes":{},"historyChunkSize":100,"shards":["shard0","shard3"],"renderer":{"resources":{},"metadata":{}}}}"#;

/// One sell order of energy.
const ORDER: &str = r#"{"_id":"o1","active":true,"amount":100,"created":5,"price":0.5,"remainingAmount":80,"resourceType":"energy","roomName":"W1N1","totalAmount":100,"type":"sell","user":"u2"}"#;

/// The market routes of a sharded server over [`routes`].
fn market_routes(request: &Request) -> (u16, String) {
    match (request.method.as_str(), request.path.as_str()) {
        (_, "/api/version") => (200, SHARDED_VERSION.to_owned()),
        ("GET", "/api/game/market/orders-index") => (
            200,
            r#"{"ok":1,"list":[{"_id":"energy","count":2,"buying":1,"selling":1}]}"#.to_owned(),
        ),
        ("GET", "/api/game/market/orders") => (200, format!(r#"{{"ok":1,"list":[{ORDER}]}}"#)),
        ("GET", "/api/game/market/my-orders") => (
            200,
            format!(r#"{{"ok":1,"shards":{{"shard3":[{ORDER}],"intershard":[]}}}}"#),
        ),
        ("GET", "/api/game/market/stats") => (
            200,
            r#"{"ok":1,"stats":[{"avgPrice":0.4,"date":"2026-10-14","resourceType":"energy","stddevPrice":0.1,"transactions":7,"volume":900}]}"#
                .to_owned(),
        ),
        ("GET", "/api/user/money-history") => (
            200,
            r#"{"ok":1,"page":2,"hasMore":false,"list":[]}"#.to_owned(),
        ),
        _ => routes(request),
    }
}

#[tokio::test]
async fn market_requests_name_the_default_or_given_shard() {
    let mut server = Server::start(market_routes, |_| true).await;
    let (overseer, _websocket) = connect(&mut server).await;
    let market = overseer.market();

    let index = market.orders_index(None).await.unwrap();
    assert_eq!(index[0].id, "energy");
    assert_eq!(index[0].count, 2);
    let orders = market.orders("energy", Some("shard3")).await.unwrap();
    assert_eq!(orders[0].r#type, MarketOrderType::Sell);
    assert_eq!(orders[0].remaining_amount, 80);
    let mine = market.my_orders().await.unwrap();
    assert_eq!(mine.shards.unwrap()["shard3"].len(), 1);
    let stats = market.stats("energy", None).await.unwrap();
    assert_eq!(stats[0].transactions, 7);
    let history = market.money_history(2).await.unwrap();
    assert_eq!(history.page, 2);
    assert!(!history.has_more);

    let sent: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|request| {
            request.path.starts_with("/api/game/market/")
                || request.path == "/api/user/money-history"
        })
        .map(|request| (request.path, request.query))
        .collect();
    let expected = [
        ("/api/game/market/orders-index", "shard=shard0"),
        (
            "/api/game/market/orders",
            "resourceType=energy&shard=shard3",
        ),
        ("/api/game/market/my-orders", ""),
        ("/api/game/market/stats", "resourceType=energy&shard=shard0"),
        ("/api/user/money-history", "page=2"),
    ]
    .map(|(path, query)| (path.to_owned(), query.to_owned()));
    assert_eq!(sent, expected);
}
