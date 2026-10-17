//! Live tests: rooms (`Rooms`): live snapshots, room information and the cheap room actions.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use overseer::model::platform::Subsystem;
use overseer::model::world::{GameBuildableStructureType, GameRoomObject, Tile};
use overseer::{Error, FlagColor, RoomSnapshot, StatInterval, Watch, net};
use tokio::time::timeout;

use crate::support::{self, PATIENCE};

/// Waits for the first snapshot `done` accepts, checking that every snapshot decodes.
async fn until(
    watch: &mut Watch<RoomSnapshot>,
    what: &str,
    mut done: impl FnMut(&RoomSnapshot) -> bool,
) -> Arc<RoomSnapshot> {
    timeout(PATIENCE, async {
        loop {
            let snapshot = watch.changed().await.expect("the session stays open");
            assert!(
                snapshot.undecoded.is_empty(),
                "undecodable objects in {}: {:?}",
                snapshot.room,
                snapshot.undecoded
            );
            if done(&snapshot) {
                return snapshot;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

/// The creeps of `user` in `snapshot`: id → position.
fn creeps(snapshot: &RoomSnapshot, user: &str) -> BTreeMap<String, (u64, u64)> {
    snapshot
        .objects
        .iter()
        .filter(|object| matches!(object, GameRoomObject::Creep(_)) && object.user() == Some(user))
        .map(|object| (object.id().to_owned(), (object.x(), object.y())))
        .collect()
}

/// The only object of `kind` at `at`, if any.
fn object_at<'a>(
    snapshot: &'a RoomSnapshot,
    kind: &str,
    at: (u64, u64),
) -> Option<&'a GameRoomObject> {
    snapshot
        .objects
        .iter()
        .find(|object| object.kind() == kind && (object.x(), object.y()) == at)
}

/// Asserts what every snapshot of a player's room shows: the spawn, the claimed controller,
/// sources and the owner.
fn assert_players_room(snapshot: &RoomSnapshot, player: &support::Player, user: &str) {
    let spawn = (u64::from(player.spawn.0), u64::from(player.spawn.1));
    let Some(GameRoomObject::Spawn(spawn_fields)) = object_at(snapshot, "spawn", spawn) else {
        panic!("no spawn at {spawn:?}: {:?}", snapshot.objects);
    };
    assert_eq!(spawn_fields.name.as_deref(), Some("Spawn1"));
    assert_eq!(spawn_fields.user, user);
    let controller = snapshot
        .objects
        .iter()
        .find_map(|object| match object {
            GameRoomObject::Controller(controller) => Some(controller),
            _ => None,
        })
        .expect("the room has a controller");
    assert_eq!(controller.user.as_deref(), Some(user));
    assert!(controller.level >= 1, "level {}", controller.level);
    assert!(
        snapshot
            .objects
            .iter()
            .any(|object| matches!(object, GameRoomObject::Source(_))),
        "no sources"
    );
    let owner = snapshot
        .users
        .get(user)
        .expect("the owner is among the users");
    assert_eq!(owner.username, player.account.username);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn watching_a_players_room_yields_typed_snapshots_that_advance() {
    let player = support::player().await;
    let overseer = player.account.connect().await;
    let user = overseer.user_id().expect("signed in").to_owned();
    let mut watch = overseer
        .rooms()
        .watch(&player.room)
        .await
        .expect("the room is watched");

    let first = until(&mut watch, "the first tick", |snapshot| {
        snapshot.game_time.is_some()
    })
    .await;
    assert_eq!(first.room, player.room);
    assert_eq!(first.shard, None);
    assert_eq!(first.subscription_error, None);
    assert_eq!(
        first.terrain.get(player.spawn.0, player.spawn.1),
        Some(Tile::Plain)
    );
    assert_players_room(&first, &player, &user);

    // Creeps appear and wander; the game time never goes back.
    let mut time = first.game_time;
    let mut seen: BTreeMap<String, BTreeSet<(u64, u64)>> = BTreeMap::new();
    let last = until(&mut watch, "a creep to move", |snapshot| {
        assert!(
            snapshot.game_time >= time,
            "{:?} after {time:?}",
            snapshot.game_time
        );
        time = snapshot.game_time;
        assert_players_room(snapshot, &player, &user);
        for (id, at) in creeps(snapshot, &user) {
            seen.entry(id).or_default().insert(at);
        }
        seen.values().any(|positions| positions.len() > 1)
    })
    .await;
    assert!(last.game_time > first.game_time);
    let names = last
        .objects
        .iter()
        .filter_map(|object| match object {
            GameRoomObject::Creep(creep) => Some(creep.name.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        names.iter().all(|name| name.starts_with('c')),
        "creeps of the bot are named c<tick>: {names:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn room_information_describes_the_players_room() {
    let player = support::player().await;
    let overseer = player.account.connect().await;
    let user = overseer.user_id().expect("signed in").to_owned();
    let rooms = overseer.rooms();

    let objects = rooms
        .objects(&player.room, None)
        .await
        .expect("room objects decode");
    let spawn = (u64::from(player.spawn.0), u64::from(player.spawn.1));
    assert!(
        objects.objects.iter().any(|object| object.kind() == "spawn"
            && (object.x(), object.y()) == spawn
            && object.user() == Some(&user)),
        "no spawn at {spawn:?}: {:?}",
        objects.objects
    );
    assert!(objects.objects.iter().any(|object| matches!(
        object,
        GameRoomObject::Controller(controller) if controller.user.as_deref() == Some(&user)
    )));
    assert_eq!(
        objects
            .users
            .get(&user)
            .and_then(|owner| owner.username.as_deref()),
        Some(player.account.username.as_str())
    );

    let terrain = rooms
        .terrain(&player.room, None)
        .await
        .expect("terrain decodes");
    assert_eq!(
        terrain.get(player.spawn.0, player.spawn.1),
        Some(Tile::Plain)
    );

    let status = rooms.status(&player.room, None).await.expect("room status");
    assert_eq!(status.status.as_deref(), Some("normal"));

    for interval in [
        StatInterval::EightMinutes,
        StatInterval::ThreeHours,
        StatInterval::OneDay,
    ] {
        let overview = rooms
            .overview(&player.room, interval, None)
            .await
            .unwrap_or_else(|error| panic!("room overview over {interval:?}: {error}"));
        let owner = overview.owner.expect("the room has an owner");
        assert_eq!(
            owner.username.as_deref(),
            Some(player.account.username.as_str())
        );
    }

    let decorations = rooms.decorations(&player.room, None).await;
    // The community contract has no decorations (`@screeps/backend` 3.3.0 routes no
    // `/api/game/room-decorations`).
    let has_decorations = overseer.world().supports(Subsystem::RoomDecorations);
    assert_eq!(has_decorations, !support::community());
    if has_decorations {
        let decorations = decorations.expect("room decorations decode");
        assert!(decorations.is_empty(), "{decorations:?}");
    } else {
        // Overseer does not ask.
        assert!(
            matches!(
                &decorations,
                Err(Error::Net(net::Error::Unsupported(
                    Subsystem::RoomDecorations
                )))
            ),
            "{decorations:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn flags_are_created_recoloured_and_removed() {
    let player = support::player().await;
    let overseer = player.account.connect().await;
    let rooms = overseer.rooms();
    let mut watch = rooms
        .watch(&player.room)
        .await
        .expect("the room is watched");
    let name = rooms.unique_flag_name(None).await.expect("a flag name");
    assert!(
        rooms
            .is_flag_name_free(&name, None)
            .await
            .expect("a name check")
    );
    let (x, y) = (player.spawn.0 + 2, player.spawn.1 + 2);

    rooms
        .create_flag(
            &player.room,
            x,
            y,
            &name,
            (FlagColor::Red, FlagColor::White),
            None,
        )
        .await
        .expect("the flag is created");
    let flag = |snapshot: &RoomSnapshot| {
        snapshot
            .flags
            .iter()
            .find(|flag| flag.name == name)
            .map(|flag| (flag.color, flag.secondary_color, flag.x, flag.y))
    };
    let (red, white) = (FlagColor::Red.code(), FlagColor::White.code());
    let at = (u64::from(x), u64::from(y));
    until(&mut watch, "the flag", |snapshot| {
        flag(snapshot) == Some((red, white, at.0, at.1))
    })
    .await;
    assert!(
        !rooms
            .is_flag_name_free(&name, None)
            .await
            .expect("a name check")
    );

    rooms
        .change_flag_colors(
            &player.room,
            &name,
            (FlagColor::Blue, FlagColor::Green),
            None,
        )
        .await
        .expect("the flag is recoloured");
    let (blue, green) = (FlagColor::Blue.code(), FlagColor::Green.code());
    until(&mut watch, "the recoloured flag", |snapshot| {
        flag(snapshot) == Some((blue, green, at.0, at.1))
    })
    .await;

    rooms
        .remove_flag(&player.room, &name, None)
        .await
        .expect("the flag is removed");
    until(&mut watch, "the flag to go", |snapshot| {
        snapshot.game_time.is_some() && flag(snapshot).is_none()
    })
    .await;
    assert!(
        rooms
            .is_flag_name_free(&name, None)
            .await
            .expect("a name check")
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn construction_sites_are_placed_and_removed() {
    let player = support::player().await;
    let overseer = player.account.connect().await;
    let user = overseer.user_id().expect("signed in").to_owned();
    let rooms = overseer.rooms();
    let mut watch = rooms
        .watch(&player.room)
        .await
        .expect("the room is watched");
    let (x, y) = (player.spawn.0 - 2, player.spawn.1 - 2);
    let at = (u64::from(x), u64::from(y));

    let reported = rooms
        .create_construction_site(
            &player.room,
            x,
            y,
            GameBuildableStructureType::Road,
            None,
            None,
        )
        .await
        .expect("the construction site is placed");
    let snapshot = until(&mut watch, "the construction site", |snapshot| {
        object_at(snapshot, "constructionSite", at).is_some()
    })
    .await;
    let Some(GameRoomObject::ConstructionSite(site)) = object_at(&snapshot, "constructionSite", at)
    else {
        panic!("not a construction site");
    };
    assert_eq!(site.structure_type, "road");
    assert_eq!(site.user, user);
    assert_eq!(site.room, player.room);
    if let Some(id) = &reported {
        assert_eq!(id, &site.id);
    }

    rooms
        .remove_construction_site(&player.room, &site.id, None)
        .await
        .expect("the removal is queued");
    until(&mut watch, "the construction site to go", |snapshot| {
        object_at(snapshot, "constructionSite", at).is_none()
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn rooms_of_other_users_and_npcs_are_watched() {
    let player = support::player().await;
    let owner = player.account.connect().await;
    let owner_id = owner.user_id().expect("signed in").to_owned();
    owner.shutdown().await;
    let visitor = support::register().await.connect().await;
    let rooms = visitor.rooms();

    let mut theirs = rooms
        .watch(&player.room)
        .await
        .expect("the room is watched");
    let snapshot = until(&mut theirs, "the other player's room", |snapshot| {
        snapshot.game_time.is_some()
    })
    .await;
    assert_players_room(&snapshot, &player, &owner_id);
    assert!(snapshot.flags.is_empty(), "{:?}", snapshot.flags);

    // The world's stronghold: an invader core with ramparts, next to keeper lairs.
    let stronghold = support::stronghold(visitor.client()).await;
    let mut npc = rooms.watch(&stronghold).await.expect("the room is watched");
    let snapshot = until(&mut npc, "the stronghold room", |snapshot| {
        snapshot.game_time.is_some()
    })
    .await;
    let kinds = snapshot
        .objects
        .iter()
        .map(GameRoomObject::kind)
        .collect::<BTreeSet<_>>();
    for kind in ["invaderCore", "rampart", "keeperLair", "source", "mineral"] {
        assert!(kinds.contains(kind), "no {kind} in {stronghold}: {kinds:?}");
    }
    assert!(snapshot.objects.iter().any(
        |object| matches!(object, GameRoomObject::InvaderCore(_)) && object.user() == Some("2")
    ));
    let invader = snapshot.users.get("2").expect("the invader user is sent");
    assert_eq!(invader.username, "Invader");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn watching_more_rooms_than_the_limit_reports_the_refusal() {
    let overseer = support::register().await.connect().await;
    let rooms = overseer.rooms();
    // The server sends a user at most two rooms per update; the one left out (chosen at
    // random every update) gets `err@room:…`.
    let names = support::world(overseer.client())
        .await
        .stats
        .into_keys()
        .take(3)
        .collect::<Vec<_>>();
    let mut watches = Vec::new();
    for room in &names {
        watches.push(rooms.watch(room).await.expect("the room is watched"));
    }
    let [first, second, third] = &mut watches[..] else {
        unreachable!("three watches");
    };
    let refused = timeout(PATIENCE, async {
        loop {
            let snapshot = tokio::select! {
                snapshot = first.changed() => snapshot,
                snapshot = second.changed() => snapshot,
                snapshot = third.changed() => snapshot,
            }
            .expect("the session stays open");
            if snapshot.subscription_error.is_some() {
                return snapshot;
            }
        }
    })
    .await
    .expect("a subscription is refused");
    assert_eq!(
        refused.subscription_error.as_deref(),
        Some("subscribe limit reached")
    );
    assert!(names.contains(&refused.room), "{}", refused.room);
}
