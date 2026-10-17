//! Other players and leaderboards: request shapes.

use overseer::StatInterval;
use overseer::model::users::LeaderboardMode;

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

    let bob = users.find("bob").await.unwrap();
    assert_eq!(bob.id, "u2");
    assert_eq!(bob.username.as_deref(), Some("Bob"));
    assert_eq!(users.find_by_id("u2").await.unwrap(), bob);
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
