//! Live tests: portals found by the world map (candidate scans over `roomMap2`, destinations
//! from room objects) and the badges of room owners.
//!
//! The portal test creates a portal pair between `W0S0` and `E0S0` through the server's admin
//! API (`OVERSEER_LIVE_ADMIN_TOKEN`); worlds of 21 rooms and more also have the generator's
//! portals between their sector centres.

use std::collections::BTreeSet;
use std::time::Duration;

use overseer::core::api::types::{
    GameRoomObject, UserBadgeColor, UserBadgeInput, UserBadgeInputParam, UserBadgeInputType,
    UserBadgeType,
};
use overseer::map::{Interest, RoomCoord, RoomRect, WorldEvent, may_have_portals};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout};

use crate::support::{self, PATIENCE};

fn room(name: &str) -> RoomCoord {
    name.parse().expect("a room name")
}

/// Runs `action` (a JSON object) on the server's admin API (`POST /admin` with
/// `OVERSEER_LIVE_ADMIN_TOKEN`) and returns the response body.
async fn admin(action: &str) -> String {
    let token = std::env::var("OVERSEER_LIVE_ADMIN_TOKEN")
        .expect("OVERSEER_LIVE_ADMIN_TOKEN is not set: run `cargo xtask e2e`");
    let url = support::url().join("admin").expect("the admin URL");
    let host = url.host_str().expect("a host");
    let port = url.port_or_known_default().expect("a port");
    let mut stream = TcpStream::connect((host, port))
        .await
        .expect("the server accepts connections");
    let request = format!(
        "POST {} HTTP/1.1\r\nHost: {host}:{port}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{action}",
        url.path(),
        action.len()
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("the request is sent");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .expect("the response arrives");
    let (head, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
    assert!(
        head.starts_with("HTTP/1.1 2"),
        "the admin API refused {action}: {head}\n{body}"
    );
    body.to_owned()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live OpenScreeps server: cargo xtask e2e"]
async fn portals_are_found_in_candidate_rooms_with_their_destinations() {
    let overseer = support::register().await.connect().await;
    let map = overseer.map();
    let mut world = map.world(None).await.expect("the world opens");
    let bounds = world.bounds();
    let (west, east) = (room("W0S0"), room("E0S0"));
    assert!(bounds.contains(west) && bounds.contains(east), "{bounds:?}");
    let created = admin(r#"{"action":"portal-create","from":"W0S0","to":"E0S0"}"#).await;
    assert!(created.contains(r#""from":"W0S0""#), "{created}");

    // The server lists the new portals in the rooms' map views once it processed them.
    let mut views = map.view(None).expect("signed in");
    views.set_rooms([west.to_string(), east.to_string()]);
    timeout(PATIENCE, async {
        loop {
            let listed = views
                .views()
                .values()
                .filter(|view| view.p.as_ref().is_some_and(|p| !p.is_empty()))
                .count();
            if listed == 2 {
                return;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the map views list the portals");

    // Every candidate room of the world is scanned and reported once.
    let candidates = bounds
        .iter()
        .filter(|&room| may_have_portals(room))
        .collect::<BTreeSet<_>>();
    world.set_interest(&Interest {
        portals: Some(RoomRect::new(
            RoomCoord {
                x: bounds.min.x - 5,
                y: bounds.min.y - 5,
            },
            RoomCoord {
                x: bounds.max.x + 5,
                y: bounds.max.y + 5,
            },
        )),
        ..Interest::default()
    });
    assert_eq!(world.progress().portals_wanted, candidates.len());
    let mut reported = Vec::new();
    // A scan that a busy server does not answer within its timeout is tried again 30 s later.
    timeout(PATIENCE * 3, async {
        while reported.len() < candidates.len() {
            if let WorldEvent::Portals(room, portals) =
                world.next().await.expect("the session stays open")
            {
                assert!(portals.iter().all(|portal| portal.room == room), "{room}");
                reported.push(room);
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("every candidate scanned in time: {reported:?} of {candidates:?}"));
    assert_eq!(
        reported.iter().copied().collect::<BTreeSet<_>>(),
        candidates
    );
    assert_eq!(reported.len(), candidates.len(), "each room once");
    let progress = world.progress();
    assert_eq!(progress.portals_scanned, progress.portals_wanted);

    // The pair: a cluster in each room leading to the other.
    for (from, to) in [(west, east), (east, west)] {
        let portals = world.portals(from).expect("scanned");
        assert!(portals.len() >= 8, "{from}: {portals:?}");
        let objects = overseer
            .rooms()
            .objects(&from.to_string(), None)
            .await
            .expect("room objects");
        let listed = objects
            .objects
            .iter()
            .filter(|object| matches!(object, GameRoomObject::Portal(_)))
            .count();
        assert_eq!(portals.len(), listed, "{from}: every portal parses");
        let leading = portals
            .iter()
            .filter(|portal| portal.destination.room == to)
            .collect::<Vec<_>>();
        assert!(leading.len() >= 8, "{from}: {portals:?}");
        for portal in leading {
            assert_eq!(portal.destination.shard, None);
            assert!(portal.destination.x.is_some() && portal.destination.y.is_some());
            assert_eq!((portal.expires_at, portal.decay_time), (None, None));
        }
    }
    // The generator links the sector centres of the world (from 21 rooms on) in a cycle: each
    // has a cluster leading to another centre.
    let ends_in_5 = |value: i32| (if value < 0 { -value - 1 } else { value }) % 10 == 5;
    let centres = candidates
        .iter()
        .copied()
        .filter(|room| ends_in_5(room.x) && ends_in_5(room.y))
        .collect::<Vec<_>>();
    if centres.len() > 1 {
        for centre in &centres {
            let portals = world.portals(*centre).expect("scanned");
            assert!(
                portals
                    .iter()
                    .any(|portal| portal.destination.room != *centre
                        && centres.contains(&portal.destination.room)),
                "{centre}: {portals:?}"
            );
        }
    }
    // Every portal of the world leads into it.
    for room in &candidates {
        for portal in world.portals(*room).expect("scanned").iter() {
            assert!(bounds.contains(portal.destination.room), "{portal:?}");
        }
    }
    // `Map::portals` reads the same.
    let direct = map
        .portals(&west.to_string(), None)
        .await
        .expect("room objects");
    assert_eq!(*world.portals(west).expect("scanned"), *direct);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live OpenScreeps server: cargo xtask e2e"]
async fn room_owners_carry_their_badges() {
    let player = support::player().await;
    let overseer = player.account.connect().await;
    overseer
        .account()
        .set_badge(UserBadgeInput {
            color1: "#123456".to_owned(),
            color2: "#abcdef".to_owned(),
            color3: "#fedcba".to_owned(),
            flip: Some(true),
            param: UserBadgeInputParam::Number(-30.0),
            r#type: UserBadgeInputType::Integer(12),
        })
        .await
        .expect("the badge is saved");

    let home = room(&player.room);
    let rooms = RoomRect::new(home, home);
    let map = overseer.map();
    let mut world = map.world_with_bounds(None, rooms).expect("the world opens");
    world.set_interest(&Interest {
        stats: Some(rooms),
        ..Interest::default()
    });
    timeout(PATIENCE, async {
        while !matches!(world.next().await.expect("open"), WorldEvent::Stats(_)) {}
    })
    .await
    .expect("the overview in time");
    let owner = world
        .stat(home)
        .expect("an overview")
        .owner
        .clone()
        .expect("an owner");
    assert_eq!(
        owner.username.as_deref(),
        Some(player.account.username.as_str())
    );
    let badge = owner.badge.expect("the owner's badge");
    assert_eq!(badge.r#type, UserBadgeType::Integer(12));
    assert_eq!(badge.color1, UserBadgeColor::String("#123456".to_owned()));
    assert_eq!((badge.param, badge.flip), (Some(-30.0), true));

    let svg = map
        .badge_svg(&player.account.username, false)
        .await
        .expect("the badge renders");
    assert!(svg.contains("<svg"), "not an SVG document: {svg}");
    let unknown = map.badge_svg(&support::unique("nobody"), false).await;
    assert!(unknown.is_err(), "{unknown:?}");
}
