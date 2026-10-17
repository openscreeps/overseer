//! Sign-in, registration and session tokens.

use overseer_net::core::Token;
use overseer_net::core::api::types::RegisterSubmitRequest;
use overseer_net::core::api::{ApiError, endpoints};
use overseer_net::{Client, Error};

use crate::support::{self, Account};

/// The message of a logical failure (`{error}` with HTTP 200), as `ApiError::Server` surfaces it.
fn server_error<T: std::fmt::Debug>(result: Result<T, Error>) -> String {
    match result {
        Err(Error::Api(ApiError::Server(message))) => message,
        other => panic!("expected a server error, got {other:?}"),
    }
}

/// Whether `result` is HTTP 401.
fn is_unauthorized<T>(result: &Result<T, Error>) -> bool {
    matches!(result, Err(Error::Api(ApiError::Unauthorized)))
}

/// Submits a registration.
async fn submit(body: RegisterSubmitRequest) -> Result<(), Error> {
    support::anonymous()
        .send(&endpoints::RegisterSubmit { body })
        .await
        .map(drop)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn credentials_sign_in_and_discover_the_user() {
    let account = support::register().await;
    let client = account.client();
    assert!(client.token().is_none());
    let world = client.world().await.expect("the account signs in");
    assert!(client.token().is_some());
    assert!(!world.is_sharded(), "a single world");
    assert_eq!(world.shard(), None);
    assert_eq!(world.history_chunk_size(), 20);
    let me = client.send(&endpoints::AuthMe).await.expect("auth/me").body;
    assert_eq!(Some(me.id.as_str()), world.user());
    assert_eq!(me.username.as_deref(), Some(account.username.as_str()));
    assert_eq!(me.email.as_deref(), Some(account.email.as_str()));
    assert_eq!(me.password, Some(true));

    // The e-mail signs in too, and so does the harness's player.
    let by_email = Client::builder(support::url())
        .credentials(account.email.clone(), account.password.clone())
        .build()
        .expect("a client");
    assert_eq!(
        by_email.world().await.expect("e-mail sign-in").user(),
        world.user()
    );
    let admin = support::admin()
        .client()
        .world()
        .await
        .expect("admin sign-in");
    assert!(admin.user().is_some());
    assert_ne!(admin.user(), world.user());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn wrong_passwords_and_unknown_users_are_unauthorized() {
    let account = support::register().await;
    let wrong = Account {
        password: format!("{}-wrong", account.password),
        ..account.clone()
    };
    let result = wrong.client().world().await;
    assert!(is_unauthorized(&result), "{result:?}");
    assert!(wrong.client().token().is_none());

    let unknown = support::new_account();
    let result = unknown.client().world().await;
    assert!(is_unauthorized(&result), "{result:?}");

    // The sign-in endpoint itself answers HTTP 401 `unauthorized`.
    let signin = endpoints::AuthSignin {
        body: overseer_net::core::api::types::AuthSigninRequest {
            email: account.username.clone(),
            password: "not-the-password".to_owned(),
            recaptcha: None,
            steam_ticket: None,
        },
    };
    let result = support::anonymous().send(&signin).await;
    assert!(is_unauthorized(&result), "{result:?}");

    // A token the server never issued is rejected, and a token-only client cannot sign in.
    let bogus = support::with_token(Token::new("0".repeat(64)));
    let result = bogus.send(&endpoints::AuthMe).await;
    assert!(is_unauthorized(&result), "{result:?}");
}

/// Registration checks tell free names and e-mails from taken ones with the server's messages.
///
/// `openscreeps/server` compares case-insensitively and refuses empty input. A community server
/// answers through screepsmod-auth 2.10.1, which looks the exact string up and validates
/// nothing (`lib/register.js`, <https://unpkg.com/screepsmod-auth@2.10.1/lib/register.js>): a
/// taken value is `User Exists`, another case of it is free.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn registration_checks_tell_available_from_taken() {
    let account = support::register().await;
    let client = support::anonymous();
    let username = |username: &str| endpoints::RegisterCheckUsername {
        username: username.to_owned(),
    };
    let email = |email: &str| endpoints::RegisterCheckEmail {
        email: email.to_owned(),
    };
    let free = support::new_account();
    client
        .send(&username(&free.username))
        .await
        .expect("a free username is available");
    client
        .send(&email(&free.email))
        .await
        .expect("a free e-mail is available");
    if support::community() {
        assert_eq!(
            server_error(client.send(&username(&account.username)).await),
            "User Exists"
        );
        assert_eq!(
            server_error(client.send(&email(&account.email)).await),
            "User Exists"
        );
        client
            .send(&username(&account.username.to_uppercase()))
            .await
            .expect("usernames compare case-sensitively");
        client
            .send(&email(&account.email.to_uppercase()))
            .await
            .expect("e-mails compare case-sensitively");
        return;
    }
    assert_eq!(
        server_error(client.send(&username(&account.username)).await),
        "exists"
    );
    // Usernames and e-mails compare case-insensitively.
    assert_eq!(
        server_error(
            client
                .send(&username(&account.username.to_uppercase()))
                .await
        ),
        "exists"
    );
    assert_eq!(
        server_error(client.send(&email(&account.email)).await),
        "exists"
    );
    assert_eq!(
        server_error(client.send(&email(&account.email.to_uppercase())).await),
        "exists"
    );
    assert_eq!(
        server_error(client.send(&username("")).await),
        "invalid username"
    );
    assert_eq!(server_error(client.send(&email("")).await), "invalid email");
}

/// Registrations the server refuses come back with its message and create nothing.
///
/// `openscreeps/server` validates every field and compares names case-insensitively. A community
/// server registers through screepsmod-auth 2.10.1, which validates nothing and only refuses a
/// username or e-mail that is taken exactly, answering with the error's stack trace
/// (`lib/register.js`, <https://unpkg.com/screepsmod-auth@2.10.1/lib/register.js>).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn registration_rejects_invalid_accounts_with_the_server_messages() {
    let taken = support::register().await;
    let fresh = support::new_account();
    let request = |change: &dyn Fn(&mut RegisterSubmitRequest)| {
        let mut body = support::registration(&fresh, None);
        change(&mut body);
        body
    };
    let cases: Vec<(&str, RegisterSubmitRequest)> = if support::community() {
        vec![
            (
                "Error: User already exists",
                request(&|body| body.username.clone_from(&taken.username)),
            ),
            (
                "Error: User already exists",
                request(&|body| body.email.clone_from(&taken.email)),
            ),
        ]
    } else {
        vec![
            (
                "username already exists",
                request(&|body| body.username = taken.username.to_uppercase()),
            ),
            (
                "email already exists",
                request(&|body| body.email = taken.email.to_uppercase()),
            ),
            (
                "invalid username",
                request(&|body| "ab".clone_into(&mut body.username)),
            ),
            (
                "invalid username",
                request(&|body| body.username = format!("{} x", fresh.username)),
            ),
            (
                "password must be 8-1024 characters long",
                request(&|body| "short".clone_into(&mut body.password)),
            ),
            ("invalid email", request(&|body| body.email.clear())),
            (
                "invalid email",
                request(&|body| "not-an-email".clone_into(&mut body.email)),
            ),
        ]
    };
    for (message, body) in cases {
        let error = server_error(submit(body).await);
        if support::community() {
            // screepsmod-auth's message goes on with the stack trace, one frame per line.
            assert_eq!(error.lines().next(), Some(message), "{error}");
        } else {
            assert_eq!(error, message);
        }
    }
    // None of the rejected attempts created the account; the valid one does.
    support::anonymous()
        .send(&endpoints::RegisterCheckUsername {
            username: fresh.username.clone(),
        })
        .await
        .expect("still available");
    submit(support::registration(&fresh, None))
        .await
        .expect("the valid registration succeeds");
    let world = fresh
        .client()
        .world()
        .await
        .expect("the new account signs in");
    assert!(world.user().is_some());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn session_tokens_rotate_on_every_authenticated_reply() {
    let account = support::register().await;
    let client = account.client();
    let mut updates = client.token_updates();
    assert_eq!(*updates.borrow_and_update(), None);
    assert!(!client.rotates_tokens());

    // Sign-in, then `auth/me` with the issued token, whose reply rotates it.
    let user = client
        .world()
        .await
        .expect("sign-in")
        .user()
        .map(str::to_owned);
    assert!(client.rotates_tokens(), "auth/me replied with X-Token");
    let first = client.token().expect("a token");
    assert!(updates.has_changed().expect("the client is alive"));
    assert_eq!(updates.borrow_and_update().as_ref(), Some(&first));

    client.send(&endpoints::AuthMe).await.expect("auth/me");
    let second = client.token().expect("a token");
    assert_ne!(second, first, "every authenticated reply rotates the token");
    assert_eq!(second.as_str().len(), support::token_length());
    assert!(second.as_str().bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert!(updates.has_changed().expect("the client is alive"));
    assert_eq!(updates.borrow_and_update().as_ref(), Some(&second));
    // Clones share the token.
    assert_eq!(client.clone().token(), Some(second.clone()));

    // The client sends the held token with every request, as the official web client does, and
    // adopts a token from any reply. OpenScreeps rotates it only on authenticated endpoints, so
    // an anonymous one leaves it alone; screepsmod-auth rotates it on every request carrying a
    // valid token, whatever the endpoint (its `X-Token` middleware, `lib/backend.js` L219-L245
    // at 9195e87), so the client takes the new one.
    client
        .send(&endpoints::PlatformVersion)
        .await
        .expect("version");
    let current = client.token().expect("a token");
    if support::community() {
        assert_ne!(current, second, "screepsmod-auth rotates on every request");
        assert!(updates.has_changed().expect("the client is alive"));
        assert_eq!(updates.borrow_and_update().as_ref(), Some(&current));
    } else {
        assert_eq!(current, second);
        assert!(!updates.has_changed().expect("the client is alive"));
    }

    // The current token authenticates another client of the same account.
    let other = support::with_token(current);
    let me = other.send(&endpoints::AuthMe).await.expect("auth/me").body;
    assert_eq!(Some(me.id), user);
    assert!(other.rotates_tokens());
}
