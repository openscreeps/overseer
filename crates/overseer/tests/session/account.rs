//! The account: the cached profile, request shapes, CPU and resources watches.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use overseer::core::Token;
use overseer::core::api::types::{
    PowerCreepCreateRequestClassName, UserAuthTokenCreateRequest, UserBadgeCustomPaths,
    UserBadgeInput, UserBadgeInputParam, UserBadgeInputType, UserNotifyPrefs, UserRoomStatName,
    UserWorldStatus,
};
use overseer::core::socket::{Channel, UserCpuCpu};
use overseer::{Auth, Error, Overseer, ServerProfile, StartRoom, StatInterval};

use crate::server::{self, Request, Server};
use crate::{connect, receive, routes, soon};

/// The account routes over [`routes`]; `/api/auth/me` reports `money` = the number of profile
/// requests so far.
fn account_routes(request: &Request, profiles: &AtomicU64) -> (u16, String) {
    let ok = || (200, r#"{"ok":1}"#.to_owned());
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/api/auth/me") if request.token.is_some() => {
            let money = profiles.fetch_add(1, Ordering::SeqCst);
            (
                200,
                format!(
                    r#"{{"ok":1,"_id":"u1","username":"Alice","money":{money},"powerExperimentations":1,"notifyPrefs":{{"sendOnline":true}}}}"#
                ),
            )
        }
        ("GET", "/api/user/overview") => (
            200,
            r#"{"ok":1,"rooms":["W1N1"],"stats":{"W1N1":[{"value":5,"endTime":1}]},"statsMax":5,"totals":{"energyHarvested":5}}"#
                .to_owned(),
        ),
        ("GET", "/api/user/world-status") => (200, r#"{"ok":1,"status":"normal"}"#.to_owned()),
        ("GET", "/api/user/world-start-room") => {
            (200, r#"{"ok":1,"room":["shard1/W2N2"]}"#.to_owned())
        }
        ("GET", "/api/user/respawn-prohibited-rooms") => {
            (200, r#"{"ok":1,"rooms":["W3N3"]}"#.to_owned())
        }
        ("GET", "/api/user/badge-svg") => (200, "<svg/>".to_owned()),
        ("GET", "/api/game/power-creeps/list") => (
            200,
            r#"{"ok":1,"list":[{"_id":"pc1","className":"operator","hitsMax":1000,"level":0,"name":"Op","powers":{},"store":{},"storeCapacity":100,"user":"u1"}]}"#
                .to_owned(),
        ),
        ("POST", "/api/game/power-creeps/create") => (
            200,
            r#"{"ok":1,"_id":"pc2","name":"Op2","className":"operator","level":0}"#.to_owned(),
        ),
        ("GET", "/api/user/auth-token") => (
            200,
            r#"{"ok":1,"tokens":[{"_id":"k1","description":null,"full":true,"token":"secret"}]}"#
                .to_owned(),
        ),
        ("POST", "/api/user/auth-token") => (200, r#"{"ok":1,"token":"fresh"}"#.to_owned()),
        (
            "POST",
            "/api/user/respawn"
            | "/api/user/badge"
            | "/api/user/notify-prefs"
            | "/api/user/delete-auth-token"
            | "/api/game/power-creeps/rename"
            | "/api/game/power-creeps/delete"
            | "/api/game/power-creeps/cancel-delete"
            | "/api/game/power-creeps/upgrade"
            | "/api/game/power-creeps/experimentation",
        ) => ok(),
        _ => routes(request),
    }
}

/// A server with [`account_routes`].
async fn start() -> Server {
    let profiles = Arc::new(AtomicU64::new(0));
    Server::start(move |request| account_routes(request, &profiles), |_| true).await
}

/// The requests to `path` (method, query, body).
fn sent(server: &Server, path: &str) -> Vec<(String, String, String)> {
    server
        .requests()
        .into_iter()
        .filter(|request| request.path == path)
        .map(|request| (request.method, request.query, request.body))
        .collect()
}

/// `(method, query, body)` of a request.
fn request(method: &str, query: &str, body: &str) -> (String, String, String) {
    (method.to_owned(), query.to_owned(), body.to_owned())
}

#[tokio::test]
async fn the_profile_is_cached_until_refreshed_or_changed() {
    let mut server = start().await;
    let (overseer, _websocket) = connect(&mut server).await;
    // Connecting read the profile once (money 0): the first `me` with no request.
    let account = overseer.account();

    let first = account.me().await.unwrap();
    assert_eq!(first.money, 0.0);
    assert_eq!(first.username.as_deref(), Some("Alice"));
    // Every handle shares the cache.
    let other = overseer.account();
    let (again, other) = tokio::join!(account.me(), other.me());
    assert!(Arc::ptr_eq(&first, &again.unwrap()));
    assert!(Arc::ptr_eq(&first, &other.unwrap()));
    assert_eq!(sent(&server, "/api/auth/me").len(), 1);

    let refreshed = account.refresh().await.unwrap();
    assert_eq!(refreshed.money, 1.0);
    assert!(Arc::ptr_eq(&refreshed, &account.me().await.unwrap()));
    assert_eq!(
        account.notify_prefs().await.unwrap(),
        UserNotifyPrefs {
            send_online: Some(true),
            ..UserNotifyPrefs::default()
        }
    );
    assert_eq!(sent(&server, "/api/auth/me").len(), 2);

    // The account's own changes clear the cache.
    let badge = UserBadgeInput {
        color1: "#ff0000".to_owned(),
        color2: "#00ff00".to_owned(),
        color3: "#0000ff".to_owned(),
        flip: Some(true),
        param: UserBadgeInputParam::Number(-20.0),
        r#type: UserBadgeInputType::Integer(7),
    };
    account.set_badge(badge).await.unwrap();
    assert_eq!(account.me().await.unwrap().money, 2.0);
    account
        .set_badge(UserBadgeInput {
            color1: "#000000".to_owned(),
            color2: "#000000".to_owned(),
            color3: "#000000".to_owned(),
            flip: None,
            param: UserBadgeInputParam::String("0".to_owned()),
            r#type: UserBadgeInputType::CustomPaths(UserBadgeCustomPaths {
                path1: "M0 0".to_owned(),
                path2: String::new(),
            }),
        })
        .await
        .unwrap();
    account
        .set_notify_prefs(UserNotifyPrefs {
            disabled_on_messages: Some(true),
            interval: Some(60),
            ..UserNotifyPrefs::default()
        })
        .await
        .unwrap();
    account.respawn().await.unwrap();
    account
        .power_creeps()
        .start_experimentation()
        .await
        .unwrap();
    assert_eq!(account.me().await.unwrap().money, 3.0);
    assert_eq!(sent(&server, "/api/auth/me").len(), 4);
    assert_eq!(
        sent(&server, "/api/user/badge"),
        [
            request(
                "POST",
                "",
                r##"{"badge":{"color1":"#ff0000","color2":"#00ff00","color3":"#0000ff","flip":true,"param":-20.0,"type":7}}"##
            ),
            request(
                "POST",
                "",
                r##"{"badge":{"color1":"#000000","color2":"#000000","color3":"#000000","param":"0","type":{"path1":"M0 0","path2":""}}}"##
            ),
        ]
    );
    assert_eq!(
        sent(&server, "/api/user/notify-prefs"),
        [request(
            "POST",
            "",
            r#"{"disabledOnMessages":true,"interval":60}"#
        )]
    );
    assert_eq!(
        sent(&server, "/api/user/respawn"),
        [request("POST", "", "")]
    );
    assert_eq!(
        sent(&server, "/api/game/power-creeps/experimentation"),
        [request("POST", "", "{}")]
    );

    overseer.shutdown().await;
    assert!(matches!(account.me().await, Err(Error::Closed)));
}

#[tokio::test]
async fn account_requests_have_the_documented_shape() {
    let mut server = start().await;
    let (overseer, _websocket) = connect(&mut server).await;
    let account = overseer.account();

    let overview = account
        .overview(StatInterval::ThreeHours, UserRoomStatName::EnergyHarvested)
        .await
        .unwrap();
    assert_eq!(overview.rooms.unwrap(), ["W1N1"]);
    assert_eq!(
        sent(&server, "/api/user/overview"),
        [request("GET", "interval=180&statName=energyHarvested", "")]
    );
    assert_eq!(
        account.world_status().await.unwrap(),
        UserWorldStatus::Normal
    );
    // Without a shard the server names the user's shard; single-world servers never send one.
    let start_room = StartRoom {
        shard: Some("shard1".to_owned()),
        room: "W2N2".to_owned(),
    };
    assert_eq!(
        account.start_room(None).await.unwrap(),
        Some(start_room.clone())
    );
    assert_eq!(
        account.start_room(Some("shard3")).await.unwrap(),
        Some(start_room)
    );
    assert_eq!(
        sent(&server, "/api/user/world-start-room"),
        [request("GET", "", ""), request("GET", "", "")]
    );
    assert_eq!(account.respawn_prohibited_rooms().await.unwrap(), ["W3N3"]);
    assert_eq!(account.badge_svg(true).await.unwrap(), "<svg/>");
    assert_eq!(
        sent(&server, "/api/user/badge-svg"),
        [request("GET", "username=Alice&border=1", "")]
    );

    let tokens = account.auth_tokens().await.unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].id, "k1");
    let created = account
        .create_auth_token(UserAuthTokenCreateRequest {
            r#type: Some("full".to_owned()),
            ..UserAuthTokenCreateRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(created, Token::new("fresh"));
    account.delete_auth_token("k1").await.unwrap();
    assert_eq!(
        sent(&server, "/api/user/auth-token"),
        [
            request("GET", "", ""),
            request("POST", "", r#"{"type":"full"}"#)
        ]
    );
    assert_eq!(
        sent(&server, "/api/user/delete-auth-token"),
        [request("POST", "", r#"{"tokenId":"k1"}"#)]
    );
}

#[tokio::test]
async fn the_rate_limit_exemption_is_the_session_tokens_entry_while_in_the_future() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    let until = u64::try_from(now.as_millis()).unwrap() + 7_200_000;
    let profiles = Arc::new(AtomicU64::new(0));
    // Entries hold the first 8 characters of each token.
    let tokens = format!(
        r#"{{"ok":1,"tokens":[{{"_id":"k1","full":true,"token":"ffffeeee","noRatelimitUntil":{}}},{{"_id":"k2","description":"bot","full":false,"token":"0123abcd","noRatelimitUntil":{until}}},{{"_id":"k3","full":true,"token":"99998888","noRatelimitUntil":1}},{{"_id":"k4","full":true,"token":"77776666"}}]}}"#,
        until + 1
    );
    let server = Server::start(
        move |request| match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/api/user/auth-token") => (200, tokens.clone()),
            _ => account_routes(request, &profiles),
        },
        |_| true,
    )
    .await;
    for (token, expected) in [
        ("0123abcd-4567-89ef-0123-456789abcdef", Some(until)),
        // Lifted once, long ago; never lifted.
        ("99998888-4567-89ef-0123-456789abcdef", None),
        ("77776666-4567-89ef-0123-456789abcdef", None),
        // A password sign-in's session token is in no entry.
        ("0123abce5d6e7f8091a2b3c4d5e6f708192a3b4c", None),
    ] {
        let profile = ServerProfile {
            url: server.url(),
            auth: Auth::Token(Token::new(token)),
            server_password: None,
            default_shard: None,
        };
        let overseer = Overseer::connect(profile).await.unwrap();
        assert_eq!(
            overseer.account().rate_limit_exemption().await.unwrap(),
            expected
        );
        overseer.shutdown().await;
    }
    assert_eq!(
        sent(&server, "/api/user/auth-token"),
        vec![request("GET", "", ""); 4]
    );
}

#[tokio::test]
async fn power_creep_requests_have_the_documented_shape() {
    let mut server = start().await;
    let (overseer, _websocket) = connect(&mut server).await;
    let power_creeps = overseer.account().power_creeps();

    let list = power_creeps.list().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "Op");
    let created = power_creeps
        .create("Op2", PowerCreepCreateRequestClassName::Operator)
        .await
        .unwrap();
    assert_eq!(created.id.as_deref(), Some("pc2"));
    power_creeps.rename("pc1", "Renamed").await.unwrap();
    power_creeps.delete("pc1").await.unwrap();
    power_creeps.cancel_delete("pc1").await.unwrap();
    let powers = BTreeMap::from([("1".to_owned(), 2), ("2".to_owned(), 1)]);
    power_creeps.upgrade("pc1", powers).await.unwrap();

    let posted: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|request| request.path.starts_with("/api/game/power-creeps/"))
        .map(|request| (request.method, request.path, request.body))
        .collect();
    let expected = [
        ("GET", "list", ""),
        ("POST", "create", r#"{"className":"operator","name":"Op2"}"#),
        ("POST", "rename", r#"{"id":"pc1","name":"Renamed"}"#),
        ("POST", "delete", r#"{"id":"pc1"}"#),
        ("POST", "cancel-delete", r#"{"id":"pc1"}"#),
        ("POST", "upgrade", r#"{"id":"pc1","powers":{"1":2,"2":1}}"#),
    ]
    .map(|(method, path, body)| {
        (
            method.to_owned(),
            format!("/api/game/power-creeps/{path}"),
            body.to_owned(),
        )
    });
    assert_eq!(posted, expected);
}

#[tokio::test]
async fn cpu_and_resources_are_watched() {
    let mut server = start().await;
    let (overseer, mut websocket) = connect(&mut server).await;
    let account = overseer.account();

    let mut cpu = account.cpu().unwrap();
    assert_eq!(
        *cpu.channel(),
        Channel::Cpu {
            user: "u1".to_owned()
        }
    );
    assert_eq!(receive(&mut websocket, 1).await, ["subscribe user:u1/cpu"]);
    let mut resources = account.resources().unwrap();
    assert_eq!(
        receive(&mut websocket, 1).await,
        ["subscribe user:u1/resources"]
    );

    server::send(
        &mut websocket,
        r#"["user:u1/cpu",{"cpu":12,"memory":2048}]"#,
    )
    .await;
    let usage = soon(cpu.changed()).await.unwrap();
    assert_eq!(usage.cpu, UserCpuCpu::Number(12.0));
    assert_eq!(usage.memory, Some(2048));
    server::send(&mut websocket, r#"["user:u1/cpu",{"cpu":"error"}]"#).await;
    assert_eq!(soon(cpu.changed()).await.unwrap().cpu, UserCpuCpu::Error);

    server::send(
        &mut websocket,
        r#"["user:u1/resources",{"credits":1.5,"pixel":3}]"#,
    )
    .await;
    let latest = soon(resources.changed()).await.unwrap();
    assert_eq!(latest.credits, 1.5);
    assert_eq!(latest.other.get("pixel"), Some(&3.0));
    assert_eq!(resources.latest(), Some(latest));

    drop(cpu);
    assert_eq!(
        receive(&mut websocket, 1).await,
        ["unsubscribe user:u1/cpu"]
    );
}

#[tokio::test]
async fn anonymous_sessions_need_a_user_for_the_account_but_not_for_users() {
    let server = Server::start(routes, |_| true).await;
    let profile = ServerProfile {
        url: server.url(),
        auth: Auth::Anonymous,
        server_password: None,
        default_shard: None,
    };
    let overseer = Overseer::connect(profile).await.unwrap();
    let account = overseer.account();
    assert!(matches!(account.me().await, Err(Error::Anonymous)));
    assert!(matches!(account.cpu(), Err(Error::Anonymous)));
    assert!(matches!(
        account.power_creeps().list().await,
        Err(Error::Anonymous)
    ));
    assert!(matches!(overseer.messages().feed(), Err(Error::Anonymous)));
    assert!(matches!(
        overseer.market().my_orders().await,
        Err(Error::Anonymous)
    ));
    // Nothing was sent for them.
    assert!(
        server
            .requests()
            .iter()
            .all(|request| request.path == "/api/version")
    );
    // Public profiles are open to anonymous sessions: the request reaches the server.
    assert!(matches!(
        overseer.users().find("Alice").await,
        Err(Error::Net(_))
    ));
    assert_eq!(server.requests().last().unwrap().path, "/api/user/find");
}
