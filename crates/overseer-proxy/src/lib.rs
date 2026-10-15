//! A small proxy in front of one Screeps server, so that browser builds of `overseer` can reach
//! servers that send no CORS headers, such as screeps.com.
//!
//! `overseer_core::ServerUrl` derives every address from one base URL (`api/…`,
//! `room-history/…`, `socket/websocket`), so a client pointed at the proxy's URL works unchanged.
//! The proxy forwards exactly these paths to its one upstream base and nothing else:
//!
//! | Path | Forwarding |
//! |---|---|
//! | `api/*` | HTTP; method, query and body unchanged |
//! | `room-history/*` | HTTP; `Content-Encoding` passes through (gzip history files) |
//! | `socket/websocket` | WebSocket upgrade; frames relayed in both directions |
//!
//! A path prefix of the upstream base is kept: with `https://screeps.com/season/`,
//! `/api/auth/me` is forwarded to `https://screeps.com/season/api/auth/me`.
//! `GET /overseer-proxy.json` answers `{"upstream": "<upstream base>"}`, so a page served by the
//! proxy can tell which server it fronts. Every other path is served from the static directory
//! when one is configured, and is `404 Not Found` otherwise.
//!
//! # Serving modes
//!
//! - **Same origin**: the proxy also serves the web build ([`ProxyConfig::static_dir`]), so the
//!   page and the API share one origin and no CORS is involved. This is the default setup of the
//!   `overseer-proxy` binary for the web.
//! - **Cross origin**: the page lives elsewhere (the Tauri webview, a development server) and its
//!   origin is listed in [`ProxyConfig::allowed_origins`]. Requests and preflights from a listed
//!   origin are answered with `Access-Control-Allow-Origin` set to that origin,
//!   `Access-Control-Allow-Headers: X-Token, X-Username, X-Server-Password, Content-Type` and
//!   `Access-Control-Expose-Headers: X-Token, X-RateLimit-Limit, X-RateLimit-Remaining,
//!   X-RateLimit-Reset`. Other origins get no CORS headers (browsers then refuse to hand the
//!   response to the page), their preflights are refused, and so are their socket upgrades
//!   (sockets are not subject to CORS, so the proxy checks `Origin` itself). Requests
//!   without `Origin` (native clients) and upgrades from the proxy's own origin are accepted.
//!
//! # What passes through
//!
//! - Request headers: only `X-Token`, `X-Username`, `X-Server-Password`, `Content-Type` and
//!   `Accept`. No cookies, no `Authorization`, no `Origin`, no forwarding headers; the proxy
//!   sends its own `User-Agent`.
//! - Responses: the status unchanged (HTTP 200 logical failures, 401, 429, …) and only the
//!   headers `X-Token` (rotated session tokens), `X-RateLimit-Limit`, `X-RateLimit-Remaining`,
//!   `X-RateLimit-Reset`, `Content-Type` and `Content-Encoding`.
//! - Redirects are never followed: the redirect status reaches the client without its
//!   `Location`, so neither the proxy nor the client carries a token to another host.
//! - Socket frames (text and binary) are relayed unchanged in both directions, and so are pings,
//!   pongs and close frames (with their code and reason). Each side's WebSocket library also
//!   answers pings itself, so a client may see an extra pong. A side that goes away without a
//!   close frame closes the other side (`1001 Going Away` towards the upstream, `1011` towards
//!   the client).
//! - No caching, request rewriting or rate-limit handling: 429s and `X-RateLimit-*` reach the
//!   client, which retries on its own.
//!
//! # Limits
//!
//! Request bodies are capped at [`MAX_REQUEST_BODY`] (code uploads reach the server's 5 MiB code
//! limit, plus JSON escaping), request heads at [`MAX_REQUEST_HEAD`] and must arrive within
//! [`HANDSHAKE_TIMEOUT`], which also bounds connecting to the upstream and its socket handshake.
//! Socket messages from clients are capped at [`MAX_SOCKET_MESSAGE`]. Larger bodies get
//! `413 Payload Too Large`, unreachable upstreams `502 Bad Gateway` (`504` on timeouts).
//!
//! # Logs and secrets
//!
//! The proxy logs through `tracing`, one line per request with the method, the path without the
//! query, the status and the duration (until the response head), and one line per closed socket.
//! It never logs tokens, passwords, queries, bodies or socket frames.
//!
//! **The operator of a proxy sees every token, password and message that passes through it**:
//! the proxy terminates the browser's connection and opens its own to the upstream. Run your own
//! (on loopback, as the Tauri app does, or self-hosted next to the web build); do not sign in
//! through a proxy run by someone else.
//!
//! # Example
//!
//! ```no_run
//! # async fn run() -> Result<(), overseer_proxy::ProxyError> {
//! use overseer_proxy::{Proxy, ProxyConfig};
//!
//! let proxy = Proxy::start(ProxyConfig {
//!     upstream: "https://screeps.com/".parse().expect("valid URL"),
//!     listen: "127.0.0.1:0".parse().expect("valid address"),
//!     static_dir: None,
//!     allowed_origins: vec!["http://tauri.localhost".to_owned()],
//! })
//! .await?;
//! // Point the client at `proxy.url()` instead of the upstream.
//! let server = proxy.url();
//! # drop(server);
//! proxy.shutdown().await;
//! # Ok(())
//! # }
//! ```
//!
//! The `overseer-proxy` binary runs the same proxy from the command line:
//!
//! ```sh
//! overseer-proxy --upstream https://screeps.com/ [--listen 127.0.0.1:8080] [--static <dir>] [--allow-origin <origin>]...
//! ```
//!
//! with the environment fallbacks `OVERSEER_PROXY_UPSTREAM`, `OVERSEER_PROXY_LISTEN`,
//! `OVERSEER_PROXY_STATIC` and `OVERSEER_PROXY_ALLOW_ORIGIN` (comma-separated).

mod cors;
mod forward;
mod serve;
mod socket;

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use url::Url;

/// Largest request body forwarded: 16 MiB. Code uploads carry up to the server's 5 MiB code
/// limit as JSON strings, which escaping (or base64 for wasm modules) makes larger.
pub const MAX_REQUEST_BODY: usize = 16 * 1024 * 1024;

/// Largest request head (request line and headers): 64 KiB.
pub const MAX_REQUEST_HEAD: usize = 64 * 1024;

/// Time allowed for a request head to arrive, and for connecting to the upstream and completing
/// its socket handshake.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

/// Largest socket message accepted from a client: 1 MiB (clients send short commands).
pub const MAX_SOCKET_MESSAGE: usize = 1024 * 1024;

/// What a [`Proxy`] forwards to and how it is reached.
#[derive(Clone, Debug)]
pub struct ProxyConfig {
    /// Upstream base URL (http or https, ends with `/`; a path prefix such as `/season/` is
    /// kept). A base without the final `/` gets one.
    pub upstream: Url,
    /// Listen address; `127.0.0.1:0` picks a free loopback port.
    pub listen: SocketAddr,
    /// Directory served for every path not forwarded (same-origin web mode); `None` serves
    /// nothing else.
    pub static_dir: Option<PathBuf>,
    /// Exact origins (`scheme://host[:port]`) allowed cross-origin; empty = same-origin only.
    pub allowed_origins: Vec<String>,
}

/// Why a [`Proxy`] could not start.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProxyError {
    /// The upstream base is not a plain http or https URL.
    #[error("invalid upstream URL: {0}")]
    Upstream(&'static str),
    /// An allowed origin is not of the form `scheme://host[:port]`.
    #[error("invalid allowed origin `{0}` (expected scheme://host[:port])")]
    Origin(String),
    /// The static directory is missing or not a directory.
    #[error("static directory {}: {source}", path.display())]
    StaticDir {
        /// The configured directory.
        path: PathBuf,
        /// What is wrong with it.
        #[source]
        source: io::Error,
    },
    /// The listen address could not be bound.
    #[error("cannot listen on {address}: {source}")]
    Bind {
        /// The configured address.
        address: SocketAddr,
        /// The bind error.
        #[source]
        source: io::Error,
    },
    /// The upstream HTTP client could not be built.
    #[error("cannot build the upstream HTTP client: {0}")]
    Client(#[source] reqwest::Error),
}

/// A running proxy. Dropping it stops it too, without waiting for its connections to close.
#[derive(Debug)]
pub struct Proxy {
    url: Url,
    upstream: Url,
    stop: CancellationToken,
    tasks: TaskTracker,
}

impl Proxy {
    /// Binds and serves on the current tokio runtime.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError`] when the configuration is invalid or the address cannot be bound.
    pub async fn start(config: ProxyConfig) -> Result<Self, ProxyError> {
        let upstream = upstream_base(config.upstream)?;
        let origins = cors::Origins::new(&config.allowed_origins)?;
        if let Some(path) = &config.static_dir {
            check_static_dir(path)?;
        }
        let listener =
            TcpListener::bind(config.listen)
                .await
                .map_err(|source| ProxyError::Bind {
                    address: config.listen,
                    source,
                })?;
        let address = listener.local_addr().map_err(|source| ProxyError::Bind {
            address: config.listen,
            source,
        })?;
        let url = local_url(address);
        let stop = CancellationToken::new();
        let tasks = TaskTracker::new();
        let state = Arc::new(serve::State::new(
            upstream.clone(),
            origins,
            config.static_dir,
            stop.clone(),
            tasks.clone(),
        )?);
        tasks.spawn(serve::accept(listener, state));
        Ok(Self {
            url,
            upstream,
            stop,
            tasks,
        })
    }

    /// `http://<bound address>/`: use as the `ServerUrl` of the upstream. An unspecified listen
    /// address (`0.0.0.0`, `::`) is reported as loopback.
    #[must_use]
    pub fn url(&self) -> Url {
        self.url.clone()
    }

    /// The upstream base every request is forwarded to.
    #[must_use]
    pub const fn upstream(&self) -> &Url {
        &self.upstream
    }

    /// Stops accepting and closes open connections, sockets included.
    pub async fn shutdown(self) {
        self.stop.cancel();
        self.tasks.close();
        self.tasks.wait().await;
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

/// Validates the upstream base and makes it end with `/`.
fn upstream_base(mut upstream: Url) -> Result<Url, ProxyError> {
    if !matches!(upstream.scheme(), "http" | "https") {
        return Err(ProxyError::Upstream("the scheme must be http or https"));
    }
    if upstream.host_str().is_none() {
        return Err(ProxyError::Upstream("a host is required"));
    }
    if !upstream.username().is_empty() || upstream.password().is_some() {
        return Err(ProxyError::Upstream("credentials are not allowed"));
    }
    if upstream.query().is_some() || upstream.fragment().is_some() {
        return Err(ProxyError::Upstream("a query or fragment is not allowed"));
    }
    if !upstream.path().ends_with('/') {
        let path = format!("{}/", upstream.path());
        upstream.set_path(&path);
    }
    Ok(upstream)
}

fn check_static_dir(path: &std::path::Path) -> Result<(), ProxyError> {
    let metadata = std::fs::metadata(path).map_err(|source| ProxyError::StaticDir {
        path: path.to_owned(),
        source,
    })?;
    if metadata.is_dir() {
        Ok(())
    } else {
        Err(ProxyError::StaticDir {
            path: path.to_owned(),
            source: io::Error::new(io::ErrorKind::NotADirectory, "not a directory"),
        })
    }
}

/// `http://<address>/`, with an unspecified address replaced by loopback.
fn local_url(address: SocketAddr) -> Url {
    let ip = match address.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    let text = format!("http://{}/", SocketAddr::new(ip, address.port()));
    // An IP address and port always form a valid URL.
    Url::parse(&text).unwrap_or_else(|_| unreachable!("`{text}` is a valid URL"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_base_gets_a_final_slash_and_keeps_its_prefix() {
        let base = upstream_base("https://screeps.com/season".parse().unwrap()).unwrap();
        assert_eq!(base.as_str(), "https://screeps.com/season/");
        let base = upstream_base("https://screeps.com".parse().unwrap()).unwrap();
        assert_eq!(base.as_str(), "https://screeps.com/");
    }

    #[test]
    fn upstream_base_rejects_other_schemes_credentials_and_queries() {
        for url in [
            "ftp://screeps.com/",
            "https://user:pass@screeps.com/",
            "https://screeps.com/?a=1",
            "https://screeps.com/#x",
        ] {
            assert!(upstream_base(url.parse().unwrap()).is_err(), "{url}");
        }
    }

    #[test]
    fn local_url_reports_loopback_for_unspecified_addresses() {
        assert_eq!(
            local_url("0.0.0.0:8080".parse().unwrap()).as_str(),
            "http://127.0.0.1:8080/"
        );
        assert_eq!(
            local_url("[::]:8080".parse().unwrap()).as_str(),
            "http://[::1]:8080/"
        );
    }
}
