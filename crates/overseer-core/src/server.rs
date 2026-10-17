//! Server addresses.

use std::fmt;

use url::Url;

/// Base URL of a Screeps server: the official world, a seasonal world or a private server.
///
/// The base always ends with `/`, so servers hosted below a path prefix (for example
/// `https://screeps.com/season/`) resolve API, room-history and socket addresses under that
/// prefix.
///
/// It also knows which server it reaches ([`Self::is_official`]), also through a proxy
/// ([`Self::proxied`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ServerUrl {
    base: Url,
    socket: Url,
    official: bool,
}

/// A server URL that cannot address a Screeps server.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServerUrlError {
    /// The text is not a URL, or a path does not resolve against it.
    #[error("invalid server URL: {0}")]
    Parse(#[from] url::ParseError),
    /// Only `http` and `https` servers are supported.
    #[error("server URL must use http or https, not {0}")]
    Scheme(String),
    /// Credentials, queries and fragments would leak into every request.
    #[error("server URL must not carry credentials, a query or a fragment")]
    Extra,
}

/// The host of every official server ([`ServerUrl::is_official`]).
const OFFICIAL_HOST: &str = "screeps.com";

impl ServerUrl {
    /// The official Screeps World.
    pub const OFFICIAL: &'static str = "https://screeps.com/";

    /// The official Screeps World at [`Self::OFFICIAL`].
    ///
    /// # Panics
    ///
    /// Never: [`Self::OFFICIAL`] is a valid `https` URL.
    #[must_use]
    pub fn official() -> Self {
        Self::parse(Self::OFFICIAL).expect("official server URL is valid")
    }

    /// Parses an `http` or `https` server base URL.
    ///
    /// # Errors
    ///
    /// Returns [`ServerUrlError`] for unparsable URLs, other schemes, and URLs with credentials,
    /// a query or a fragment.
    pub fn parse(input: &str) -> Result<Self, ServerUrlError> {
        let mut base = Url::parse(input)?;
        let socket_scheme = match base.scheme() {
            "http" => "ws",
            "https" => "wss",
            other => return Err(ServerUrlError::Scheme(other.to_owned())),
        };
        if !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(ServerUrlError::Extra);
        }
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        let mut socket = base.join("socket/websocket")?;
        socket
            .set_scheme(socket_scheme)
            .map_err(|()| ServerUrlError::Scheme(socket_scheme.to_owned()))?;
        let official = base.host_str() == Some(OFFICIAL_HOST);
        Ok(Self {
            base,
            socket,
            official,
        })
    }

    /// The address `proxy` of something that forwards every request to `server` unchanged,
    /// such as `overseer-proxy`: requests go to `proxy`, and it is the server `server` is
    /// ([`Self::is_official`]).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::parse`] for `proxy`.
    pub fn proxied(proxy: &str, server: &Self) -> Result<Self, ServerUrlError> {
        Ok(Self {
            official: server.official,
            ..Self::parse(proxy)?
        })
    }

    /// Whether this is a screeps.com server (the persistent world, `/season/`, `/ptr/`), which
    /// follows the official contract: socket channels, room history and shard-routed requests
    /// name a shard. Every other server is a private server (`openscreeps/server`, or the
    /// Screeps server with community mods) following the standalone profile: one world whose
    /// channels, history and requests name no shard, whatever shards its `GET /api/version`
    /// lists. The official web client draws the same line (`options.official`).
    #[must_use]
    pub const fn is_official(&self) -> bool {
        self.official
    }

    /// The base URL, ending with `/`.
    #[must_use]
    pub const fn base(&self) -> &Url {
        &self.base
    }

    /// The URL of a path relative to the base, for example `api/game/time` or
    /// `room-history/shard0/W1N1/100.json`.
    ///
    /// # Errors
    ///
    /// Returns [`ServerUrlError::Parse`] when `path` does not resolve against the base.
    pub fn join(&self, path: &str) -> Result<Url, ServerUrlError> {
        Ok(self.base.join(path)?)
    }

    /// The raw WebSocket transport of the socket endpoint (`ws(s)://…/socket/websocket`).
    #[must_use]
    pub const fn socket(&self) -> &Url {
        &self.socket
    }
}

impl fmt::Display for ServerUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.base.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::{ServerUrl, ServerUrlError};

    #[test]
    fn prefixed_servers_keep_their_prefix_for_paths_and_socket() {
        let season = ServerUrl::parse("https://screeps.com/season").unwrap();
        assert_eq!(season.base().as_str(), "https://screeps.com/season/");
        assert_eq!(
            season.join("api/game/time").unwrap().as_str(),
            "https://screeps.com/season/api/game/time"
        );
        assert_eq!(
            season
                .join("room-history/shard0/W1N1/100.json")
                .unwrap()
                .as_str(),
            "https://screeps.com/season/room-history/shard0/W1N1/100.json"
        );
        assert_eq!(
            season.socket().as_str(),
            "wss://screeps.com/season/socket/websocket"
        );
        let local = ServerUrl::parse("http://127.0.0.1:21025").unwrap();
        assert_eq!(
            local.socket().as_str(),
            "ws://127.0.0.1:21025/socket/websocket"
        );
    }

    #[test]
    fn only_screeps_com_is_official_also_through_a_proxy() {
        for official in [
            ServerUrl::OFFICIAL,
            "https://SCREEPS.com:443/season",
            "https://screeps.com/ptr/",
        ] {
            assert!(
                ServerUrl::parse(official).unwrap().is_official(),
                "{official}"
            );
        }
        for private in [
            "http://localhost:21025/",
            "https://screeps.com.example/",
            "https://www.example.com/screeps.com/",
        ] {
            assert!(
                !ServerUrl::parse(private).unwrap().is_official(),
                "{private}"
            );
        }
        let proxy = "http://127.0.0.1:8080/";
        let fronting = ServerUrl::proxied(proxy, &ServerUrl::official()).unwrap();
        assert!(fronting.is_official());
        assert_eq!(
            fronting.socket().as_str(),
            "ws://127.0.0.1:8080/socket/websocket"
        );
        let local = ServerUrl::parse("http://localhost:21025/").unwrap();
        assert!(!ServerUrl::proxied(proxy, &local).unwrap().is_official());
    }

    #[test]
    fn rejects_urls_that_would_leak_into_requests() {
        assert_eq!(
            ServerUrl::parse("ftp://screeps.com/"),
            Err(ServerUrlError::Scheme("ftp".to_owned()))
        );
        for leaky in [
            "https://user:pass@screeps.com/",
            "https://screeps.com/?token=x",
            "https://screeps.com/#x",
        ] {
            assert_eq!(
                ServerUrl::parse(leaky),
                Err(ServerUrlError::Extra),
                "{leaky}"
            );
        }
    }
}
