//! The Overseer sessions of `upload` and `poll`. The library does the
//! discovery, the sign-in, HTTP, the sockets and the token rotation.

use overseer::core::ServerUrl;
use overseer::{Auth, Error, Overseer, ServerProfile};

/// Connects to the server at `url` with `auth` through the `overseer`
/// library. The caller must call `shutdown` on the session before its command
/// returns. This rule also applies when the command fails.
pub(crate) async fn connect(url: &ServerUrl, auth: Auth) -> Result<Overseer, Error> {
    Overseer::connect(ServerProfile {
        url: url.clone(),
        auth,
        server_password: None,
        default_shard: None,
        server_kind: None,
        plugins: None,
    })
    .await
}
