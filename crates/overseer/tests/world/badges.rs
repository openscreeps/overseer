//! Player badges on the world map: the owners' and reservers' badge parameters from map
//! statistics, and their SVG.

use overseer::map::{Interest, RoomCoord, RoomRect, WorldEvent};
use overseer::model::account::{UserBadgeColor, UserBadgeType};

use crate::server::Request;
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
