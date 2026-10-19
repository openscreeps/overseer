//! What the account has in the world and in code, as an `Overseer` session of the account
//! reads it. The observer only reads: every change comes from the CLI.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use overseer::model::account::UserWorldStatus;
use overseer::model::world::GameRoomObject;
use overseer::{Modules, Overseer};

/// How long the observer waits for the world to show a placed spawn.
const SETTLE: Duration = Duration::from_secs(30);

/// One owned spawn as the observer sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedSpawn {
    /// The object ID.
    pub id: String,
    /// The spawn name.
    pub name: Option<String>,
    /// The room.
    pub room: String,
    /// The x coordinate in the room.
    pub x: u64,
    /// The y coordinate in the room.
    pub y: u64,
}

/// The account's world status, branches, owned rooms and spawns.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    /// The world status.
    pub status: UserWorldStatus,
    /// The names of all branches.
    pub branches: BTreeSet<String>,
    /// The names of the branches that run in the world.
    pub active_world: BTreeSet<String>,
    /// The owned rooms, sorted.
    pub rooms: Vec<String>,
    /// The owned spawns in the owned rooms.
    pub spawns: Vec<OwnedSpawn>,
}

/// Reads the account's world status, branches, owned rooms and spawns.
///
/// # Panics
///
/// When a read fails.
pub async fn state(observer: &Overseer) -> State {
    let id = observer.user_id().expect("a signed-in observer").to_owned();
    let status = observer
        .account()
        .world_status()
        .await
        .expect("world status");
    let listed = observer.code().branches().await.expect("branches");
    let branches = listed.iter().map(|branch| branch.branch.clone()).collect();
    let active_world = listed
        .iter()
        .filter(|branch| branch.active_world == Some(true))
        .map(|branch| branch.branch.clone())
        .collect();
    let mut rooms: Vec<String> = observer
        .users()
        .rooms(&id)
        .await
        .expect("owned rooms")
        .into_iter()
        .map(|room| room.room)
        .collect();
    rooms.sort();
    let mut spawns = Vec::new();
    for room in &rooms {
        let objects = observer
            .rooms()
            .objects(room, None)
            .await
            .expect("room objects");
        spawns.extend(
            objects
                .objects
                .into_iter()
                .filter_map(|object| match object {
                    GameRoomObject::Spawn(spawn) if spawn.user == id => Some(OwnedSpawn {
                        id: spawn.id,
                        name: spawn.name,
                        room: spawn.room,
                        x: spawn.x,
                        y: spawn.y,
                    }),
                    _ => None,
                }),
        );
    }
    State {
        status,
        branches,
        active_world,
        rooms,
        spawns,
    }
}

/// The state once the world shows the placed spawn. Waits at most [`SETTLE`].
///
/// # Panics
///
/// When a read fails or the world status does not become `normal` in time.
pub async fn settled(observer: &Overseer) -> State {
    let deadline = Instant::now() + SETTLE;
    loop {
        let read = state(observer).await;
        if read.status == UserWorldStatus::Normal || Instant::now() >= deadline {
            assert_eq!(
                read.status,
                UserWorldStatus::Normal,
                "poll --execute placed no spawn"
            );
            return read;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// The modules of `branch`.
///
/// # Panics
///
/// When the read fails.
pub async fn modules(observer: &Overseer, branch: &str) -> Modules {
    observer
        .code()
        .modules(branch)
        .await
        .expect("branch modules")
}

/// The module names of `modules`.
pub fn names(modules: &Modules) -> Vec<&str> {
    modules.iter().map(|(name, _)| name).collect()
}
