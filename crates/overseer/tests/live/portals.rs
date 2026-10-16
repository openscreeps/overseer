//! Live tests: portals found by the world map (every room scanned over `roomMap2`, destinations
//! from room objects) and the badges of room owners.
//!
//! The portal test creates a portal pair between `W0S0` and `E0S0` through the server's admin
//! API (`OVERSEER_LIVE_ADMIN_TOKEN`); worlds of 21 rooms and more also have the generator's
//! portals between their sector centres.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use overseer::core::api::types::{
    GameRoomObject, UserBadgeColor, UserBadgeInput, UserBadgeInputParam, UserBadgeInputType,
    UserBadgeType,
};
use overseer::map::{Interest, RoomCoord, RoomRect, WorldEvent};
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

    // Every room of the world is scanned and reported once; a room with portals is reported
    // with pending destinations when scanned (unless its pair tells them at once), and again
    // once they are fetched or inferred in the background.
    let candidates = bounds.iter().collect::<BTreeSet<_>>();
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
    let mut reported: BTreeMap<RoomCoord, Vec<bool>> = BTreeMap::new();
    // OpenScreeps answers scans behind its ticks (about a second each when many wait); a scan
    // it does not answer within its timeout is tried again 30 s later.
    timeout(PATIENCE * 20, async {
        loop {
            // Until every room is reported with its destinations (the world's progress runs
            // ahead of the events still queued).
            let progress = world.progress();
            if progress.is_complete()
                && progress.portals_pending == 0
                && reported.len() == candidates.len()
                && reported
                    .values()
                    .all(|reports| reports.last() == Some(&false))
            {
                return;
            }
            if let WorldEvent::Portals(room, portals) =
                world.next().await.expect("the session stays open")
            {
                assert!(portals.iter().all(|portal| portal.room == room), "{room}");
                let pending = portals.iter().any(|portal| portal.destination.is_none());
                reported.entry(room).or_default().push(pending);
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("every room scanned in time: {reported:?} of {candidates:?}"));
    assert_eq!(
        reported.keys().copied().collect::<BTreeSet<_>>(),
        candidates
    );
    for (room, reports) in &reported {
        // At most one pending report (the scan), then the destinations: never scanned twice.
        // Inferred portals may be reported again when a fetch already in flight replaces them.
        let pending = reports.iter().take_while(|&&pending| pending).count();
        assert!(
            pending <= 1 && reports[pending..].iter().all(|&pending| !pending),
            "{room}: {reports:?}"
        );
    }
    let progress = world.progress();
    assert_eq!(progress.portals_scanned, progress.portals_wanted);

    // Every portal of the world lands exactly on a portal of its destination room that leads
    // back to it, with the same lifetime: the created pair, and the generator's pairs of
    // sector centres (from 21 rooms on; with an odd count one centre has none).
    let lands_back = |from: RoomCoord, to: RoomCoord| {
        let there = world.portals(to).expect("scanned");
        world
            .portals(from)
            .expect("scanned")
            .iter()
            .filter_map(|portal| Some((portal, portal.destination.as_ref()?)))
            .filter(|(_, destination)| destination.room == to)
            .all(|(portal, destination)| {
                there.iter().any(|back| {
                    let Some(home) = &back.destination else {
                        return false;
                    };
                    (Some(back.x), Some(back.y)) == (destination.x, destination.y)
                        && home.room == from
                        && (home.x, home.y) == (Some(portal.x), Some(portal.y))
                        && (back.expires_at, back.decay_time)
                            == (portal.expires_at, portal.decay_time)
                })
            })
    };
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
        assert_eq!(portals.len(), listed, "{from}: every portal known");
        let leading = portals
            .iter()
            .filter_map(|portal| portal.destination.as_ref())
            .filter(|destination| destination.room == to)
            .collect::<Vec<_>>();
        assert!(leading.len() >= 8, "{from}: {portals:?}");
        for destination in leading {
            assert_eq!(destination.shard, None);
            assert!(destination.x.is_some() && destination.y.is_some());
        }
        assert!(lands_back(from, to), "{from} -> {to}: {portals:?}");
    }
    let ends_in_5 = |value: i32| (if value < 0 { -value - 1 } else { value }) % 10 == 5;
    let centres = candidates
        .iter()
        .copied()
        .filter(|room| ends_in_5(room.x) && ends_in_5(room.y))
        .collect::<Vec<_>>();
    let mut unpaired = 0;
    for centre in &centres {
        let portals = world.portals(*centre).expect("scanned");
        let partners = portals
            .iter()
            .filter_map(|portal| portal.destination.as_ref())
            .map(|destination| destination.room)
            .filter(|to| to != centre && centres.contains(to))
            .collect::<BTreeSet<_>>();
        match partners.iter().next() {
            None => unpaired += 1,
            Some(&partner) => {
                assert_eq!(partners.len(), 1, "{centre}: {portals:?}");
                assert!(lands_back(*centre, partner), "{centre} -> {partner}");
                assert!(lands_back(partner, *centre), "{partner} -> {centre}");
            }
        }
    }
    assert!(
        unpaired <= centres.len() % 2,
        "{unpaired} of {centres:?} unpaired"
    );
    // Every portal of the world leads into it.
    for room in &candidates {
        for portal in world.portals(*room).expect("scanned").iter() {
            let destination = portal.destination.as_ref().expect("fetched or inferred");
            assert!(bounds.contains(destination.room), "{portal:?}");
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
