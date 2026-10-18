//! Other players and leaderboards: request shapes, unknown users on each contract, and the
//! room holdings of sharded and single-world servers.

use std::collections::BTreeMap;

use overseer::model::platform::{Plugins, ServerKind};
use overseer::model::users::{LeaderboardMode, RoomHoldings};
use overseer::{Auth, Overseer, ServerProfile, StatInterval};

use crate::server::{Request, Server};
use crate::{connect, routes};

/// Bob's public profile.
const BOB: &str = r##"{"_id":"u2","username":"Bob","gcl":1000,"badge":{"color1":"#ff0000","color2":3,"color3":"#0000ff","flip":false,"param":10,"type":4}}"##;

/// The user and leaderboard routes over [`routes`].
fn user_routes(request: &Request) -> (u16, String) {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/api/user/find") => (200, format!(r#"{{"ok":1,"user":{BOB}}}"#)),
        ("GET", "/api/user/rooms") => (200, r#"{"ok":1,"rooms":["W2N2","W3N2"]}"#.to_owned()),
        ("GET", "/api/user/stats") => (
            200,
            r#"{"ok":1,"stats":{"energyHarvested":120,"creepsLost":2}}"#.to_owned(),
        ),
        ("GET", "/api/user/badge-svg") => (200, "<svg>bob</svg>".to_owned()),
        ("GET", "/api/leaderboard/list") => (
            200,
            format!(
                r#"{{"ok":1,"count":31,"list":[{{"_id":"r1","rank":20,"score":5000,"season":"2026-10","user":"u2"}}],"users":{{"u2":{BOB}}}}}"#
            ),
        ),
        ("GET", "/api/leaderboard/find") => (
            200,
            r#"{"ok":1,"list":[{"_id":"r1","rank":20,"score":5000,"season":"2026-10","user":"u2"},{"_id":"r0","rank":3,"score":900,"season":"2026-09","user":"u2"}]}"#
                .to_owned(),
        ),
        ("GET", "/api/leaderboard/seasons") => (
            200,
            r#"{"ok":1,"seasons":[{"_id":"2026-10","name":"October 2026","date":"2026-10-01T00:00:00.000Z"},{"_id":"2026-09","name":"September 2026"}]}"#
                .to_owned(),
        ),
        _ => routes(request),
    }
}

#[tokio::test]
async fn user_and_leaderboard_requests_have_the_documented_shape() {
    let mut server = Server::start(user_routes, |_| true).await;
    let (overseer, _websocket) = connect(&mut server).await;
    let users = overseer.users();

    let bob = users.find("bob").await.unwrap().unwrap();
    assert_eq!(bob.id, "u2");
    assert_eq!(bob.username.as_deref(), Some("Bob"));
    assert_eq!(users.find_by_id("u2").await.unwrap(), Some(bob));
    let rooms = users.rooms("u2").await.unwrap();
    let rooms: Vec<_> = rooms
        .iter()
        .map(|room| (room.shard.as_deref(), room.room.as_str()))
        .collect();
    assert_eq!(rooms, [(None, "W2N2"), (None, "W3N2")]);
    let stats = users.stats("u2", StatInterval::OneDay).await.unwrap();
    assert_eq!(stats.len(), 2);
    assert_eq!(
        users.badge_svg("Bob", false).await.unwrap(),
        "<svg>bob</svg>"
    );

    let page = users
        .leaderboard(LeaderboardMode::World, Some("2026-10"), 20, 10)
        .await
        .unwrap();
    assert_eq!(page.count, 31);
    assert_eq!(page.list[0].rank, 20);
    assert_eq!(page.users["u2"].username.as_deref(), Some("Bob"));
    let all = users
        .leaderboard_find(LeaderboardMode::Power, "Bob", None)
        .await
        .unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[1].season, "2026-09");
    let seasons = users.leaderboard_seasons().await.unwrap();
    let ids: Vec<_> = seasons.iter().map(|season| season.id.as_str()).collect();
    assert_eq!(ids, ["2026-10", "2026-09"]);

    let sent: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|request| {
            request.path.starts_with("/api/user/") || request.path.starts_with("/api/leaderboard/")
        })
        .map(|request| (request.path, request.query))
        .collect();
    let expected = [
        ("/api/user/find", "username=bob"),
        ("/api/user/find", "id=u2"),
        ("/api/user/rooms", "id=u2"),
        ("/api/user/stats", "id=u2&interval=1440"),
        ("/api/user/badge-svg", "username=Bob"),
        (
            "/api/leaderboard/list",
            "mode=world&season=2026-10&limit=10&offset=20",
        ),
        ("/api/leaderboard/find", "mode=power&username=Bob"),
        ("/api/leaderboard/seasons", ""),
    ]
    .map(|(path, query)| (path.to_owned(), query.to_owned()));
    assert_eq!(sent, expected);
}

/// `GET /api/user/find` answering each lookup of `query` with its error string: `ghost` the
/// contract's own for an unknown user (`unknown`), `stranger` the one of another contract.
fn lookup_routes(unknown: &'static str, other: &'static str) -> impl Fn(&Request) -> (u16, String) {
    move |request| match request.path.as_str() {
        "/api/user/find" if request.query.ends_with("=ghost") => {
            (200, format!(r#"{{"error":"{unknown}"}}"#))
        }
        "/api/user/find" => (200, format!(r#"{{"error":"{other}"}}"#)),
        _ => routes(request),
    }
}

#[tokio::test]
async fn unknown_users_are_none_in_the_error_string_of_each_contract() {
    const NOT_FOUND: &str = "user not found";
    const TYPE_ERROR: &str = "TypeError: Cannot read properties of null (reading 'steam')";
    for (kind, unknown, other) in [
        (ServerKind::Official, NOT_FOUND, TYPE_ERROR),
        (ServerKind::OpenScreeps, NOT_FOUND, TYPE_ERROR),
        (ServerKind::Community, TYPE_ERROR, NOT_FOUND),
    ] {
        let server = Server::start(lookup_routes(unknown, other), |_| true).await;
        let profile = ServerProfile {
            url: server.url(),
            auth: Auth::Anonymous,
            server_password: None,
            default_shard: None,
            server_kind: Some(kind),
            plugins: Some(Plugins::NONE),
        };
        let users = Overseer::connect(profile).await.unwrap().users();
        assert_eq!(users.find("ghost").await.unwrap(), None, "{kind:?}");
        assert_eq!(users.find_by_id("ghost").await.unwrap(), None, "{kind:?}");
        // Another contract's string is a failure, not an unknown user.
        let error = users.find("stranger").await.unwrap_err();
        assert_eq!(error.server_message(), Some(other), "{kind:?}");
    }
}

/// The `GET /api/user/rooms` replies of a sharded server: `reservations` only for the flag,
/// and an error for user `ghost`.
fn sharded_rooms_routes(request: &Request) -> (u16, String) {
    match (request.path.as_str(), request.query.as_str()) {
        ("/api/user/rooms", "id=ghost&reservation=") => {
            (200, r#"{"error":"invalid params"}"#.to_owned())
        }
        ("/api/user/rooms", "id=u2") => (
            200,
            r#"{"ok":1,"shards":{"shard0":["W1N1"],"shard3":[]}}"#.to_owned(),
        ),
        ("/api/user/rooms", "id=u2&reservation=") => (
            200,
            r#"{"ok":1,"shards":{"shard0":["W1N1"],"shard3":[]},"reservations":{"shard0":[],"shard3":["W9N9"]}}"#
                .to_owned(),
        ),
        _ => routes(request),
    }
}

#[tokio::test]
async fn room_holdings_keep_reservations_and_empty_shards_and_errors() {
    let server = Server::start(sharded_rooms_routes, |_| true).await;
    let profile = ServerProfile {
        url: server.url(),
        auth: Auth::Anonymous,
        server_password: None,
        default_shard: None,
        server_kind: Some(ServerKind::Official),
        plugins: Some(Plugins::NONE),
    };
    let users = Overseer::connect(profile).await.unwrap().users();
    let shards = |entries: &[(&str, &[&str])]| -> BTreeMap<String, Vec<String>> {
        entries
            .iter()
            .map(|(shard, rooms)| {
                let rooms = rooms.iter().map(|&room| room.to_owned()).collect();
                ((*shard).to_owned(), rooms)
            })
            .collect()
    };
    let owned = shards(&[("shard0", &["W1N1"]), ("shard3", &[])]);

    assert_eq!(
        users.room_holdings("u2", true).await.unwrap(),
        RoomHoldings::Sharded {
            owned: owned.clone(),
            reserved: Some(shards(&[("shard0", &[]), ("shard3", &["W9N9"])])),
        }
    );
    // Without the flag the reply has no report of reserved rooms.
    assert_eq!(
        users.room_holdings("u2", false).await.unwrap(),
        RoomHoldings::Sharded {
            owned,
            reserved: None,
        }
    );
    let rooms = users.rooms("u2").await.unwrap();
    assert_eq!(rooms.len(), 1);
    assert_eq!(rooms[0].shard.as_deref(), Some("shard0"));
    // A server error stays an error.
    let error = users.room_holdings("ghost", true).await.unwrap_err();
    assert_eq!(error.server_message(), Some("invalid params"));
}

#[tokio::test]
async fn single_world_room_holdings_have_no_reservations() {
    let mut server = Server::start(user_routes, |_| true).await;
    let (overseer, _websocket) = connect(&mut server).await;
    assert_eq!(
        overseer.users().room_holdings("u2", true).await.unwrap(),
        RoomHoldings::SingleWorld {
            owned: vec!["W2N2".to_owned(), "W3N2".to_owned()],
        }
    );
}
