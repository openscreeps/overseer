//! Accepting connections and routing requests.

use std::convert::Infallible;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderValue, Method, Request, Response, StatusCode};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt as _, Empty, Full};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tower::ServiceExt as _;
use tower_http::services::ServeDir;
use tracing::{info, warn};
use url::Url;

use crate::cors::{self, Origin, Origins};
use crate::{HANDSHAKE_TIMEOUT, MAX_REQUEST_HEAD, ProxyError, forward, socket};

/// Response bodies: empty, small local ones, static files and streamed upstream bodies.
pub(crate) type Body = UnsyncBoxBody<Bytes, io::Error>;

/// The forwarded socket path.
pub(crate) const SOCKET_PATH: &str = "/socket/websocket";

/// The upstream discovery document.
const DISCOVERY_PATH: &str = "/overseer-proxy.json";

/// The proxy's `User-Agent` towards the upstream.
const USER_AGENT: &str = concat!("overseer-proxy/", env!("CARGO_PKG_VERSION"));

/// Longest wait for the next piece of an upstream response.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// Shared by every connection of one proxy.
pub(crate) struct State {
    /// The upstream base, ending with `/`.
    pub(crate) upstream: Url,
    origins: Origins,
    /// The upstream client: no redirects, no decompression, no cookies.
    pub(crate) http: reqwest::Client,
    static_files: Option<ServeDir>,
    /// Cancelled when the proxy stops.
    pub(crate) stop: CancellationToken,
    /// Every connection and socket relay of the proxy.
    pub(crate) tasks: TaskTracker,
}

impl State {
    pub(crate) fn new(
        upstream: Url,
        origins: Origins,
        static_dir: Option<PathBuf>,
        stop: CancellationToken,
        tasks: TaskTracker,
    ) -> Result<Self, ProxyError> {
        // Err only means a provider is already installed, which then stays in use.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            // Bodies pass through as the upstream encoded them (gzip room history).
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .user_agent(USER_AGENT)
            .connect_timeout(HANDSHAKE_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .build()
            .map_err(ProxyError::Client)?;
        Ok(Self {
            upstream,
            origins,
            http,
            static_files: static_dir.map(ServeDir::new),
            stop,
            tasks,
        })
    }

    /// The upstream address of a forwarded `path_and_query` (starting with `/`), or `None` when
    /// it would leave the upstream base.
    pub(crate) fn upstream_url(&self, path_and_query: &str) -> Option<Url> {
        let relative = path_and_query.strip_prefix('/')?;
        let url = Url::parse(&format!("{}{relative}", self.upstream)).ok()?;
        url.as_str()
            .starts_with(self.upstream.as_str())
            .then_some(url)
    }
}

/// Accepts connections until the proxy stops.
pub(crate) async fn accept(listener: TcpListener, state: Arc<State>) {
    loop {
        let accepted = tokio::select! {
            () = state.stop.cancelled() => return,
            accepted = listener.accept() => accepted,
        };
        match accepted {
            Ok((stream, _)) => {
                state.tasks.spawn(connection(stream, Arc::clone(&state)));
            }
            Err(error) => {
                // Out of file descriptors and the like: back off instead of spinning.
                warn!(%error, "accepting a connection failed");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

/// Serves one HTTP/1.1 connection until it closes or the proxy stops.
async fn connection(stream: TcpStream, state: Arc<State>) {
    // Latency matters more than packet count for socket frames and small JSON replies.
    let _ = stream.set_nodelay(true);
    let stop = state.stop.clone();
    let service = service_fn(move |request| {
        let state = Arc::clone(&state);
        async move { Ok::<_, Infallible>(handle(&state, request).await) }
    });
    let mut builder = http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(HANDSHAKE_TIMEOUT)
        .max_header_size(MAX_REQUEST_HEAD);
    let connection = builder
        .serve_connection(TokioIo::new(stream), service)
        .with_upgrades();
    tokio::select! {
        // A failed connection (malformed request, timeout, reset) needs no handling.
        _ = connection => {}
        () = stop.cancelled() => {}
    }
}

/// Answers one request and logs its method, path, status and duration.
async fn handle(state: &Arc<State>, request: Request<Incoming>) -> Response<Body> {
    let started = Instant::now();
    let method = request.method().clone();
    // The path only: queries may carry user data.
    let path = request.uri().path().to_owned();
    let origin = state.origins.classify(request.headers());
    let mut response = route(state, request, &origin).await;
    cors::decorate(&mut response, &origin, !state.origins.is_empty());
    info!(
        method = %method,
        path = %path,
        status = response.status().as_u16(),
        duration_ms = millis(started),
        "request"
    );
    response
}

/// Milliseconds since `started`.
pub(crate) fn millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// What a path is.
#[derive(Debug, PartialEq, Eq)]
enum Route {
    /// `api/*` or `room-history/*`.
    Forward,
    /// `socket/websocket`.
    Socket,
    /// `overseer-proxy.json`.
    Discovery,
    /// A forwarded prefix with a dot segment, which would leave the forwarded paths.
    Rejected,
    /// Anything else: the static directory.
    Static,
}

impl Route {
    fn of(path: &str) -> Self {
        if path == SOCKET_PATH {
            Self::Socket
        } else if path == DISCOVERY_PATH {
            Self::Discovery
        } else if path.starts_with("/api/") || path.starts_with("/room-history/") {
            if path.contains('\\') || path.split('/').any(is_dot_segment) {
                Self::Rejected
            } else {
                Self::Forward
            }
        } else {
            Self::Static
        }
    }
}

/// `.` or `..`, also percent-encoded (URL parsers resolve `%2e` like `.`).
fn is_dot_segment(segment: &str) -> bool {
    let mut rest = segment;
    let mut dots = 0;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('.') {
            rest = after;
        } else if rest
            .get(..3)
            .is_some_and(|escape| escape.eq_ignore_ascii_case("%2e"))
        {
            rest = &rest[3..];
        } else {
            return false;
        }
        dots += 1;
    }
    matches!(dots, 1 | 2)
}

async fn route(state: &Arc<State>, request: Request<Incoming>, origin: &Origin) -> Response<Body> {
    if request.method() == Method::OPTIONS {
        return preflight(request.headers(), origin);
    }
    match Route::of(request.uri().path()) {
        Route::Forward => forward::forward(state, request).await,
        Route::Socket => socket::upgrade(state, request, origin).await,
        Route::Discovery => discovery(state, request.method()),
        Route::Rejected => status(StatusCode::NOT_FOUND),
        Route::Static => serve_static(state, request).await,
    }
}

/// Answers a preflight locally: allowed origins get the CORS allowances, other origins are
/// refused, and requests without a foreign origin get an empty answer.
fn preflight(headers: &http::HeaderMap, origin: &Origin) -> Response<Body> {
    match origin {
        Origin::Denied => status(StatusCode::FORBIDDEN),
        Origin::Allowed(_) => {
            let mut response = status(StatusCode::NO_CONTENT);
            cors::allow_preflight(&mut response, headers);
            response
        }
        Origin::Absent | Origin::Same => status(StatusCode::NO_CONTENT),
    }
}

/// `{"upstream": "<upstream base>"}`.
fn discovery(state: &State, method: &Method) -> Response<Body> {
    if method != Method::GET && method != Method::HEAD {
        return status(StatusCode::METHOD_NOT_ALLOWED);
    }
    let document = serde_json::json!({ "upstream": state.upstream.as_str() }).to_string();
    let mut response = Response::new(full(document.into()));
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn serve_static(state: &State, request: Request<Incoming>) -> Response<Body> {
    let Some(files) = &state.static_files else {
        return status(StatusCode::NOT_FOUND);
    };
    match files.clone().oneshot(request).await {
        Ok(response) => response.map(http_body_util::BodyExt::boxed_unsync),
        Err(never) => match never {},
    }
}

/// An empty response with `status`.
pub(crate) fn status(status: StatusCode) -> Response<Body> {
    let mut response = Response::new(Empty::new().map_err(|never| match never {}).boxed_unsync());
    *response.status_mut() = status;
    response
}

/// A body of `bytes`.
fn full(bytes: Bytes) -> Body {
    Full::new(bytes)
        .map_err(|never| match never {})
        .boxed_unsync()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_follow_the_forwarded_prefixes() {
        assert_eq!(Route::of("/api/auth/me"), Route::Forward);
        assert_eq!(
            Route::of("/room-history/shard0/W1N1/100.json"),
            Route::Forward
        );
        assert_eq!(Route::of("/socket/websocket"), Route::Socket);
        assert_eq!(Route::of("/overseer-proxy.json"), Route::Discovery);
        assert_eq!(Route::of("/api"), Route::Static);
        assert_eq!(Route::of("/socket/info"), Route::Static);
        assert_eq!(Route::of("/"), Route::Static);
        for path in [
            "/api/../x",
            "/api/%2e%2E/x",
            "/api/.%2e/x",
            "/api/./x",
            "/room-history/..",
            "/api/a\\..\\b",
        ] {
            assert_eq!(Route::of(path), Route::Rejected, "{path}");
        }
        assert_eq!(Route::of("/api/...x/.hidden"), Route::Forward);
    }

    #[test]
    fn upstream_urls_keep_the_prefix_and_query() {
        let state = State::new(
            "https://screeps.com/season/".parse().unwrap(),
            Origins::new(&[]).unwrap(),
            None,
            CancellationToken::new(),
            TaskTracker::new(),
        )
        .unwrap();
        assert_eq!(
            state
                .upstream_url("/api/user/memory?path=a.b")
                .unwrap()
                .as_str(),
            "https://screeps.com/season/api/user/memory?path=a.b"
        );
        assert!(state.upstream_url("/api/../../x").is_none());
    }
}
