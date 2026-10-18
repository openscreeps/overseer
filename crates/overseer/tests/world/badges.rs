//! Player badges on the world map: the owners' and reservers' badge parameters from map
//! statistics, their SVG, and the session's shared badge service (one fetch per badge
//! version, failures remembered, within the request budget).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt as _;
use overseer::map::{Interest, RoomCoord, RoomRect, WorldEvent};
use overseer::model::account::{UserBadgeColor, UserBadgeType};
use overseer::net::{Priority, REQUEST_WINDOW};
use overseer::users::{BadgeKey, RETRY_FAILED};

use crate::server::{Request, Response, Ws};
use crate::{connect, query, soon};

fn room(name: &str) -> RoomCoord {
    name.parse().unwrap()
}

/// `W1N1` owned by Alice (`u1`, a standard badge), `W2N2` reserved by Bob (`u2`, a custom
/// badge), `W1N2` owned by Carol, whom the server does not describe; Alice's badge as SVG.
fn routes(request: &Request) -> (u16, String) {
    match request.path.as_str() {
        "/api/game/map-stats" => (
            200,
            r##"{"ok":1,"gameTime":7,"stats":{
                "W1N1":{"status":"normal","own":{"user":"u1","level":5}},
                "W2N2":{"status":"normal","own":{"user":"u2","level":0}},
                "W1N2":{"status":"normal","own":{"user":"u3","level":2}},
                "W2N1":{"status":"normal"}},
              "users":{
                "u1":{"_id":"u1","username":"Alice","badge":{"type":7,"color1":"#112233","color2":"#445566","color3":"#778899","param":12.5,"flip":false}},
                "u2":{"_id":"u2","username":"Bob","badge":{"type":{"path1":"M0 0L1 1","path2":"M1 0"},"color1":"#000000","color2":"#ffffff","color3":"#ff0000","flip":true}}}}"##
                .to_owned(),
        ),
        "/api/user/badge-svg" if query(request, "username") == Some("Alice") => {
            (200, "<svg>alice</svg>".to_owned())
        }
        _ => (404, r#"{"error":"not found"}"#.to_owned()),
    }
}

#[tokio::test]
async fn overviews_carry_the_badges_of_owners_and_reservers() {
    let (_server, overseer, _websocket) = connect(routes).await;
    let rooms = RoomRect::new(room("W2N2"), room("W1N1"));
    let mut world = overseer.map().world_with_bounds(None, rooms).unwrap();
    world.set_interest(&Interest {
        stats: Some(rooms),
        ..Interest::default()
    });
    let stats = soon(async {
        loop {
            if let WorldEvent::Stats(stats) = world.next().await.unwrap() {
                return stats;
            }
        }
    })
    .await;
    assert_eq!(stats.len(), 4);

    let owner = world.stat(room("W1N1")).unwrap().owner.clone().unwrap();
    let badge = owner.badge.unwrap();
    assert_eq!(badge.r#type, UserBadgeType::Integer(7));
    assert_eq!(
        (badge.color1, badge.param, badge.flip),
        (
            UserBadgeColor::String("#112233".to_owned()),
            Some(12.5),
            false
        )
    );
    let reservation = world
        .stat(room("W2N2"))
        .unwrap()
        .reservation
        .clone()
        .unwrap();
    assert_eq!(reservation.username.as_deref(), Some("Bob"));
    let badge = reservation.badge.unwrap();
    assert!(matches!(badge.r#type, UserBadgeType::CustomPaths(paths) if paths.path1 == "M0 0L1 1"));
    assert!(badge.flip);
    // A user the server did not describe has no badge.
    let unnamed = world.stat(room("W1N2")).unwrap().owner.clone().unwrap();
    assert_eq!((unnamed.username, unnamed.badge), (None, None));
}

#[tokio::test]
async fn badges_render_as_svg_by_username() {
    let (server, overseer, _websocket) = connect(routes).await;
    let map = overseer.map();
    assert_eq!(
        map.badge_svg("Alice", true).await.unwrap(),
        "<svg>alice</svg>"
    );
    assert!(map.badge_svg("Nobody", false).await.is_err());
    let queries = server
        .requests()
        .into_iter()
        .filter(|request| request.path == "/api/user/badge-svg")
        .map(|request| request.query)
        .collect::<Vec<_>>();
    assert_eq!(queries, ["username=Alice&border=1", "username=Nobody"]);
}

/// The key of `username`'s badge (player `user`) in version `hash`.
fn key(user: &str, username: &str, hash: u64) -> BadgeKey {
    BadgeKey {
        user: user.to_owned(),
        username: username.to_owned(),
        hash,
    }
}

/// The badge requests the server received, by username.
fn badge_requests(server: &crate::server::Server) -> Vec<String> {
    server
        .requests()
        .into_iter()
        .filter(|request| request.path == "/api/user/badge-svg")
        .filter_map(|request| query(&request, "username").map(str::to_owned))
        .collect()
}

/// Reads the client's frames until the socket closes, so that keep-alive pings get answers
/// while the clock jumps.
fn drain(mut websocket: Ws) {
    tokio::spawn(async move { while websocket.next().await.is_some() {} });
}

#[tokio::test]
async fn viewers_share_one_fetch_per_badge_version_and_failures_wait() {
    let (server, overseer, websocket) = connect(routes).await;
    drain(websocket);
    let badges = overseer.users().badges();
    let alice = key("u1", "Alice", 1);
    assert_eq!(badges.cached(&alice), None);
    // Two viewers at once (another handle of the same session): one request.
    let other = overseer.users().badges();
    let (first, second) = soon(async { tokio::join!(badges.svg(&alice), other.svg(&alice)) }).await;
    assert_eq!(first.as_deref(), Some("<svg>alice</svg>"));
    assert_eq!(second, first);
    assert_eq!(badges.cached(&alice), first);
    assert_eq!(badges.svg(&alice).await, first);
    assert_eq!(badge_requests(&server), ["Alice"]);

    // A changed badge is another version: fetched again, the old one is gone.
    let changed = key("u1", "Alice", 2);
    assert_eq!(badges.svg(&changed).await, first);
    assert_eq!(badges.cached(&alice), None);
    assert_eq!(badge_requests(&server), ["Alice", "Alice"]);

    // A badge that cannot be fetched is not asked again for a while.
    tokio::time::pause();
    let nobody = key("u9", "Nobody", 1);
    assert_eq!(badges.svg(&nobody).await, None);
    assert_eq!(badges.svg(&nobody).await, None);
    assert_eq!(badge_requests(&server), ["Alice", "Alice", "Nobody"]);
    tokio::time::advance(RETRY_FAILED).await;
    assert_eq!(badges.svg(&nobody).await, None);
    assert_eq!(
        badge_requests(&server),
        ["Alice", "Alice", "Nobody", "Nobody"]
    );
}

/// [`routes`] on a server with rate limits (the headers of `GET /api/game/world-size`).
fn limited(request: &Request) -> Response {
    if request.path == "/api/game/world-size" {
        let reset = (SystemTime::now() + Duration::from_mins(1))
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        return Response {
            status: 200,
            headers: vec![
                ("X-RateLimit-Limit", "120".to_owned()),
                ("X-RateLimit-Remaining", "119".to_owned()),
                ("X-RateLimit-Reset", reset.to_string()),
            ],
            body: br#"{"ok":1,"width":40,"height":40}"#.to_vec(),
        };
    }
    routes(request).into()
}

#[tokio::test]
async fn badge_fetches_wait_for_the_shared_request_budget() {
    let (server, overseer, websocket) = connect(limited).await;
    drain(websocket);
    tokio::time::pause();
    let client = overseer.client().clone();
    let first = tokio::time::Instant::now();
    // The user's own requests fill the share of visible work.
    for _ in 0..Priority::Visible.budget() {
        overseer.map().world_size(None).await.unwrap();
    }
    assert!(client.has_rate_limits());
    assert!(client.budget_ready_at(Priority::Visible, 0).is_some());
    let svg = overseer.users().badges().svg(&key("u1", "Alice", 1)).await;
    assert_eq!(svg.as_deref(), Some("<svg>alice</svg>"));
    // The fetch went once the first of those requests left the window.
    assert!(tokio::time::Instant::now() >= first + REQUEST_WINDOW);
    assert_eq!(badge_requests(&server), ["Alice"]);
}
