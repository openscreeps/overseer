//! The server under test and a new account on it.
//!
//! `cargo xtask e2e` starts a disposable world and sets `OVERSEER_E2E_TARGET` (`openscreeps`,
//! `community` or `official`) and `OVERSEER_LIVE_URL` (the server base URL). The CLI scenario
//! uploads code and places a spawn, so it registers a new account of its own on each run
//! (`POST /api/register/submit`). Both disposable targets support password accounts.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use overseer::core::ServerUrl;
use overseer::model::account::Registration;
use overseer::net::Client;
use overseer::net::api::PasswordAccount as _;
use overseer::{Auth, Overseer, ServerProfile};

/// The server under test (`OVERSEER_LIVE_URL`) as the environment gives it.
///
/// # Panics
///
/// When the variable is missing. Run the live tests with `cargo xtask e2e`.
pub fn url_text() -> String {
    std::env::var("OVERSEER_LIVE_URL")
        .expect("OVERSEER_LIVE_URL is not set: run the live tests with `cargo xtask e2e`")
}

/// The server under test (`OVERSEER_LIVE_URL`).
///
/// # Panics
///
/// When the variable is missing or is not a URL.
pub fn url() -> ServerUrl {
    ServerUrl::parse(&url_text()).expect("OVERSEER_LIVE_URL is a server URL")
}

/// The target of the run (`OVERSEER_E2E_TARGET`), empty when the variable is not set.
pub fn target() -> String {
    std::env::var("OVERSEER_E2E_TARGET").unwrap_or_default()
}

/// Whether the tests run against screeps.com, where no test may change the world.
pub fn official() -> bool {
    target() == "official"
}

/// A registered account that signs in with a password.
#[derive(Clone, Debug)]
pub struct Account {
    /// The unique username.
    pub username: String,
    /// The password.
    pub password: String,
}

impl Account {
    /// Registers a new account without code. The account has no world presence yet.
    ///
    /// # Panics
    ///
    /// When the server does not register the account.
    pub async fn register() -> Self {
        let username = unique("cli");
        let account = Self {
            password: format!("pw-{}", unique("")),
            username,
        };
        let registration = Registration {
            username: account.username.clone(),
            email: format!("{}@e2e.invalid", account.username),
            password: account.password.clone(),
            modules: None,
        };
        Client::builder(url())
            .build()
            .expect("a client")
            .api()
            .await
            .expect("the server's operations")
            .password_account()
            .expect("the server registers accounts")
            .register(registration)
            .await
            .expect("registration succeeds");
        account
    }

    /// A session of this account.
    ///
    /// # Panics
    ///
    /// When the account does not sign in.
    pub async fn connect(&self) -> Overseer {
        Overseer::connect(ServerProfile {
            url: url(),
            auth: Auth::Credentials {
                email: self.username.clone(),
                password: self.password.clone(),
            },
            server_password: None,
            default_shard: None,
            server_kind: None,
            plugins: None,
        })
        .await
        .expect("the account signs in")
    }
}

/// A name that nobody used yet: `prefix` and a value unique for each process and call.
pub fn unique(prefix: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let mixed = now
        .as_secs()
        .wrapping_mul(1_000_000_007)
        .wrapping_add(u64::from(now.subsec_nanos()))
        ^ u64::from(std::process::id()).wrapping_mul(0x0100_0000_01b3)
        ^ NEXT
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15);
    format!("{prefix}{:010x}", mixed & 0xff_ffff_ffff)
}
