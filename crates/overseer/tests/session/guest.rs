//! A server without an account ([`Guest`]): discovery, the checks of a new username or e-mail
//! address, registration and its field rules, on each contract that registers accounts.

use overseer::core::api::Refusal;
use overseer::model::account::{Availability, Registration};
use overseer::model::platform::Subsystem;
use overseer::{Error, Guest, net};

use crate::routes;
use crate::server::{Request, Server};

/// `GET /api/version` of the stock Screeps server: a community server, no features.
const COMMUNITY_VERSION: &str = r#"{"ok":1,"protocol":14,"useNativeAuth":false,"users":1,"serverData":{"customObjectTypes":{},"historyChunkSize":20,"socketUpdateThrottle":100,"renderer":{"resources":{},"metadata":{}}}}"#;

/// `GET /api/authmod` of screepsmod-auth.
const AUTHMOD: &str = r#"{"ok":1,"name":"screepsmod-auth","version":"2.10.1","allowRegistration":true,"steam":true,"github":false,"gitlab":false}"#;

/// The registration routes over [`routes`]: `taken` and `taken@example.com` are used
/// (`taken_error` answers them), everything else is free.
fn registration_routes(request: &Request, taken_error: &str) -> (u16, String) {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/api/register/check-username" | "/api/register/check-email")
            if request.query.contains("=taken") =>
        {
            (200, format!(r#"{{"error":"{taken_error}"}}"#))
        }
        ("GET", "/api/register/check-username" | "/api/register/check-email")
        | ("POST", "/api/register/submit") => (200, r#"{"ok":1}"#.to_owned()),
        _ => routes(request),
    }
}

/// A valid registration of `username`.
fn registration(username: &str) -> Registration {
    Registration {
        username: username.to_owned(),
        email: format!("{username}@example.com"),
        password: "long enough".to_owned(),
        modules: None,
    }
}

/// The paths requested from `server` other than discovery.
fn sent(server: &Server) -> Vec<String> {
    server
        .requests()
        .into_iter()
        .map(|request| request.path)
        .filter(|path| path != "/api/version" && path != "/api/authmod")
        .collect()
}

#[tokio::test]
async fn an_openscreeps_server_checks_and_registers_valid_accounts_only() {
    let server = Server::start(
        |request: &Request| registration_routes(request, "exists"),
        |_| true,
    )
    .await;
    let guest = Guest::discover(server.url(), None, None).await.unwrap();
    assert!(guest.can_register());
    assert!(guest.api().supports(Subsystem::PasswordAccounts));

    assert_eq!(
        guest.username_available("player").await.unwrap(),
        Availability::Available
    );
    assert_eq!(
        guest.username_available("taken").await.unwrap(),
        Availability::Taken
    );
    assert_eq!(
        guest.email_available("taken@example.com").await.unwrap(),
        Availability::Taken
    );
    guest.register(registration("player")).await.unwrap();
    let before = sent(&server);
    assert_eq!(
        before,
        [
            "/api/register/check-username",
            "/api/register/check-username",
            "/api/register/check-email",
            "/api/register/submit",
        ]
    );

    // Values that break a rule are not sent.
    assert!(matches!(
        guest.username_available("a b").await,
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        guest.email_available("not an address").await,
        Err(Error::InvalidInput(_))
    ));
    let mut short = registration("player");
    short.password = "short".to_owned();
    assert!(matches!(
        guest.register(short).await,
        Err(Error::InvalidInput(_))
    ));
    assert_eq!(sent(&server), before);
}

#[tokio::test]
async fn community_servers_register_only_with_screepsmod_auth() {
    let server = Server::start(
        |request: &Request| match request.path.as_str() {
            "/api/version" => (200, COMMUNITY_VERSION.to_owned()),
            "/api/authmod" => (200, AUTHMOD.to_owned()),
            _ => registration_routes(request, "User Exists"),
        },
        |_| true,
    )
    .await;
    let guest = Guest::discover(server.url(), None, None).await.unwrap();
    assert!(guest.can_register());
    assert_eq!(
        guest.username_available("taken").await.unwrap(),
        Availability::Taken
    );
    guest.register(registration("player")).await.unwrap();

    // The stock server without the plugin registers nothing: nothing is sent.
    let server = Server::start(
        |request: &Request| match request.path.as_str() {
            "/api/version" => (200, COMMUNITY_VERSION.to_owned()),
            _ => (404, r#"{"error":"not found"}"#.to_owned()),
        },
        |_| true,
    )
    .await;
    let guest = Guest::discover(server.url(), None, None).await.unwrap();
    assert!(!guest.can_register());
    let discovered = server.requests().len();
    match guest.register(registration("player")).await {
        Err(Error::Net(net::Error::Unsupported(subsystem))) => {
            assert_eq!(subsystem, Subsystem::PasswordAccounts);
        }
        other => panic!("registration should be unsupported: {other:?}"),
    }
    assert_eq!(server.requests().len(), discovered);
}

#[tokio::test]
async fn a_missing_server_password_is_a_refusal() {
    let server = Server::start(
        |_: &Request| (200, r#"{"error":"incorrect server password"}"#.to_owned()),
        |_| true,
    )
    .await;
    let error = Guest::discover(server.url(), None, None).await.unwrap_err();
    assert_eq!(error.refusal(), Some(Refusal::ServerPassword));
    let server = Server::start(
        |_: &Request| (401, r#"{"error":"unauthorized"}"#.to_owned()),
        |_| true,
    )
    .await;
    let error = Guest::discover(server.url(), None, None).await.unwrap_err();
    assert_eq!(error.refusal(), Some(Refusal::Unauthorized));
}
