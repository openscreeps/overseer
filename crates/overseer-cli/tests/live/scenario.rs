//! The CLI scenario: real `overseer` processes against the disposable world, for a new account.
//! Each assertion is a change, or the absence of a change, in the server's state after a
//! command. An `Overseer` session of the account only observes that state.

use std::collections::BTreeSet;

use overseer::model::account::UserWorldStatus;
use overseer::model::world::Tile;
use overseer::{Modules, Overseer};

use crate::observer::{OwnedSpawn, State, modules, names, settled, state};
use crate::support::{self, Account};
use crate::workspace::{MIRROR, PRIMARY, RADIUS, Workspace};

/// The main module of the first build: a live loop that writes to `Memory`.
const MAIN_V1: &str = "module.exports.loop = function () {\n  Memory.e2e = Game.time;\n};\n";
/// The main module of the second build.
const MAIN_V2: &str =
    "module.exports.loop = function () {\n  Memory.e2e = { build: 2, tick: Game.time };\n};\n";
/// The source map of the main module, a JSON map.
const MAP: &str =
    r#"{"version":3,"file":"main.js","sources":["main.ts"],"names":[],"mappings":"AAAA"}"#;
/// The smallest `WebAssembly` module: the magic and the version.
const WASM: &[u8] = b"\0asm\x01\0\0\0";

/// The commands in order, each followed by what it must have changed:
///
/// 1. `upload --dry-run` changes nothing.
/// 2. `poll --once --execute` does not place a spawn while the primary branch does not run in
///    the world.
/// 3. `upload` creates both branches, activates only the primary branch, and uploads the JS
///    module, the source map module and the `WebAssembly` module.
/// 4. `upload --profile primary` replaces the primary branch's modules and does not change the
///    mirror branch.
/// 5. `poll --once` without `--execute` changes nothing.
/// 6. `poll --once --execute` places `Spawn1` on the selector's tile, within the radius.
/// 7. A later `poll --once --execute` places no second spawn.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn upload_and_first_spawn() {
    if support::official() {
        eprintln!("skipped: the CLI scenario changes the world, so it does not run on screeps.com");
        return;
    }
    let account = Account::register().await;
    let observer = account.connect().await;
    let workspace = Workspace::new(account);
    workspace.write_build(MAIN_V1, MAP, Some(WASM));
    scenario(&workspace, &observer).await;
    observer.shutdown().await;
}

/// Runs the commands of [`upload_and_first_spawn`] and checks their outcomes.
async fn scenario(workspace: &Workspace, observer: &Overseer) {
    let initial = state(observer).await;
    assert_eq!(initial.status, UserWorldStatus::Empty, "a new account");
    assert!(
        initial.rooms.is_empty() && initial.spawns.is_empty(),
        "{initial:?}"
    );
    assert!(!initial.branches.contains(PRIMARY) && !initial.branches.contains(MIRROR));

    workspace.succeed(&["upload", "--dry-run"]);
    assert_eq!(
        state(observer).await,
        initial,
        "upload --dry-run changed the account"
    );

    // The primary branch does not run in the world yet, so poll must not place a spawn.
    workspace.succeed(&["poll", "--once", "--execute", "--profile", PRIMARY]);
    assert_eq!(
        state(observer).await,
        initial,
        "poll placed a spawn without an active branch"
    );

    uploads(workspace, observer, &initial).await;

    let start = observer
        .account()
        .start_room(None)
        .await
        .expect("start room")
        .expect("the server names a start room")
        .room;
    let before = state(observer).await;
    workspace.succeed(&["poll", "--once"]);
    assert_eq!(
        state(observer).await,
        before,
        "a read-only poll changed the account"
    );
    assert_eq!(before.status, UserWorldStatus::Empty);

    workspace.succeed(&["poll", "--once", "--execute"]);
    let placed = settled(observer).await;
    let spawn = placement(workspace, observer, &placed, &start).await;

    workspace.succeed(&["poll", "--once", "--execute", "--profile", PRIMARY]);
    let after = state(observer).await;
    assert_eq!(after.status, UserWorldStatus::Normal);
    assert_eq!(
        after.rooms, placed.rooms,
        "a second poll changed the owned rooms"
    );
    assert_eq!(
        after.spawns,
        vec![spawn],
        "a second poll placed another spawn"
    );
}

/// The upload of both profiles, then a second build to the primary profile only.
async fn uploads(workspace: &Workspace, observer: &Overseer, initial: &State) {
    workspace.succeed(&["upload"]);
    let uploaded = state(observer).await;
    assert!(
        uploaded.branches.contains(PRIMARY),
        "{:?}",
        uploaded.branches
    );
    assert!(
        uploaded.branches.contains(MIRROR),
        "{:?}",
        uploaded.branches
    );
    assert_eq!(
        uploaded.active_world,
        BTreeSet::from([PRIMARY.to_owned()]),
        "only the primary branch runs in the world"
    );
    assert_eq!(
        uploaded.status, initial.status,
        "an upload changed the world"
    );
    assert_eq!(uploaded.rooms, initial.rooms);
    let mirror = modules(observer, MIRROR).await;
    assert_first_build(&modules(observer, PRIMARY).await, PRIMARY);
    assert_first_build(&mirror, MIRROR);

    workspace.write_build(MAIN_V2, MAP, None);
    workspace.succeed(&["upload", "--profile", PRIMARY]);
    let primary = modules(observer, PRIMARY).await;
    assert_eq!(primary.js("main"), Some(MAIN_V2), "the second build's main");
    assert_eq!(primary.js("main.js.map"), Some(map_module().as_str()));
    assert_eq!(
        primary.kind("main_bg"),
        None,
        "the removed module stays on the branch"
    );
    assert_eq!(primary.len(), 2, "{:?}", names(&primary));
    assert_eq!(
        modules(observer, MIRROR).await,
        mirror,
        "the mirror branch changed"
    );
    let replaced = state(observer).await;
    assert_eq!(replaced.active_world, uploaded.active_world);
    assert_eq!(replaced.branches, uploaded.branches);
}

/// The one spawn after the placing poll: `Spawn1` of the account, on the selector's tile, in a
/// room within the radius of `start`, off walls.
async fn placement(
    workspace: &Workspace,
    observer: &Overseer,
    placed: &State,
    start: &str,
) -> OwnedSpawn {
    let [spawn] = placed.spawns.as_slice() else {
        panic!("one spawn of the account, not {:?}", placed.spawns);
    };
    assert_eq!(spawn.name.as_deref(), Some("Spawn1"));
    assert_eq!(
        placed.rooms,
        vec![spawn.room.clone()],
        "the spawn's room is the owned room"
    );
    let answer = workspace.answer().expect("the selector answered");
    assert_eq!(
        answer["room"].as_str(),
        Some(spawn.room.as_str()),
        "{answer}"
    );
    assert_eq!(answer["x"].as_u64(), Some(spawn.x), "{answer}");
    assert_eq!(answer["y"].as_u64(), Some(spawn.y), "{answer}");
    let from = room_xy(start).unwrap_or_else(|| panic!("start room {start:?}"));
    let to = room_xy(&spawn.room).unwrap_or_else(|| panic!("room {:?}", spawn.room));
    let radius = i64::from(RADIUS);
    assert!(
        (from.0 - to.0).abs() <= radius && (from.1 - to.1).abs() <= radius,
        "{} is not within {RADIUS} rooms of the start room {start}",
        spawn.room
    );
    assert!(
        (1..=48).contains(&spawn.x) && (1..=48).contains(&spawn.y),
        "{spawn:?}"
    );
    let terrain = observer
        .rooms()
        .terrain(&spawn.room, None)
        .await
        .expect("terrain");
    let tile = terrain.get(
        u8::try_from(spawn.x).expect("x"),
        u8::try_from(spawn.y).expect("y"),
    );
    assert_ne!(tile, Some(Tile::Wall), "{spawn:?}");
    spawn.clone()
}

/// The first build on `branch`: the main module as written, the source map as a module that
/// exports it, and the `WebAssembly` bytes.
fn assert_first_build(modules: &Modules, branch: &str) {
    assert_eq!(modules.len(), 3, "{branch}: {:?}", names(modules));
    assert_eq!(modules.js("main"), Some(MAIN_V1), "{branch}: main");
    assert_eq!(
        modules.js("main.js.map"),
        Some(map_module().as_str()),
        "{branch}: map"
    );
    let wasm = modules.wasm("main_bg").expect("base64 WebAssembly");
    assert_eq!(wasm.as_deref(), Some(WASM), "{branch}: main_bg");
}

/// The source map module as the server must keep it.
fn map_module() -> String {
    format!("module.exports = {MAP};")
}

/// The world coordinates of the room `name`. `W0` and `N0` are -1, `E0` and `S0` are 0. Returns
/// none for a name that is not a room name.
fn room_xy(name: &str) -> Option<(i64, i64)> {
    let (horizontal, rest) = name.split_at_checked(1)?;
    let (x, rest) = rest.split_at(rest.find(['N', 'S'])?);
    let (vertical, y) = rest.split_at(1);
    let (x, y): (i64, i64) = (x.parse().ok()?, y.parse().ok()?);
    let x = match horizontal {
        "W" => -x - 1,
        "E" => x,
        _ => return None,
    };
    Some((x, if vertical == "N" { -y - 1 } else { y }))
}
