//! Console area: expressions for the signed-in user's runtime.
//!
//! The contracts send the same request; the replies differ (screeps.com echoes the queued
//! console document, `OpenScreeps` answers `{ok: 1}`) but carry nothing to read, so they are
//! dropped: the output arrives on the socket's console channel.

use overseer_core::api::{community, official, openscreeps};

use super::{Api, Community, Official, OpenScreeps, delegate};
use crate::Error;

/// The signed-in user's console.
pub trait ConsoleApi {
    /// Queues `expression` for the signed-in user's runtime on `shard` (`None`: the
    /// [`World`](crate::World)'s default shard; single-world servers never send one) via
    /// `POST /api/user/console`. Its output and result arrive on the console channel
    /// ([`Socket::subscribe_console`](crate::Socket::subscribe_console)), not in the reply.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send); an expression whose JSON
    /// form exceeds 1024 characters is rejected by the server.
    fn console(
        &self,
        expression: &str,
        shard: Option<&str>,
    ) -> impl Future<Output = Result<(), Error>>;
}

impl ConsoleApi for Api {
    async fn console(&self, expression: &str, shard: Option<&str>) -> Result<(), Error> {
        delegate!(self, api => api.console(expression, shard).await)
    }
}

/// `impl ConsoleApi for $api`, sending the endpoint of the contract module `$contract`.
macro_rules! console_api {
    ($($api:ty => $contract:ident),+ $(,)?) => {$(
        impl ConsoleApi for $api {
            async fn console(&self, expression: &str, shard: Option<&str>) -> Result<(), Error> {
                let body = $contract::types::UserConsoleRequest {
                    expression: expression.to_owned(),
                    shard: self.client.resolve_shard(shard).await?,
                };
                self.client
                    .send(&$contract::endpoints::UserConsole { body })
                    .await?;
                Ok(())
            }
        }
    )+};
}

console_api!(Official => official, OpenScreeps => openscreeps, Community => community);

#[cfg(test)]
mod tests {
    use overseer_core::api::{Endpoint, Session};
    use overseer_core::{ServerUrl, Token};

    use super::{official, openscreeps};

    /// Decodes `body` as a 200 reply to `E`.
    fn decodes<E: Endpoint>(body: &str) -> bool {
        let mut session = Session::new(ServerUrl::official(), Some(Token::new("test-token")));
        let reply = http::Response::builder().status(200).body(body).unwrap();
        session.response::<E, _>(reply).is_ok()
    }

    #[test]
    fn both_console_replies_decode() {
        // screeps.com echoes the inserted console document and the driver's metadata.
        assert!(decodes::<official::endpoints::UserConsole>(
            r#"{"ok":1,"result":{"ok":1,"n":1},"ops":[{"user":"5a1b2c3d4e5f60718293a4b5",
                "expression":"Game.time","shard":"shard3","_id":"65f0c0ffee0123456789abcd"}],
                "insertedCount":1,"insertedIds":["65f0c0ffee0123456789abcd"]}"#
        ));
        assert!(decodes::<openscreeps::endpoints::UserConsole>(
            r#"{"ok":1}"#
        ));
    }
}
