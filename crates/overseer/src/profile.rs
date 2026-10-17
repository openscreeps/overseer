//! Where and as whom to connect.

use std::fmt;

use overseer_net::core::{ServerUrl, Token};
use overseer_net::model::platform::{Plugins, ServerKind};

/// How the session authenticates.
///
/// `Debug` output never contains the token, e-mail or password.
#[derive(Clone)]
pub enum Auth {
    /// No account: HTTP endpoints that need none and the `server-message` channel only.
    Anonymous,
    /// An account API token (where the server has API tokens) or a session token from an
    /// earlier session.
    ///
    /// Session tokens of private servers expire a minute (by default) after they were issued
    /// and are replaced with every authenticated reply: while the session runs it keeps its
    /// token valid with a cheap request whenever it has been idle for 25 seconds, and
    /// [`Overseer::tokens`](crate::Overseer::tokens) reports each new token to persist. API
    /// tokens, which the server never replaces, cause no extra requests. See the crate's
    /// [session lifecycle](crate#session-lifecycle).
    Token(Token),
    /// E-mail (or username) and password sign-in (`POST /api/auth/signin`), on servers with
    /// password sign-in. The session signs in on connect and again whenever the server rejects
    /// the session token.
    Credentials {
        /// Account e-mail (or username, as the server accepts).
        email: String,
        /// Account password.
        password: String,
    },
}

impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Anonymous => f.write_str("Anonymous"),
            Self::Token(_) => f.write_str("Token(<redacted>)"),
            Self::Credentials { .. } => f.write_str("Credentials(<redacted>)"),
        }
    }
}

/// One account on one server: everything [`Overseer::connect`](crate::Overseer::connect)
/// needs.
///
/// Persisting profiles and storing their secrets is the app's job. `Debug` output never
/// contains secrets.
#[derive(Clone)]
pub struct ServerProfile {
    /// The server, for example [`ServerUrl::official`].
    pub url: ServerUrl,
    /// How to authenticate.
    pub auth: Auth,
    /// `X-Server-Password` of a password-protected private server.
    pub server_password: Option<String>,
    /// The shard used when an operation names none (official server); `None` uses the first
    /// shard the server lists. Ignored by single-world servers.
    pub default_shard: Option<String>,
    /// Forces the contract the server speaks instead of discovering it
    /// ([`ClientBuilder::server_kind`](crate::net::ClientBuilder::server_kind)); `None`
    /// discovers it. The session's [subsystems](crate::net::World::subsystems) follow from it.
    pub server_kind: Option<ServerKind>,
    /// Forces the plugins a community server runs instead of detecting them
    /// ([`ClientBuilder::plugins`](crate::net::ClientBuilder::plugins)); `None` detects them.
    pub plugins: Option<Plugins>,
}

impl fmt::Debug for ServerProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerProfile")
            .field("url", &self.url)
            .field("auth", &self.auth)
            .field(
                "server_password",
                &self.server_password.as_ref().map(|_| "<redacted>"),
            )
            .field("default_shard", &self.default_shard)
            .field("server_kind", &self.server_kind)
            .field("plugins", &self.plugins)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use overseer_net::core::{ServerUrl, Token};

    use super::{Auth, ServerProfile};

    #[test]
    fn debug_output_never_contains_secrets() {
        let mut profile = ServerProfile {
            url: ServerUrl::official(),
            auth: Auth::Token(Token::new("token-secret")),
            server_password: Some("server-secret".to_owned()),
            default_shard: Some("shard3".to_owned()),
            server_kind: None,
            plugins: None,
        };
        let token = format!("{profile:?}");
        profile.auth = Auth::Credentials {
            email: "mail-secret@example.com".to_owned(),
            password: "password-secret".to_owned(),
        };
        let credentials = format!("{profile:?}");
        for debug in [token, credentials] {
            assert!(!debug.contains("secret"), "{debug}");
            assert!(debug.contains("shard3"), "{debug}");
        }
    }
}
