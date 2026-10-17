//! Live tests: room history playback (`History`, `HistoryPlayer`).

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use overseer::core::api::ApiError;
use overseer::core::api::types::GameRoomObject;
use overseer::{Error, RoomSnapshot, Seek, net};
use tokio::time::{sleep, timeout};

use crate::support::{self, PATIENCE};

/// The creeps of `user` in `snapshot`: id → position.
fn creeps(snapshot: &RoomSnapshot, user: &str) -> BTreeMap<String, (u64, u64)> {
    snapshot
        .objects
        .iter()
        .filter(|object| matches!(object, GameRoomObject::Creep(_)) && object.user() == Some(user))
        .map(|object| (object.id().to_owned(), (object.x(), object.y())))
        .collect()
}

/// The player replays a chunk of its room tick by tick, steps across chunks and reports ticks
/// that are not kept.
///
/// The community server keeps no room history: `@screeps/backend` 3.3.0 answers every
/// `/room-history` request with HTTP 500 `not implemented` unless a mod replaces its
/// `onGetRoomHistory` (`lib/game/server.js`,
/// <https://github.com/screeps/backend-local/blob/9d079282303ec04e577ac2bb97f64312b25e4ccd/lib/game/server.js#L34-L36>),
/// and the world under test runs none. There the player reports that refusal and stays put.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn history_replays_the_players_room() {
    let player = support::player().await;
    let overseer = player.account.connect().await;
    let user = overseer.user_id().expect("signed in").to_owned();
    let size = overseer.world().history_chunk_size();
    assert_eq!(size, 20, "the server advertises its chunk size");
    if support::community() {
        let mut history = overseer
            .history()
            .player(&player.room, None)
            .expect("a history player");
        let refused = history.seek(size).await.expect_err("no history is served");
        assert!(
            matches!(
                &refused,
                Error::Net(net::Error::Api(ApiError::Status { status, .. })) if status.as_u16() == 500
            ),
            "{refused:?}"
        );
        assert_eq!(history.tick(), None);
        assert!(history.snapshot().is_none());
        return;
    }
    let mut room = overseer
        .rooms()
        .watch(&player.room)
        .await
        .expect("the room is watched");

    // The first chunk that starts after a creep exists, and the time it is complete. Ticks pass
    // at the server's pace, slower while the whole suite loads it: each tick must come within
    // `PATIENCE`, and the wait is bounded in ticks.
    let (mut first, mut base) = (None, None);
    loop {
        let snapshot = timeout(PATIENCE, room.changed())
            .await
            .expect("the room advances")
            .expect("the session stays open");
        let Some(time) = snapshot.game_time else {
            continue;
        };
        let first = *first.get_or_insert(time);
        assert!(
            time < first + 10 * size,
            "a creep and two history chunks within {} ticks",
            10 * size
        );
        if base.is_none() && !creeps(&snapshot, &user).is_empty() {
            base = Some(time - time % size + size);
        }
        // The next chunk has begun: this one is complete.
        if base.is_some_and(|base| time > base + size) {
            break;
        }
    }
    drop(room);
    let base = base.expect("a creep was seen");

    let mut history = overseer
        .history()
        .player(&player.room, None)
        .expect("a history player");
    assert_eq!(history.room(), player.room);
    assert_eq!(history.shard(), None);
    assert_eq!(history.tick(), None);
    assert!(history.snapshot().is_none());
    // Chunks are written as their ticks are processed; give the server a moment.
    timeout(PATIENCE, async {
        while history.seek(base).await.expect("history decodes") == Seek::NotKept {
            sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("the chunk is kept");
    assert_eq!(history.tick(), Some(base));

    let spawn = (u64::from(player.spawn.0), u64::from(player.spawn.1));
    let mut positions: BTreeMap<String, BTreeSet<(u64, u64)>> = BTreeMap::new();
    for tick in base..base + size {
        if tick > base {
            assert_eq!(history.step().await.expect("history decodes"), Seek::Moved);
        }
        assert_eq!(history.tick(), Some(tick));
        let snapshot = history.snapshot().expect("a replayed state");
        assert_eq!(snapshot.game_time, Some(tick));
        assert_eq!(snapshot.room, player.room);
        assert!(
            snapshot.undecoded.is_empty(),
            "tick {tick}: undecodable objects {:?}",
            snapshot.undecoded
        );
        assert!(
            snapshot
                .objects
                .iter()
                .any(|object| object.kind() == "spawn"
                    && (object.x(), object.y()) == spawn
                    && object.user() == Some(&user)),
            "tick {tick}: no spawn at {spawn:?}"
        );
        let creeps = creeps(&snapshot, &user);
        assert!(!creeps.is_empty(), "tick {tick}: no creeps");
        for (id, at) in creeps {
            positions.entry(id).or_default().insert(at);
        }
        let state = history.state().expect("a replayed state");
        assert!(state.game_time().is_some_and(|changed| changed <= tick));
    }
    assert!(
        positions.values().any(|seen| seen.len() > 1),
        "the creeps wander during the chunk: {positions:?}"
    );

    // Stepping past the chunk loads the next one; stepping back replays this one.
    assert_eq!(history.step().await.expect("history decodes"), Seek::Moved);
    assert_eq!(history.tick(), Some(base + size));
    assert_eq!(
        history.step_back().await.expect("history decodes"),
        Seek::Moved
    );
    assert_eq!(history.tick(), Some(base + size - 1));

    // Ticks far ahead are not kept, and the player stays where it was.
    let ahead = history
        .seek(base + 1000 * size)
        .await
        .expect("a missing chunk is no error");
    assert_eq!(ahead, Seek::NotKept);
    assert_eq!(history.tick(), Some(base + size - 1));
}
