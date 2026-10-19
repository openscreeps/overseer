//! Live tests: the account handle (profile, world status, start room, statistics, CPU and
//! resources, badge, notification preferences), the account edits of the profile page
//! (e-mail and password) and persistent API tokens.

use std::time::Duration;

use overseer::core::Token;
use overseer::core::api::ApiError;
use overseer::model::account::{
    AuthTokenRequest, StartRoom, UserBadgeColor, UserBadgeInput, UserBadgeInputParam,
    UserBadgeInputType, UserBadgeType, UserNotifyPrefs, UserRoomStatName, UserWorldStatus,
};
use overseer::model::platform::Subsystem;
use overseer::model::socket::UserCpuCpu;
use overseer::{Auth, Error, Overseer, ServerProfile, StatInterval, net};
use tokio::time::{sleep, timeout};

use crate::support::{self, PATIENCE};

/// Signs in with `login` (username or e-mail) and `password`.
async fn sign_in(login: &str, password: &str) -> Result<Overseer, Error> {
    Overseer::connect(ServerProfile {
        url: support::url(),
        auth: Auth::Credentials {
            email: login.to_owned(),
            password: password.to_owned(),
        },
        server_password: None,
        default_shard: None,
        server_kind: None,
        plugins: None,
    })
    .await
}

/// The `{error}` text of a logical failure the server reported with HTTP 200.
fn server_error(error: &Error) -> &str {
    match error {
        Error::Net(net::Error::Api(ApiError::Server(message))) => message,
        error => panic!("not a server error: {error:?}"),
    }
}

/// Whether `error` is the server's rejection of a sign-in or token (HTTP 401 `unauthorized`).
fn is_unauthorized(error: &Error) -> bool {
    matches!(error, Error::Net(net::Error::Api(ApiError::Unauthorized)))
}

/// A fresh account: its profile, no world presence yet, the server's default start room, an
/// empty badge and the default notification preferences.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn fresh_account_profile() {
    let account = support::register().await;
    let overseer = account.connect().await;
    let handle = overseer.account();

    let me = handle.me().await.expect("the profile");
    assert_eq!(overseer.user_id(), Some(me.id.as_str()));
    assert_eq!(me.username.as_deref(), Some(account.username.as_str()));
    assert_eq!(me.email.as_deref(), Some(account.email.as_str()));
    assert_eq!(
        me.has_password,
        Some(true),
        "password sign-in is configured"
    );
    assert_eq!(me.gcl, Some(0.0));
    assert!(me.cpu.is_some_and(|cpu| cpu > 0.0), "CPU {:?}", me.cpu);
    assert!(me.money.abs() < f64::EPSILON, "credits {}", me.money);
    assert!(me.badge.is_none());

    assert_eq!(
        handle.world_status().await.expect("the world status"),
        UserWorldStatus::Empty
    );
    let start = handle
        .start_room(None)
        .await
        .expect("the start room")
        .expect("the server names a start room");
    assert_eq!(start.shard, None);
    assert!(
        start.room.starts_with('W') || start.room.starts_with('E'),
        "not a room name: {}",
        start.room
    );
    assert_eq!(
        handle
            .respawn_prohibited_rooms()
            .await
            .expect("the prohibited rooms"),
        Vec::<String>::new()
    );
    let overview = handle
        .overview(StatInterval::default(), UserRoomStatName::EnergyHarvested)
        .await
        .expect("the overview");
    assert_eq!(overview.rooms, []);

    let svg = handle.badge_svg(false).await.expect("the badge");
    assert!(svg.contains("<svg"), "not an SVG document: {svg}");
    assert_eq!(
        handle.notify_prefs().await.expect("the preferences"),
        UserNotifyPrefs::default()
    );
    let mut resources = handle.resources().expect("the resources watch");
    let resources = timeout(PATIENCE, resources.changed())
        .await
        .expect("the resources arrive in time")
        .expect("the session is open");
    assert!(resources.credits.abs() < f64::EPSILON);
}

/// A player: plays in the room of its spawn, which is its start room and in its overview, and
/// its runs report CPU.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn player_profile() {
    let player = support::player().await;
    let overseer = player.account.connect().await;
    let handle = overseer.account();

    assert_eq!(
        handle.world_status().await.expect("the world status"),
        UserWorldStatus::Normal
    );
    assert_eq!(
        handle.start_room(None).await.expect("the start room"),
        Some(StartRoom {
            shard: None,
            room: player.room.clone(),
        })
    );
    let overview = handle
        .overview(StatInterval::default(), UserRoomStatName::CreepsProduced)
        .await
        .expect("the overview");
    let rooms: Vec<_> = overview
        .rooms
        .iter()
        .map(|room| (room.shard.as_deref(), room.room.as_str()))
        .collect();
    assert_eq!(rooms, [(None, player.room.as_str())]);

    let mut cpu = handle.cpu().expect("the CPU watch");
    let used = timeout(PATIENCE, async {
        loop {
            let report = cpu.changed().await.expect("the session is open");
            // Subscribing reports Memory's size with no CPU; runs report what they used.
            if let UserCpuCpu::Number(used) = report.cpu
                && used > 0.0
            {
                break (used, report.memory);
            }
        }
    })
    .await
    .expect("a run reports its CPU in time");
    assert!(
        used.1.is_some_and(|memory| memory > 0),
        "Memory size {:?}",
        used.1
    );
}

/// A new badge is stored, read back in the profile and rendered by `badge-svg`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn badge_round_trip() {
    let account = support::register().await;
    let overseer = account.connect().await;
    let handle = overseer.account();
    let empty = handle.badge_svg(false).await.expect("the empty badge");

    handle
        .set_badge(UserBadgeInput {
            color1: "#ff0000".to_owned(),
            color2: "#00ff00".to_owned(),
            color3: "#0000ff".to_owned(),
            flip: Some(true),
            param: UserBadgeInputParam::Number(25.0),
            r#type: UserBadgeInputType::Integer(7),
        })
        .await
        .expect("the badge is saved");
    let badge = handle
        .me()
        .await
        .expect("the profile")
        .badge
        .clone()
        .expect("a badge");
    assert_eq!(badge.r#type, UserBadgeType::Integer(7));
    assert_eq!(badge.color1, UserBadgeColor::String("#ff0000".to_owned()));
    assert_eq!(badge.param, Some(25.0));
    assert!(badge.flip);
    let svg = handle.badge_svg(true).await.expect("the badge");
    assert!(svg.contains("<svg"), "not an SVG document: {svg}");
    assert_ne!(svg, empty, "the badge is drawn");

    let rejected = handle
        .set_badge(UserBadgeInput {
            color1: "red".to_owned(),
            color2: "#00ff00".to_owned(),
            color3: "#0000ff".to_owned(),
            flip: None,
            param: UserBadgeInputParam::Number(0.0),
            r#type: UserBadgeInputType::Integer(99),
        })
        .await
        .expect_err("an invalid badge is rejected");
    assert_eq!(server_error(&rejected), "invalid params");
}

/// Notification preferences are saved, read back, and fields left out keep their value.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn notify_prefs_round_trip() {
    let account = support::register().await;
    let overseer = account.connect().await;
    let handle = overseer.account();

    let prefs = UserNotifyPrefs {
        disabled: Some(true),
        disabled_on_messages: Some(true),
        errors_interval: Some(100_000),
        interval: Some(60),
        send_online: Some(true),
    };
    handle
        .set_notify_prefs(prefs.clone())
        .await
        .expect("the preferences are saved");
    assert_eq!(handle.notify_prefs().await.expect("the preferences"), prefs);

    handle
        .set_notify_prefs(UserNotifyPrefs {
            disabled: Some(false),
            interval: Some(5),
            ..UserNotifyPrefs::default()
        })
        .await
        .expect("the preferences are saved");
    let expected = UserNotifyPrefs {
        disabled: Some(false),
        interval: Some(5),
        ..prefs
    };
    assert_eq!(
        handle.notify_prefs().await.expect("the preferences"),
        expected
    );
    // Another session reads what the server stored.
    let other = account.connect().await;
    assert_eq!(
        other
            .account()
            .notify_prefs()
            .await
            .expect("the preferences"),
        expected
    );
}

/// The e-mail address changes (stored lower-cased): the profile shows it and it signs in, the
/// old one no longer does; invalid and taken addresses are refused with the server's text.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn email_change_round_trip() {
    let account = support::register().await;
    let other = support::register().await;
    let overseer = account.connect().await;
    let set_email = |email: String| {
        let overseer = overseer.clone();
        async move { overseer.account().set_email(&email).await }
    };

    let changed = format!("{}@E2E.invalid", support::unique("Mail"));
    set_email(changed.clone())
        .await
        .expect("the e-mail is changed");
    let lower = changed.to_lowercase();
    let me = overseer.account().refresh().await.expect("the profile");
    assert_eq!(me.email.as_deref(), Some(lower.as_str()));
    let by_email = sign_in(&lower, &account.password)
        .await
        .expect("the new e-mail signs in");
    assert_eq!(by_email.user_id(), overseer.user_id());
    let old = sign_in(&account.email, &account.password)
        .await
        .expect_err("the old e-mail no longer signs in");
    assert!(is_unauthorized(&old), "unexpected error: {old:?}");

    let invalid = set_email("not an address".to_owned())
        .await
        .expect_err("an invalid e-mail is refused");
    assert_eq!(server_error(&invalid), "invalid email");
    let taken = set_email(other.email.clone())
        .await
        .expect_err("a taken e-mail is refused");
    assert_eq!(server_error(&taken), "email already exists");
}

/// Changes the password of `overseer`'s account (`POST /api/user/password`).
async fn set_password(
    overseer: &Overseer,
    old_password: String,
    password: String,
) -> Result<(), Error> {
    overseer
        .account()
        .set_password(Some(&old_password), &password)
        .await
}

/// The password changes: a new session signs in with the new password, the old one fails.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn password_change() {
    let account = support::register().await;
    let overseer = account.connect().await;
    let password = format!("new-{}", support::unique(""));
    set_password(&overseer, account.password.clone(), password.clone())
        .await
        .expect("the password is changed");
    let renewed = sign_in(&account.username, &password)
        .await
        .expect("the new password signs in");
    assert_eq!(renewed.user_id(), overseer.user_id());
    renewed
        .account()
        .me()
        .await
        .expect("the new session is signed in");
    let old = sign_in(&account.username, &account.password)
        .await
        .expect_err("the old password no longer signs in");
    assert!(is_unauthorized(&old), "unexpected error: {old:?}");
}

/// A wrong current password is refused alike on every server and changes nothing. On
/// `community` Overseer checks it before sending the change: screepsmod-auth 2.10.1 would
/// answer it with HTTP 401, then store an undefined password and reply again, which ends the
/// server's backend (`lib/backend.js`, `POST /api/user/password`,
/// <https://unpkg.com/screepsmod-auth@2.10.1/lib/backend.js>).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn wrong_current_password_is_refused() {
    let account = support::register().await;
    let overseer = account.connect().await;
    let wrong = set_password(
        &overseer,
        format!("{}-wrong", account.password),
        "never-used-1234".to_owned(),
    )
    .await
    .expect_err("a wrong current password is refused");
    assert!(
        matches!(wrong, Error::Net(net::Error::WrongPassword)),
        "unexpected error: {wrong:?}"
    );
    sign_in(&account.username, &account.password)
        .await
        .expect("the password is unchanged");
    overseer
        .account()
        .me()
        .await
        .expect("the session and the server are still up");
}

/// A session authenticated with `token` alone.
async fn connect_with_token(token: Token) -> Result<Overseer, Error> {
    Overseer::connect(ServerProfile {
        url: support::url(),
        auth: Auth::Token(token),
        server_password: None,
        default_shard: None,
        server_kind: None,
        plugins: None,
    })
    .await
}

/// Longer than the session-token lifetime of private servers (60 s by default).
const SESSION_EXPIRY: Duration = Duration::from_secs(65);

/// A persistent API token ([`Subsystem::ApiTokens`]): it signs in a second session, the server
/// never replaces it, it outlives an expired session token, it is listed by its first 8
/// characters, and its deletion revokes it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn api_token_round_trip() {
    let account = support::register().await;
    let overseer = account.connect().await;
    if !overseer.world().supports(Subsystem::ApiTokens) {
        eprintln!("skipped: the server has no API tokens");
        return;
    }
    let token = overseer
        .account()
        .create_auth_token(AuthTokenRequest {
            kind: Some("full".to_owned()),
            ..AuthTokenRequest::default()
        })
        .await
        .expect("an API token is created");
    // The session token of the password sign-in, kept alive only while its session runs.
    let session_token = overseer
        .tokens()
        .borrow()
        .clone()
        .expect("the sign-in holds a session token");
    assert_ne!(session_token, token);
    overseer.shutdown().await;

    let second = connect_with_token(token.clone())
        .await
        .expect("the API token signs in a second session");
    let me = second.account().me().await.expect("the profile");
    assert_eq!(me.username.as_deref(), Some(account.username.as_str()));
    support::subscribed(&second).await;

    sleep(SESSION_EXPIRY).await;
    let expired = connect_with_token(session_token)
        .await
        .expect_err("the session token expired on the server");
    assert!(is_unauthorized(&expired), "unexpected error: {expired:?}");
    second
        .account()
        .me()
        .await
        .expect("the API token still authenticates after the session expiry");
    assert_eq!(
        second.tokens().borrow().as_ref(),
        Some(&token),
        "the server never replaced the API token"
    );

    let prefix = token
        .as_str()
        .get(..8)
        .expect("a token of at least 8 characters");
    let entries = second
        .account()
        .auth_tokens()
        .await
        .expect("the token list");
    let entry = entries
        .iter()
        .find(|entry| entry.token.get(..8) == Some(prefix))
        .expect("the token is listed by its first 8 characters");
    assert!(entry.full, "a full-access token");
    assert_ne!(entry.token, token.as_str(), "the list hides the token");
    second
        .account()
        .delete_auth_token(&entry.id)
        .await
        .expect("the token is deleted");
    let entries = second
        .account()
        .auth_tokens()
        .await
        .expect_err("the deleted token no longer authenticates");
    assert!(is_unauthorized(&entries), "unexpected error: {entries:?}");
    second.shutdown().await;
    let refused = connect_with_token(token)
        .await
        .expect_err("a deleted token is refused");
    assert!(is_unauthorized(&refused), "unexpected error: {refused:?}");
    eprintln!(
        "API token: created, signed in a second session, kept after {}s (session token \
         expired), listed by its first 8 characters, deleted and refused",
        SESSION_EXPIRY.as_secs()
    );
}
