//! Live tests: the account handle (profile, world status, start room, statistics, CPU and
//! resources, badge, notification preferences) and the account edits of the profile page
//! (e-mail and password).

use overseer::core::api::types::{
    UserBadgeColor, UserBadgeInput, UserBadgeInputParam, UserBadgeInputType, UserBadgeType,
    UserEmailRequest, UserNotifyPrefs, UserPasswordSetRequest, UserRoomStatName, UserWorldStatus,
};
use overseer::core::api::{ApiError, endpoints};
use overseer::core::socket::UserCpuCpu;
use overseer::{Auth, Error, Overseer, ServerProfile, StartRoom, StatInterval, net};
use tokio::time::timeout;

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
    })
    .await
}

/// The `{error}` text of a logical failure the server reported with HTTP 200.
fn server_error(error: &net::Error) -> &str {
    match error {
        net::Error::Api(ApiError::Server(message)) => message,
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
    assert_eq!(me.password, Some(true), "password sign-in is configured");
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
    assert_eq!(overview.rooms.unwrap_or_default(), Vec::<String>::new());

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
    assert_eq!(overview.rooms, Some(vec![player.room.clone()]));

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
    let Error::Net(rejected) = rejected else {
        panic!("not a server error: {rejected:?}");
    };
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
        async move {
            let body = UserEmailRequest { email };
            overseer
                .client()
                .send(&endpoints::UserEmailSet { body })
                .await
                .map(drop)
        }
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
) -> Result<(), net::Error> {
    let body = UserPasswordSetRequest {
        old_password,
        password,
    };
    overseer
        .client()
        .send(&endpoints::UserPasswordSet { body })
        .await
        .map(drop)
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

/// A wrong current password is refused with the server's text and changes nothing.
///
/// Not run on `community` (see `COMMUNITY_SKIPPED` in `xtask/src/e2e.rs`): screepsmod-auth
/// 2.10.1 answers it with HTTP 401, then stores an undefined password and replies again, which
/// ends the server's backend (`lib/backend.js`, `POST /api/user/password`,
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
    assert_eq!(server_error(&wrong), "invalid password");
    sign_in(&account.username, &account.password)
        .await
        .expect("the password is unchanged");
}
