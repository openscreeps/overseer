//! The session's exemption from rate limits: kept by the session, given to the client's request
//! budget without a world map, and checked while an app follows it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use overseer::core::Token;
use overseer::model::platform::Subsystem;
use overseer::net::Priority;
use overseer::rate_limits::Exemption;
use overseer::{Auth, Error, Overseer, ServerProfile};

use crate::server::{Request, Server};
use crate::{connect, routes, soon};

/// `GET /api/version` of the stock Screeps server: a community server, no features.
const COMMUNITY_VERSION: &str = r#"{"ok":1,"protocol":14,"useNativeAuth":false,"users":1,"serverData":{"customObjectTypes":{},"historyChunkSize":20,"socketUpdateThrottle":100,"renderer":{"resources":{},"metadata":{}}}}"#;

/// `GET /api/authmod` of screepsmod-auth, which lifts rate limits.
const AUTHMOD: &str = r#"{"ok":1,"name":"screepsmod-auth","version":"2.10.1","allowRegistration":true,"steam":true,"github":false,"gitlab":false}"#;

/// The session's API token; the token list names its first 8 characters.
const TOKEN: &str = "0123abcd-4567-89ef-0123-456789abcdef";

/// Now, in milliseconds since the Unix epoch.
fn epoch_millis() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

/// A community server with screepsmod-auth whose token list exempts [`TOKEN`] until `until`
/// (milliseconds since the Unix epoch, 0 for never) and counts its requests in `lists`.
fn lift_routes(request: &Request, until: &AtomicU64, lists: &AtomicU64) -> (u16, String) {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/api/version") => (200, COMMUNITY_VERSION.to_owned()),
        ("GET", "/api/authmod") => (200, AUTHMOD.to_owned()),
        ("GET", "/api/auth/me") if request.token.is_some() => (
            200,
            r#"{"ok":1,"_id":"u1","username":"Alice","money":0,"powerExperimentations":0}"#
                .to_owned(),
        ),
        ("GET", "/api/user/auth-token") => {
            lists.fetch_add(1, Ordering::SeqCst);
            let until = match until.load(Ordering::SeqCst) {
                0 => String::new(),
                until => format!(r#","noRatelimitUntil":{until}"#),
            };
            (
                200,
                format!(
                    r#"{{"ok":1,"tokens":[{{"_id":"k1","full":true,"token":"0123abcd"{until}}}]}}"#
                ),
            )
        }
        _ => routes(request),
    }
}

/// A server of [`lift_routes`] and a session signed in with [`TOKEN`] (without a socket
/// handshake: these tests use HTTP only).
async fn start(until: &Arc<AtomicU64>, lists: &Arc<AtomicU64>) -> (Server, Overseer) {
    let (until, lists) = (Arc::clone(until), Arc::clone(lists));
    let server = Server::start(
        move |request| lift_routes(request, &until, &lists),
        |_| true,
    )
    .await;
    let profile = ServerProfile {
        url: server.url(),
        auth: Auth::Token(Token::new(TOKEN)),
        server_password: None,
        default_shard: None,
        server_kind: None,
        plugins: None,
    };
    let overseer = Overseer::connect(profile).await.unwrap();
    (server, overseer)
}

/// Waits until `lists` counts `count` requests.
async fn lists_reach(lists: &AtomicU64, count: u64) {
    soon(async {
        while lists.load(Ordering::SeqCst) < count {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
}

#[tokio::test]
async fn a_checked_exemption_reaches_the_request_budget_without_a_world_map() {
    let until = Arc::new(AtomicU64::new(epoch_millis() + 7_200_000));
    let lists = Arc::new(AtomicU64::new(0));
    let (_server, overseer) = start(&until, &lists).await;
    let client = overseer.client();
    let mut exemption = overseer.rate_limits().exemption();
    assert_eq!(*exemption.borrow_and_update(), Exemption::default());
    assert!(!client.is_rate_limit_exempt());

    // One check: the session keeps the answer and the client lets every request go.
    let expected = until.load(Ordering::SeqCst);
    assert_eq!(
        overseer.account().rate_limit_exemption().await.unwrap(),
        Some(expected)
    );
    assert!(exemption.has_changed().unwrap());
    assert_eq!(exemption.borrow_and_update().until, Some(expected));
    assert!(client.is_rate_limit_exempt());
    assert!(client.budget_ready_at(Priority::Background, 1000).is_none());

    // The exemption is gone: the next check takes it back.
    until.store(0, Ordering::SeqCst);
    assert_eq!(
        overseer.account().rate_limit_exemption().await.unwrap(),
        None
    );
    assert_eq!(exemption.borrow_and_update().until, None);
    assert!(!client.is_rate_limit_exempt());

    // Learned elsewhere: the same state, the same budget.
    overseer.rate_limits().set_exempt_until(Some(expected));
    assert!(client.is_rate_limit_exempt());
    assert_eq!(exemption.borrow_and_update().until, Some(expected));
    overseer.shutdown().await;
}

#[tokio::test]
async fn a_follower_checks_at_once_on_request_and_stops_with_the_last_handle() {
    let until = Arc::new(AtomicU64::new(epoch_millis() + 7_200_000));
    let lists = Arc::new(AtomicU64::new(0));
    let (_server, overseer) = start(&until, &lists).await;
    let limits = overseer.rate_limits();
    let mut exemption = limits.exemption();

    // Following checks at once; every follower shares the one check.
    let following = limits.follow().unwrap();
    let again = overseer.rate_limits().follow().unwrap();
    let settled = *soon(exemption.wait_for(|exemption| exemption.settled))
        .await
        .unwrap();
    assert_eq!(settled.until, Some(until.load(Ordering::SeqCst)));
    assert!(overseer.client().is_rate_limit_exempt());
    assert_eq!(lists.load(Ordering::SeqCst), 1);

    // Asked (for example after the user lifted the limits): one more check at once.
    limits.request_check();
    lists_reach(&lists, 2).await;
    // The user's attention within 15 s of a check waits for the throttle.
    limits.attention();
    following.set_held_back(true);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(lists.load(Ordering::SeqCst), 2);

    // The last handle gone, nothing is checked any more.
    drop(following);
    drop(again);
    tokio::time::sleep(Duration::from_millis(100)).await;
    limits.request_check();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(lists.load(Ordering::SeqCst), 2);
    overseer.shutdown().await;
}

#[tokio::test]
async fn only_servers_that_lift_rate_limits_can_be_followed() {
    let mut server = Server::start(routes, |_| true).await;
    let (overseer, _websocket) = connect(&mut server).await;
    let error = overseer.rate_limits().follow().unwrap_err();
    assert!(
        matches!(
            error,
            Error::Net(overseer::net::Error::Unsupported(Subsystem::RateLimitLift))
        ),
        "{error:?}"
    );
}
