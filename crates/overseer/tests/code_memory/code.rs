//! Branch requests, directory push and pull, and the code feeds.

use overseer::code::{ActiveBranchChange, ActiveBranches, CodeChange, ModuleKind};
use overseer::core::socket::Channel;
use overseer::core::socket::types::UserCodeHash;
use overseer::{CodeTarget, Error, Modules};
use serde_json::json;

use crate::server::{self, Server};
use crate::{TempDir, WASM, WASM_BASE64, bodies, connect, queries, receive, routes, soon};

#[tokio::test]
async fn branches_and_modules_are_read() {
    let mut server = Server::start(routes, |_| true).await;
    let (overseer, _websocket) = connect(&mut server).await;
    let code = overseer.code();

    let branches = code.branches().await.unwrap();
    assert_eq!(
        branches
            .iter()
            .map(|branch| branch.branch.as_str())
            .collect::<Vec<_>>(),
        ["default", "sim"]
    );
    assert_eq!(
        code.active_branches().await.unwrap(),
        ActiveBranches {
            world: Some("default".to_owned()),
            simulation: Some("sim".to_owned()),
        }
    );

    let modules = code.modules("next").await.unwrap();
    assert_eq!(modules.js("main"), Some("module.exports.loop = () => {};"));
    assert_eq!(modules.wasm("engine").unwrap().as_deref(), Some(&WASM[..]));
    let (branch, active) = code.active_modules(CodeTarget::Simulation).await.unwrap();
    assert_eq!((branch.as_str(), &active), ("sim", &modules));
    assert_eq!(
        queries(&server, "/api/user/branches"),
        ["withCode=false", "withCode=false"]
    );
    let code_queries = queries(&server, "/api/user/code");
    assert_eq!(code_queries.len(), 2);
    assert_eq!(code_queries[0], "branch=next");
    assert!(
        ["branch=%24activeSim", "branch=$activeSim"].contains(&code_queries[1].as_str()),
        "{}",
        code_queries[1]
    );

    overseer.shutdown().await;
    assert!(matches!(code.branches().await, Err(Error::Closed)));
    assert!(matches!(code.changes(), Err(Error::Closed)));
}

#[tokio::test]
async fn branch_changes_carry_the_documented_bodies() {
    let mut server = Server::start(routes, |_| true).await;
    let (overseer, _websocket) = connect(&mut server).await;
    let code = overseer.code();
    let mut modules = Modules::new();
    modules.insert_js("main", "loop");
    modules.insert_wasm("engine", &WASM);

    code.set_modules("default", modules.clone()).await.unwrap();
    code.clone_branch("default", "next").await.unwrap();
    code.create_branch("fresh", modules).await.unwrap();
    code.set_active_branch(CodeTarget::World, "next")
        .await
        .unwrap();
    code.set_active_branch(CodeTarget::Simulation, "fresh")
        .await
        .unwrap();
    code.delete_branch("default").await.unwrap();

    let wire = json!({ "main": "loop", "engine": { "binary": WASM_BASE64 } });
    assert_eq!(
        bodies(&server, "POST", "/api/user/code"),
        [json!({ "branch": "default", "modules": wire })]
    );
    assert_eq!(
        bodies(&server, "POST", "/api/user/clone-branch"),
        [
            json!({ "branch": "default", "newName": "next" }),
            json!({ "defaultModules": wire, "newName": "fresh" }),
        ]
    );
    assert_eq!(
        bodies(&server, "POST", "/api/user/set-active-branch"),
        [
            json!({ "activeName": "activeWorld", "branch": "next" }),
            json!({ "activeName": "activeSim", "branch": "fresh" }),
        ]
    );
    assert_eq!(
        bodies(&server, "POST", "/api/user/delete-branch"),
        [json!({ "branch": "default" })]
    );
}

#[tokio::test]
async fn directories_are_pushed_and_pulled() {
    let mut server = Server::start(routes, |_| true).await;
    let (overseer, _websocket) = connect(&mut server).await;
    let code = overseer.code();
    let local = TempDir::new("push");
    local.put("main.js", "pushed");
    local.put("engine.wasm", WASM);
    local.put("package.json", "{}");

    let pushed = code.push_dir("default", local.path()).await.unwrap();
    assert_eq!(
        pushed.iter().collect::<Vec<_>>(),
        [("engine", ModuleKind::Wasm), ("main", ModuleKind::Js)]
    );
    assert_eq!(
        bodies(&server, "POST", "/api/user/code"),
        [json!({
            "branch": "default",
            "modules": { "main": "pushed", "engine": { "binary": WASM_BASE64 } },
        })]
    );

    let target = TempDir::new("pull");
    target.put("stale.js", "");
    target.put("notes.txt", "");
    let pulled = code
        .pull_dir(CodeTarget::World.selector(), target.path())
        .await
        .unwrap();
    assert_eq!(target.entries(), ["engine.wasm", "main.js", "notes.txt"]);
    assert_eq!(
        std::fs::read_to_string(target.path().join("main.js")).unwrap(),
        pulled.js("main").unwrap()
    );
    assert_eq!(
        std::fs::read(target.path().join("engine.wasm")).unwrap(),
        WASM
    );
}

#[tokio::test]
async fn code_and_active_branch_events_reach_their_feeds() {
    let mut server = Server::start(routes, |_| true).await;
    let (overseer, mut websocket) = connect(&mut server).await;
    let code = overseer.code();

    let mut changes = code.changes().unwrap();
    let mut activations = code.active_branch_changes().unwrap();
    assert_eq!(
        *changes.channel(),
        Channel::Code {
            user: "u1".to_owned()
        }
    );
    assert_eq!(
        receive(&mut websocket, 2).await,
        [
            "subscribe user:u1/code",
            "subscribe user:u1/set-active-branch"
        ]
    );

    server::send(
        &mut websocket,
        &format!(
            r#"["user:u1/code",{{"branch":"next","modules":{{"main":"x","engine":{{"binary":"{WASM_BASE64}"}}}},"timestamp":5,"hash":7}}]"#
        ),
    )
    .await;
    server::send(
        &mut websocket,
        r#"["user:u1/set-active-branch",{"activeName":"activeSim","branch":"next"}]"#,
    )
    .await;

    let change: CodeChange = soon(changes.recv()).await.unwrap();
    assert_eq!(change.branch, "next");
    assert_eq!(change.timestamp, Some(5));
    assert_eq!(change.hash, Some(UserCodeHash::Number(7.0)));
    assert_eq!(change.modules.js("main"), Some("x"));
    assert_eq!(
        change.modules.wasm("engine").unwrap().as_deref(),
        Some(&WASM[..])
    );
    assert_eq!(
        soon(activations.recv()).await.unwrap(),
        ActiveBranchChange {
            target: CodeTarget::Simulation,
            branch: "next".to_owned(),
        }
    );

    drop(changes);
    drop(activations);
    assert_eq!(
        receive(&mut websocket, 2).await,
        [
            "unsubscribe user:u1/code",
            "unsubscribe user:u1/set-active-branch"
        ]
    );
}
