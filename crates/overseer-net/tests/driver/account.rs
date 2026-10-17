//! Account operations whose shape differs per server: the registration checks (`exists`
//! natively, `User Exists` from screepsmod-auth), registration and password changes through
//! the password accounts, and the start room.

use overseer_core::Token;
use overseer_core::model::account::{Availability, Registration, StartRoom};
use overseer_net::api::{AccountApi as _, PasswordAccount as _};
use overseer_net::{Client, Error};

use crate::server::{Request, Server};
use crate::{ME, VERSION};

/// `GET /api/version` of the stock Screeps server: no features.
const STOCK_VERSION: &str = r#"{"ok":1,"protocol":14,"users":1,"serverData":{"customObjectTypes":{},"historyChunkSize":20,"renderer":{"resources":{},"metadata":{}}},"packageVersion":"4.2.21"}"#;

/// `GET /api/authmod` of screepsmod-auth.
const AUTHMOD: &str = r#"{"ok":1,"name":"screepsmod-auth","version":"2.10.1","allowRegistration":true,"steam":true,"github":false,"gitlab":false}"#;

/// A server answering `version`, `GET /api/authmod` with `authmod` (HTTP 404 without),
/// `GET /api/auth/me` and the registration routes: the name `taken` and the address
/// `taken@example.com` are used, answered with the error `exists`.
fn routes(
    version: &'static str,
    authmod: Option<&'static str>,
    exists: &'static str,
) -> impl Fn(&Request) -> (u16, String) {
    move |request| match request.path.as_str() {
        "/api/version" => (200, version.to_owned()),
        "/api/authmod" => match authmod {
            Some(reply) => (200, reply.to_owned()),
            None => (404, "Not Found".to_owned()),
        },
        "/api/auth/me" => (200, ME.to_owned()),
        "/api/register/check-username" | "/api/register/check-email" => {
            if request.query.contains("=taken") {
                (200, format!(r#"{{"error":"{exists}"}}"#))
            } else if request.query.ends_with('=') {
                (200, r#"{"error":"invalid username"}"#.to_owned())
            } else {
                (200, r#"{"ok":1}"#.to_owned())
            }
        }
        "/api/register/submit" | "/api/user/password" => (200, r#"{"ok":1}"#.to_owned()),
        "/api/user/world-start-room" => (200, r#"{"ok":1,"room":["W5N5"]}"#.to_owned()),
        _ => (404, "Not Found".to_owned()),
    }
}

/// The first request to `path`.
fn request(server: &Server, path: &str) -> Request {
    server
        .requests()
        .into_iter()
        .find(|request| request.path == path)
        .unwrap_or_else(|| panic!("no request to {path}"))
}

/// A client of `server` signed in with a token.
fn client(server: &Server) -> Client {
    Client::builder(server.url())
        .token(Token::new("t"))
        .build()
        .unwrap()
}

/// Checks the taken and a free username and address of `client`'s server.
async fn checks(client: &Client) {
    let api = client.api().await.unwrap();
    assert_eq!(
        api.username_available("taken").await.unwrap(),
        Availability::Taken
    );
    assert_eq!(
        api.username_available("free").await.unwrap(),
        Availability::Available
    );
    assert_eq!(
        api.email_available("taken@example.com").await.unwrap(),
        Availability::Taken
    );
    assert_eq!(
        api.email_available("free@example.com").await.unwrap(),
        Availability::Available
    );
    // Other errors stay errors.
    assert!(api.username_available("").await.is_err());
}

/// The registration of these tests.
fn registration() -> Registration {
    Registration {
        username: "Alice".to_owned(),
        email: "alice@example.com".to_owned(),
        password: "secret-password".to_owned(),
        modules: None,
    }
}

#[tokio::test]
async fn openscreeps_checks_registers_and_changes_passwords_natively() {
    let server = Server::start(routes(VERSION, None, "exists"), |_| true).await;
    let client = client(&server);
    checks(&client).await;
    let api = client.api().await.unwrap();
    let accounts = api.password_account().unwrap();
    accounts.register(registration()).await.unwrap();
    assert_eq!(
        request(&server, "/api/register/submit").body,
        r#"{"email":"alice@example.com","password":"secret-password","username":"Alice"}"#
    );
    accounts.set_password(None, "new-password").await.unwrap();
    // `OpenScreeps` requires the current password; an account without one sends it empty.
    assert_eq!(
        request(&server, "/api/user/password").body,
        r#"{"oldPassword":"","password":"new-password"}"#
    );
    // Single-world: the shard is neither sent nor reported.
    assert_eq!(
        api.start_room(Some("shard0")).await.unwrap(),
        Some(StartRoom {
            shard: None,
            room: "W5N5".to_owned(),
        })
    );
    assert_eq!(request(&server, "/api/user/world-start-room").query, "");
}

#[tokio::test]
async fn community_checks_follow_screepsmod_auth_when_it_runs() {
    let base = routes(STOCK_VERSION, Some(AUTHMOD), "User Exists");
    let server = Server::start(
        move |request: &Request| match request.path.as_str() {
            // An account without a password (a Steam sign-up).
            "/api/auth/me" => (
                200,
                r#"{"ok":1,"_id":"u1","username":"Alice","money":0,"powerExperimentations":0,"password":false}"#.to_owned(),
            ),
            _ => base(request),
        },
        |_| true,
    )
    .await;
    let client = client(&server);
    checks(&client).await;
    let api = client.api().await.unwrap();
    let accounts = api.password_account().unwrap();
    accounts.register(registration()).await.unwrap();
    accounts.set_password(None, "new-password").await.unwrap();
    // screepsmod-auth takes no current password for an account without one, and none is
    // checked.
    assert_eq!(
        request(&server, "/api/user/password").body,
        r#"{"password":"new-password"}"#
    );
    assert!(paths(&server).iter().all(|path| path != "/api/auth/signin"));
}

/// The paths requested so far.
fn paths(server: &Server) -> Vec<String> {
    server
        .requests()
        .into_iter()
        .map(|request| request.path)
        .collect()
}

#[tokio::test]
async fn screepsmod_auth_password_changes_check_the_current_password_first() {
    // screepsmod-auth erases the password when sent a wrong current one: Overseer signs in
    // with it first and sends the change only when that works.
    let base = routes(STOCK_VERSION, Some(AUTHMOD), "User Exists");
    let server = Server::start(
        move |request: &Request| match request.path.as_str() {
            "/api/auth/me" => (
                200,
                r#"{"ok":1,"_id":"u1","username":"Alice","money":0,"powerExperimentations":0,"password":true}"#.to_owned(),
            ),
            "/api/auth/signin" if request.body.contains(r#""password":"right""#) => {
                (200, r#"{"ok":1,"token":"s"}"#.to_owned())
            }
            "/api/auth/signin" => (401, r#"{"error":"unauthorized"}"#.to_owned()),
            _ => base(request),
        },
        |_| true,
    )
    .await;
    let client = client(&server);
    let api = client.api().await.unwrap();
    let accounts = api.password_account().unwrap();
    let discovered = server.requests().len();

    let wrong = accounts.set_password(Some("wrong"), "new-password").await;
    assert!(matches!(wrong, Err(Error::WrongPassword)), "{wrong:?}");
    let missing = accounts.set_password(None, "new-password").await;
    assert!(matches!(missing, Err(Error::WrongPassword)), "{missing:?}");
    assert_eq!(
        paths(&server)[discovered..],
        ["/api/auth/me", "/api/auth/signin", "/api/auth/me"],
        "no change was sent"
    );
    assert_eq!(
        request(&server, "/api/auth/signin").body,
        r#"{"email":"Alice","password":"wrong"}"#
    );

    accounts
        .set_password(Some("right"), "new-password")
        .await
        .unwrap();
    assert_eq!(
        paths(&server)[discovered + 3..],
        ["/api/auth/me", "/api/auth/signin", "/api/user/password"]
    );
    assert_eq!(
        request(&server, "/api/user/password").body,
        r#"{"oldPassword":"right","password":"new-password"}"#
    );
}

#[tokio::test]
async fn openscreeps_wrong_current_passwords_are_told_alike() {
    let base = routes(VERSION, None, "exists");
    let server = Server::start(
        move |request: &Request| match request.path.as_str() {
            "/api/user/password" => (200, r#"{"error":"invalid password"}"#.to_owned()),
            _ => base(request),
        },
        |_| true,
    )
    .await;
    let api = client(&server).api().await.unwrap();
    let wrong = api
        .password_account()
        .unwrap()
        .set_password(Some("wrong"), "new-password")
        .await;
    assert!(matches!(wrong, Err(Error::WrongPassword)), "{wrong:?}");
}

#[tokio::test]
async fn community_checks_without_plugins_answer_natively() {
    let server = Server::start(routes(STOCK_VERSION, None, "exists"), |_| true).await;
    let client = client(&server);
    checks(&client).await;
    assert!(client.api().await.unwrap().password_account().is_none());
}
