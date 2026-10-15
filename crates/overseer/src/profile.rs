//! Where and as whom to connect.

use std::fmt;

use overseer_net::core::{ServerUrl, Token};

/// How the session authenticates.
///
/// `Debug` output never contains the token, e-mail or password.
#[derive(Clone)]
pub enum Auth {
    /// No account: HTTP endpoints that need none and the `server-message` channel only.
    Anonymous,
    /// An account API token (official server) or a session token from an earlier session.
    Token(Token),
    /// E-mail and password sign-in (`POST /api/auth/signin`, private servers). The session
    /// signs in on connect and again whenever the server rejects the session token.
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
