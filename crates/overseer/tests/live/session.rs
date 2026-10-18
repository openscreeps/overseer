//! Live tests: signing in, anonymous sessions, the connection and session token refresh.

use std::time::Duration;

use overseer::core::api::ApiError;
use overseer::model::account::UserWorldStatus;
use overseer::net::api::AccountApi as _;
use overseer::{Auth, ConnectionStatus, Error, Notice, Overseer, ServerProfile, net};
use tokio::sync::watch;
use tokio::time::{Instant, timeout, timeout_at};

use crate::support::{self, PATIENCE};

/// A profile of the server under test with `auth`.
fn profile(auth: Auth) -> ServerProfile {
    ServerProfile {
        url: support::url(),
        auth,
        server_password: None,
        default_shard: None,
        server_kind: None,
        plugins: None,
    }
}

/// Waits until the socket of `status` is connected.
async fn connected(mut status: watch::Receiver<ConnectionStatus>) {
    timeout(
        PATIENCE,
        status.wait_for(|status| *status == ConnectionStatus::Connected),
    )
    .await
    .expect("the socket connects in time")
    .expect("the session is open");
}

/// Whether `error` is the server's rejection of a sign-in or token (HTTP 401 `unauthorized`).
fn is_unauthorized(error: &Error) -> bool {
    matches!(error, Error::Net(net::Error::Api(ApiError::Unauthorized)))
}

/// Signing in with the username discovers the single world and the user, connects the socket,
/// and shutting down closes the session.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn signs_in_with_username() {
    let account = support::register().await;
    let overseer = account.connect().await;

    let world = overseer.world();
    assert!(!world.is_sharded(), "the server runs a single world");
    assert_eq!(world.shards(), [] as [String; 0]);
    assert_eq!(world.shard(), None);
    let me = overseer.account().me().await.expect("the profile");
    assert_eq!(overseer.user_id(), Some(me.id.as_str()));
    assert_eq!(me.username.as_deref(), Some(account.username.as_str()));
    assert_eq!(me.email.as_deref(), Some(account.email.as_str()));
    assert!(
        overseer.tokens().borrow().is_some(),
        "the sign-in issued a session token"
    );
    connected(overseer.status()).await;

    overseer.shutdown().await;
    assert_eq!(*overseer.status().borrow(), ConnectionStatus::Closed);
    assert!(matches!(overseer.account().me().await, Err(Error::Closed)));
}

/// The harness's own player (created by `init`, not registered) signs in; it has no spawn.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn harness_player_signs_in() {
    let admin = support::admin();
    let overseer = admin.connect().await;
    let account = overseer.account();
    let me = account.me().await.expect("the profile");
    assert_eq!(overseer.user_id(), Some(me.id.as_str()));
    assert_eq!(me.username.as_deref(), Some(admin.username.as_str()));
    assert_eq!(
        account.world_status().await.expect("the world status"),
        UserWorldStatus::Empty
    );
    connected(overseer.status()).await;
}

/// Signing in with the e-mail address reaches the same user as the username.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn signs_in_with_email() {
    let account = support::register().await;
    let by_name = account.connect().await;
    let by_email = Overseer::connect(profile(Auth::Credentials {
        email: account.email.clone(),
        password: account.password.clone(),
    }))
    .await
    .expect("the e-mail signs in");

    assert!(by_email.user_id().is_some());
    assert_eq!(by_email.user_id(), by_name.user_id());
    let me = by_email.account().me().await.expect("the profile");
    assert_eq!(me.username.as_deref(), Some(account.username.as_str()));
    connected(by_email.status()).await;
}

/// A wrong password or an unknown user is the server's `unauthorized` (HTTP 401).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn wrong_credentials_are_unauthorized() {
    let account = support::register().await;
    for (login, password) in [
        (
            account.username.clone(),
            format!("{}-wrong", account.password),
        ),
        (account.email.clone(), format!("{}-wrong", account.password)),
        (support::unique("nobody"), account.password.clone()),
    ] {
        let error = Overseer::connect(profile(Auth::Credentials {
            email: login,
            password,
        }))
        .await
        .expect_err("the sign-in is rejected");
        assert!(is_unauthorized(&error), "unexpected error: {error:?}");
        assert_eq!(error.to_string(), "unauthorized");
    }
}

/// An anonymous session connects and reads public data; account features need a user.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn anonymous_session_reads_public_data() {
    let account = support::register().await;
    let overseer = Overseer::connect(profile(Auth::Anonymous))
        .await
        .expect("an anonymous session connects");

    assert_eq!(overseer.user_id(), None);
    assert!(overseer.tokens().borrow().is_none());
    connected(overseer.status()).await;
    let found = overseer
        .users()
        .find(&account.username)
        .await
        .expect("users are public")
        .expect("the account exists");
    assert_eq!(found.username.as_deref(), Some(account.username.as_str()));
    let unknown = overseer.users().find("nobody-at-all-0").await;
    assert!(
        matches!(unknown, Ok(None)),
        "an unknown user is none: {unknown:?}"
    );
    let badge = overseer
        .users()
        .badge_svg(&account.username, false)
        .await
        .expect("badges are public");
    assert!(badge.contains("<svg"), "not an SVG document: {badge}");

    assert!(matches!(
        overseer.account().me().await,
        Err(Error::Anonymous)
    ));
    assert!(matches!(overseer.console().feed(), Err(Error::Anonymous)));
    assert!(matches!(
        overseer
            .memory()
            .get(&overseer::memory::MemoryPath::parse("ticks").unwrap(), None)
            .await,
        Err(Error::Anonymous)
    ));
}

/// A session idle for longer than the server's token lifetime (60 s) keeps its rotating token
/// alive: the refresh task reports rotated tokens, the first token expires, and the session
/// still serves authenticated requests. The session holds a token, not credentials, so a lost
/// token would fail instead of signing in again.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn idle_session_keeps_its_token_alive() {
    const IDLE: Duration = Duration::from_secs(70);

    let account = support::register().await;
    let client = account.client();
    client.world().await.expect("the credentials sign in");
    let issued = client.token().expect("the sign-in issued a session token");
    let overseer = Overseer::connect(profile(Auth::Token(issued.clone())))
        .await
        .expect("the token connects");
    let mut notices = overseer.notices();
    let mut tokens = overseer.tokens();
    tokens.borrow_and_update();
    connected(overseer.status()).await;

    let deadline = Instant::now() + IDLE;
    let mut rotations = 0_u32;
    loop {
        match timeout_at(deadline, tokens.changed()).await {
            Ok(Ok(())) => rotations += 1,
            Ok(Err(_)) => panic!("the token updates ended"),
            Err(_) => break,
        }
    }

    // One refresh at least every 25 s without other requests.
    assert!(
        rotations >= 2,
        "only {rotations} token rotations in {IDLE:?}"
    );
    let current = tokens.borrow().clone().expect("a current token");
    assert_ne!(current, issued, "the token was rotated");
    // Discovery is anonymous; the first authenticated request shows the token expired.
    let expired = net::Client::builder(support::url())
        .token(issued)
        .build()
        .expect("a client")
        .api()
        .await
        .expect("discovery needs no account")
        .me()
        .await
        .expect_err("the first token expired");
    assert!(
        matches!(expired, net::Error::Api(ApiError::Unauthorized)),
        "unexpected error: {expired:?}"
    );

    let me = overseer
        .account()
        .refresh()
        .await
        .expect("the idle session still signs requests");
    assert_eq!(me.username.as_deref(), Some(account.username.as_str()));
    assert_eq!(*overseer.status().borrow(), ConnectionStatus::Connected);
    while let Ok(notice) = notices.try_recv() {
        assert_ne!(notice, Notice::AuthFailed, "the token was rejected");
    }
}
