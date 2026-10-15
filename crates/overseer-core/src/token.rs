//! Authentication tokens.

use std::fmt;

use serde::{Deserialize, Serialize};

/// An account API token or a server-issued session token.
///
/// The same token authenticates HTTP requests (`X-Token`) and the socket (`auth <token>`).
/// Servers may replace it: private servers issue short-lived session tokens with every
/// authenticated reply, so always keep the latest one. `Debug` output is redacted and there is
/// deliberately no `Display` implementation.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Token(String);

impl Token {
    /// Wraps a token value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The token value, for sending to the server that issued it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::Token;

    #[test]
    fn debug_output_never_contains_the_secret() {
        let token = Token::new("0123-secret");
        assert!(!format!("{token:?}").contains("secret"));
    }
}
