//! The storage of the saved servers for one test, and the scenario of a saved sign-in: real
//! `overseer` processes save a server, sign in, upload through a profile that names the saved
//! server, and sign out.
//!
//! Each test uses the storage of a unique app identifier (`OVERSEER_APP_IDENTIFIER`): its own
//! directory and its own credential service. The test removes both afterwards. It never
//! touches the saved servers of the user.

use overseer::Overseer;
use overseer::model::platform::Subsystem;
use overseer_storage::native::Storage;
use overseer_storage::profiles::{server_password_key, token_key};

use crate::observer::modules;
use crate::scenario::{MAIN_V1, MAP, WASM};
use crate::support::{self, Account};
use crate::workspace::{SAVED, SAVED_SERVER, Workspace};

/// The key of the secret that checks the credential store.
const PROBE: &str = "probe";

/// The storage of a unique app identifier. Dropping it deletes the secrets of its saved
/// servers and removes its directory.
pub struct TestStorage {
    identifier: String,
    storage: Storage,
}

impl TestStorage {
    /// The storage of a new app identifier. Creates nothing.
    ///
    /// # Panics
    ///
    /// When the system gives no configuration directory.
    pub fn new() -> Self {
        let identifier = format!("com.openscreeps.overseer.e2e-{}", support::unique(""));
        let storage = Storage::open(&identifier).expect("a configuration directory");
        Self {
            identifier,
            storage,
        }
    }

    /// The app identifier.
    pub fn identifier(&self) -> &str {
        &self.identifier
    }

    /// Whether the OS credential store works: saves, reads and deletes a secret.
    fn credential_store(&self) -> Result<(), String> {
        let check = || -> Result<(), overseer_storage::Error> {
            self.storage.set_secret(PROBE, "probe-value")?;
            let read = self.storage.secret(PROBE)?;
            self.storage.delete_secret(PROBE)?;
            if read.as_deref() == Some("probe-value") {
                Ok(())
            } else {
                Err(overseer_storage::Error::Secrets {
                    key: PROBE.to_owned(),
                    message: "the store does not keep the secret".to_owned(),
                })
            }
        };
        check().map_err(|error| error.to_string())
    }
}

impl Drop for TestStorage {
    fn drop(&mut self) {
        if !self.storage.dir().exists() {
            return;
        }
        let profiles = self.storage.load().unwrap_or_default();
        for server in &profiles.servers {
            for key in [token_key(&server.url), server_password_key(&server.url)] {
                if let Err(error) = self.storage.delete_secret(&key) {
                    eprintln!("cannot delete the secret {key}: {error}");
                }
            }
        }
        if let Err(error) = std::fs::remove_dir_all(self.storage.dir()) {
            eprintln!("cannot remove {}: {error}", self.storage.dir().display());
        }
    }
}

/// The commands in order, each followed by what it must have changed:
///
/// 1. `servers add URL --name e2e` saves the server.
/// 2. `login e2e --email USER` (the password on stdin) saves the account and a token, never
///    the password: an API token where the server creates one, else the session token.
/// 3. `profiles` shows the saved sign-in of the profile.
/// 4. `upload` with a configuration whose profile says `server = "e2e"` and that has no
///    `[servers]` creates the branch with the build.
/// 5. `logout e2e` deletes the token and forgets the account.
///
/// Without an OS credential store (for example Linux CI without a Secret Service), the test
/// skips this scenario.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn saved_sign_in_and_upload() {
    if support::official() {
        eprintln!("skipped: the CLI scenario changes the world, so it does not run on screeps.com");
        return;
    }
    let storage = TestStorage::new();
    if let Err(error) = storage.credential_store() {
        eprintln!(
            "skipped: the saved sign-in scenario needs the OS credential store, which is not available: {error}"
        );
        return;
    }
    let account = Account::register().await;
    let observer = account.connect().await;
    let (username, password) = (account.username.clone(), account.password.clone());
    let workspace = Workspace::saved(account, storage.identifier());
    workspace.write_build(MAIN_V1, MAP, Some(WASM));

    let url = support::url_text();
    workspace.succeed(&["servers", "add", &url, "--name", SAVED_SERVER]);
    workspace.succeed_with_input(
        &["login", SAVED_SERVER, "--email", &username],
        &format!("{password}\n"),
    );
    let profiles = storage.storage.load().expect("the saved servers");
    let server = profiles
        .find(SAVED_SERVER)
        .expect("one server e2e")
        .expect("the saved server e2e")
        .clone();
    let user = server.user.clone().expect("the signed-in account");
    let me = observer.account().me().await.expect("the account");
    assert_eq!(user.id, me.id, "the saved account");
    assert_eq!(server.login.as_deref(), Some(username.as_str()));
    let token = storage
        .storage
        .secret(&token_key(&server.url))
        .expect("the credential store")
        .expect("a saved token");
    assert!(!token.is_empty() && token != password, "never the password");
    // A server with API tokens gets a new API token, else the CLI saves the session token.
    let api_tokens = observer.world().supports(Subsystem::ApiTokens);
    assert_eq!(
        user.api_token,
        Some(api_tokens),
        "the kind of the saved token"
    );
    let source = if api_tokens {
        "saved API token"
    } else {
        "saved session token"
    };
    let listing = workspace.succeed(&["profiles"]);
    assert!(listing.contains(source), "{listing}");

    workspace.succeed(&["upload"]);
    let uploaded = modules(&observer, SAVED).await;
    assert_eq!(
        uploaded.js("main"),
        Some(MAIN_V1),
        "the uploaded main module"
    );

    if api_tokens {
        assert_eq!(listed(&observer, &token).await, 1, "the new API token");
        workspace.succeed(&["logout", SAVED_SERVER, "--revoke"]);
        assert_eq!(listed(&observer, &token).await, 0, "logout --revoke");
    } else {
        workspace.succeed(&["logout", SAVED_SERVER]);
    }
    let profiles = storage.storage.load().expect("the saved servers");
    let server = profiles
        .find(SAVED_SERVER)
        .expect("one server e2e")
        .expect("the saved server e2e");
    assert_eq!(server.user, None, "logout forgets the account");
    assert_eq!(
        storage
            .storage
            .secret(&token_key(&server.url))
            .expect("the credential store"),
        None,
        "logout deletes the token"
    );
    observer.shutdown().await;
}

/// The number of entries of the API token list of the account that match `token`.
///
/// # Panics
///
/// When the read fails.
async fn listed(observer: &Overseer, token: &str) -> usize {
    observer
        .account()
        .auth_tokens()
        .await
        .expect("the API tokens")
        .iter()
        .filter(|entry| !entry.token.is_empty() && token.starts_with(&entry.token))
        .count()
}
