//! Live tests: code branches and modules, activation and live code changes.

use overseer::code::{ActiveBranchChange, ActiveBranches, CodeChange};
use overseer::core::api::ApiError;
use overseer::{CodeTarget, Error, Feed, Modules, net};
use tokio::time::timeout;

use crate::support::{self, PATIENCE};

/// The smallest valid `WebAssembly` module: magic and version.
const WASM: &[u8] = b"\0asm\x01\0\0\0";

/// The `{error}` text of a logical failure the server reported with HTTP 200.
fn server_error(error: &Error) -> &str {
    match error {
        Error::Net(net::Error::Api(ApiError::Server(message))) => message,
        error => panic!("not a server error: {error:?}"),
    }
}

/// The first item of `feed` that `wanted` accepts; fails the test naming `what` otherwise.
async fn next<T: Clone + Send + 'static>(
    what: &str,
    feed: &mut Feed<T>,
    wanted: impl Fn(&T) -> bool,
) -> T {
    timeout(PATIENCE, async {
        loop {
            let item = feed.recv().await.expect("the session is open");
            if wanted(&item) {
                break item;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what} did not arrive in time"))
}

/// Runs `operation` and returns its value; fails the test with `what` on error.
async fn ok<T>(what: &str, operation: impl Future<Output = Result<T, Error>>) -> T {
    operation
        .await
        .unwrap_or_else(|error| panic!("{what}: {error}"))
}

/// A new account has the `default` branch, active in the world and the simulator, holding the
/// code it registered with.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn default_branch_of_a_new_account() {
    let mut registered = Modules::new();
    registered.insert_js("main", "module.exports.loop = function () {};");
    let account = support::register_with(Some(registered.clone())).await;
    let overseer = account.connect().await;
    let code = overseer.code();

    let branches = ok("the branches", code.branches()).await;
    assert_eq!(branches.len(), 1, "{branches:?}");
    assert_eq!(branches[0].branch, "default");
    assert_eq!(
        ok("the active branches", code.active_branches()).await,
        ActiveBranches {
            world: Some("default".to_owned()),
            simulation: Some("default".to_owned()),
        }
    );
    assert_eq!(ok("the modules", code.modules("default")).await, registered);
    assert_eq!(
        ok("the world code", code.active_modules(CodeTarget::World)).await,
        ("default".to_owned(), registered.clone())
    );
    assert_eq!(
        ok(
            "the simulator code",
            code.active_modules(CodeTarget::Simulation)
        )
        .await,
        ("default".to_owned(), registered)
    );
}

/// Branches are created, uploaded to (JavaScript and `WebAssembly`), read back, cloned,
/// activated and deleted; every change is announced live.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn branches_round_trip() {
    let account = support::register().await;
    let overseer = account.connect().await;
    let code = overseer.code();
    let mut changes = code.changes().expect("the code changes feed");
    let mut activations = code.active_branch_changes().expect("the activations feed");
    support::subscribed(&overseer).await;
    let branch = support::unique("b");
    let copy = support::unique("c");

    let mut created = Modules::new();
    created.insert_js("main", "require('util').run();");
    created.insert_js("util", "module.exports.run = function () {};");
    ok(
        "the branch is created",
        code.create_branch(&branch, created.clone()),
    )
    .await;
    let change: CodeChange = next("the new branch", &mut changes, |change: &CodeChange| {
        change.branch == branch
    })
    .await;
    assert_eq!(change.modules, created);
    assert!(change.timestamp.is_some());
    let listed = ok("the branches", code.branches()).await;
    let entry = listed
        .iter()
        .find(|entry| entry.branch == branch)
        .expect("the new branch is listed");
    assert_ne!(entry.active_world, Some(true), "a new branch is inactive");
    assert_eq!(ok("the modules", code.modules(&branch)).await, created);

    let mut uploaded = Modules::new();
    uploaded.insert_js("main", "module.exports.loop = function () {};");
    uploaded.insert_wasm("engine", WASM);
    ok("the upload", code.set_modules(&branch, uploaded.clone())).await;
    let change = next("the upload", &mut changes, |change: &CodeChange| {
        change.branch == branch && change.modules.kind("util").is_none()
    })
    .await;
    assert_eq!(change.modules, uploaded);
    let stored = ok("the modules", code.modules(&branch)).await;
    assert_eq!(stored, uploaded, "the upload replaces every module");
    assert_eq!(
        stored.wasm("engine").expect("valid base64").as_deref(),
        Some(WASM)
    );
    assert_eq!(stored.js("util"), None);

    ok("the clone", code.clone_branch(&branch, &copy)).await;
    assert_eq!(ok("the copy", code.modules(&copy)).await, uploaded);

    ok(
        "the activation",
        code.set_active_branch(CodeTarget::World, &branch),
    )
    .await;
    let activation = next(
        "the activation",
        &mut activations,
        |change: &ActiveBranchChange| change.target == CodeTarget::World,
    )
    .await;
    assert_eq!(activation.branch, branch);
    let active = ok("the active branches", code.active_branches()).await;
    assert_eq!(active.get(CodeTarget::World), Some(branch.as_str()));
    assert_eq!(active.get(CodeTarget::Simulation), Some("default"));
    assert_eq!(
        ok("the world code", code.active_modules(CodeTarget::World)).await,
        (branch.clone(), uploaded)
    );

    ok("the copy is deleted", code.delete_branch(&copy)).await;
    // The server keeps the active branch.
    ok("the active branch is kept", code.delete_branch(&branch)).await;
    let names: Vec<String> = ok("the branches", code.branches())
        .await
        .into_iter()
        .map(|entry| entry.branch)
        .collect();
    assert!(names.contains(&branch), "{names:?}");
    assert!(!names.contains(&copy), "{names:?}");
}

/// Unknown branches are refused with the server's texts.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn unknown_branches_are_refused() {
    let account = support::register().await;
    let overseer = account.connect().await;
    let code = overseer.code();
    let missing = support::unique("missing");

    let error = code.modules(&missing).await.expect_err("no code to read");
    assert_eq!(server_error(&error), "no code");
    let error = code
        .set_modules(&missing, Modules::new())
        .await
        .expect_err("no branch to upload to");
    assert_eq!(server_error(&error), "branch does not exist");
    let error = code
        .set_active_branch(CodeTarget::World, &missing)
        .await
        .expect_err("no branch to activate");
    assert_eq!(server_error(&error), "no branch");
}
