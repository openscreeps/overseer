//! A fake Screeps server: scripted HTTP routes and an echoing socket, recording what reaches it.

use std::convert::Infallible;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, ready};
use std::time::{Duration, Instant};

use async_tungstenite::WebSocketStream;
use async_tungstenite::tokio::TokioAdapter;
use async_tungstenite::tungstenite::Message;
use async_tungstenite::tungstenite::handshake::derive_accept_key;
use async_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use async_tungstenite::tungstenite::protocol::{CloseFrame, Role};
use futures_util::StreamExt as _;
use http::{Request, Response, StatusCode};
use http_body_util::{BodyExt as _, Full};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use url::Url;

/// The rotated session token every `api/echo` answer carries.
pub const ROTATED_TOKEN: &str = "SECRET-ROTATED-TOKEN";

/// The bytes of every room-history file (served with `Content-Encoding: gzip`).
pub const HISTORY: &[u8] = b"\x1f\x8b\x08\x00fake-gzip-payload";

/// A request as the upstream received it.
#[derive(Clone, Debug)]
pub struct Seen {
    pub method: String,
    /// Path and query.
    pub target: String,
    /// Lowercase header names with their values.
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
}

impl Seen {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

struct Shared {
    prefix: String,
    base: String,
    /// A request arriving on a connection idle for longer than this is dropped (see
    /// [`Upstream::start_closing_idle`]); `None` keeps idle connections open.
    idle: Option<Duration>,
    connections: AtomicUsize,
    /// `<method> <target>` of every dropped request.
    dropped: Mutex<Vec<String>>,
    seen: Mutex<Vec<Seen>>,
    socket: mpsc::UnboundedSender<String>,
}

/// A running fake server; it stops with the test's runtime.
pub struct Upstream {
    /// `http://127.0.0.1:<port><prefix>`.
    pub base: Url,
    shared: Arc<Shared>,
    socket: mpsc::UnboundedReceiver<String>,
}

impl Upstream {
    /// Serves under `prefix` (`/` or `/season/`); other paths are 404. Connections stay open
    /// for further requests.
    pub async fn start(prefix: &str) -> Self {
        Self::launch(prefix, None).await
    }

    /// Like [`Self::start`], but a request arriving on a connection that idled for longer than
    /// `idle` since its last answer is dropped unread, with its connection: what a client
    /// sees when it reuses a connection as Node's keep-alive timeout closes it (the client cannot
    /// have noticed the close yet).
    pub async fn start_closing_idle(prefix: &str, idle: Duration) -> Self {
        Self::launch(prefix, Some(idle)).await
    }

    async fn launch(prefix: &str, idle: Option<Duration>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}{prefix}", listener.local_addr().unwrap());
        let (socket_tx, socket) = mpsc::unbounded_channel();
        let shared = Arc::new(Shared {
            prefix: prefix.to_owned(),
            base: base.clone(),
            idle,
            connections: AtomicUsize::new(0),
            dropped: Mutex::new(Vec::new()),
            seen: Mutex::new(Vec::new()),
            socket: socket_tx,
        });
        let accept = Arc::clone(&shared);
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                accept.connections.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(serve(stream, Arc::clone(&accept)));
            }
        });
        Self {
            base: base.parse().unwrap(),
            shared,
            socket,
        }
    }

    /// Every request received so far.
    pub fn seen(&self) -> Vec<Seen> {
        lock(&self.shared.seen).clone()
    }

    /// The path and query of every request received so far.
    pub fn targets(&self) -> Vec<String> {
        self.seen().into_iter().map(|seen| seen.target).collect()
    }

    /// The TCP connections accepted so far.
    pub fn connections(&self) -> usize {
        self.shared.connections.load(Ordering::SeqCst)
    }

    /// `<method> <target>` of every request dropped on an idle connection so far (not part of
    /// [`Self::seen`]).
    pub fn dropped(&self) -> Vec<String> {
        lock(&self.shared.dropped).clone()
    }

    /// The next socket event: `open`, `text:<text>`, `binary:<bytes>`, `ping:<bytes>`,
    /// `close:<code>:<reason>` or `ended`.
    pub async fn socket_event(&mut self) -> String {
        tokio::time::timeout(crate::PATIENCE, self.socket.recv())
            .await
            .expect("socket event in time")
            .expect("server running")
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Serves one connection with keep-alive and upgrades.
async fn serve(stream: TcpStream, shared: Arc<Shared>) {
    let idle_since = Arc::new(Mutex::new(None));
    let connection = Connection {
        stream,
        shared: Arc::clone(&shared),
        idle_since: Arc::clone(&idle_since),
    };
    let service = service_fn(move |request| {
        let shared = Arc::clone(&shared);
        let idle_since = Arc::clone(&idle_since);
        async move {
            let response = respond(&shared, request).await;
            // An upgraded connection carries the socket from now on.
            if response.status() != StatusCode::SWITCHING_PROTOCOLS {
                *lock(&idle_since) = Some(Instant::now());
            }
            Ok::<_, Infallible>(response)
        }
    });
    let _ = http1::Builder::new()
        .serve_connection(TokioIo::new(connection), service)
        .with_upgrades()
        .await;
}

/// A connection that drops a request arriving after it idled for longer than [`Shared::idle`].
struct Connection {
    stream: TcpStream,
    shared: Arc<Shared>,
    /// When the last answer was made; `None` while none was, while a request is being read and
    /// after an upgrade.
    idle_since: Arc<Mutex<Option<Instant>>>,
}

impl AsyncRead for Connection {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let since = *lock(&this.idle_since);
        if let (Some(idle), Some(since)) = (this.shared.idle, since) {
            let mut head = [0; 512];
            let mut head = ReadBuf::new(&mut head);
            ready!(this.stream.poll_peek(cx, &mut head))?;
            if !head.filled().is_empty() && since.elapsed() > idle {
                let head = String::from_utf8_lossy(head.filled());
                let request = head.split(' ').take(2).collect::<Vec<_>>().join(" ");
                lock(&this.shared.dropped).push(request);
                // hyper gives up the connection and closes it, the request unread.
                return Poll::Ready(Err(io::ErrorKind::ConnectionReset.into()));
            }
        }
        let before = buf.filled().len();
        ready!(Pin::new(&mut this.stream).poll_read(cx, buf))?;
        if buf.filled().len() > before {
            *lock(&this.idle_since) = None;
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Connection {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
}

async fn respond(shared: &Arc<Shared>, request: Request<Incoming>) -> Response<Full<Bytes>> {
    let (parts, body) = request.into_parts();
    let target = parts
        .uri
        .path_and_query()
        .map_or("/", |path| path.as_str())
        .to_owned();
    let headers = parts
        .headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect();
    let upgrade = parts.headers.contains_key(http::header::UPGRADE);
    let body = if upgrade {
        Bytes::new()
    } else {
        body.collect().await.unwrap().to_bytes()
    };
    lock(&shared.seen).push(Seen {
        method: parts.method.to_string(),
        target: target.clone(),
        headers,
        body,
    });
    let Some(route) = parts.uri.path().strip_prefix(shared.prefix.as_str()) else {
        return reply(StatusCode::NOT_FOUND, "wrong prefix");
    };
    if route == "socket/websocket" && upgrade {
        let request = Request::from_parts(parts, ());
        return accept_socket(shared, request);
    }
    if route == "api/echo" {
        let mut response = reply(StatusCode::OK, r#"{"ok":1}"#);
        for (name, value) in [
            ("x-token", ROTATED_TOKEN),
            ("x-ratelimit-limit", "120"),
            ("x-ratelimit-remaining", "119"),
            ("x-ratelimit-reset", "1700000000"),
            ("content-type", "application/json; charset=utf-8"),
            ("set-cookie", "session=upstream"),
            ("server", "fake-upstream"),
            ("x-other", "1"),
            ("access-control-allow-origin", "*"),
        ] {
            response.headers_mut().insert(name, value.parse().unwrap());
        }
        return response;
    }
    if let Some(code) = route.strip_prefix("api/status/") {
        let status = StatusCode::from_u16(code.parse().unwrap()).unwrap();
        let mut response = reply(status, &format!(r#"{{"error":"status {code}"}}"#));
        response
            .headers_mut()
            .insert("x-ratelimit-remaining", "0".parse().unwrap());
        return response;
    }
    if route == "api/redirect" {
        let mut response = reply(StatusCode::FOUND, "");
        let location = format!("{}api/stolen", shared.base);
        response
            .headers_mut()
            .insert(http::header::LOCATION, location.parse().unwrap());
        return response;
    }
    if route == "room-history" || route.starts_with("room-history/") {
        let mut response = Response::new(Full::new(Bytes::from_static(HISTORY)));
        let headers = response.headers_mut();
        headers.insert("content-type", "application/json".parse().unwrap());
        headers.insert("content-encoding", "gzip".parse().unwrap());
        return response;
    }
    reply(StatusCode::NOT_FOUND, "not found upstream")
}

fn reply(status: StatusCode, body: &str) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from(body.to_owned())));
    *response.status_mut() = status;
    response
}

/// Switches protocols and runs the echo socket: sends `o`, echoes text as `echo:<text>` and
/// binary reversed, closes with `4001 bye` on `close-me`.
fn accept_socket(shared: &Arc<Shared>, mut request: Request<()>) -> Response<Full<Bytes>> {
    let key = request.headers()["sec-websocket-key"].as_bytes().to_owned();
    let events = shared.socket.clone();
    let upgrade = hyper::upgrade::on(&mut request);
    tokio::spawn(async move {
        let io = TokioIo::new(upgrade.await.unwrap());
        let mut socket =
            WebSocketStream::from_raw_socket(TokioAdapter::new(io), Role::Server, None).await;
        let _ = events.send("open".to_owned());
        socket.send(Message::text("o")).await.unwrap();
        while let Some(Ok(message)) = socket.next().await {
            match message {
                Message::Text(text) if text.as_str() == "close-me" => {
                    let _ = events.send(format!("text:{}", text.as_str()));
                    let frame = CloseFrame {
                        code: CloseCode::Library(4001),
                        reason: "bye".into(),
                    };
                    let _ = socket.send(Message::Close(Some(frame))).await;
                }
                Message::Text(text) => {
                    let _ = events.send(format!("text:{}", text.as_str()));
                    let echo = format!("echo:{}", text.as_str());
                    let _ = socket.send(Message::text(echo)).await;
                }
                Message::Binary(data) => {
                    let _ = events.send(format!("binary:{:?}", data.as_ref()));
                    let reversed: Vec<u8> = data.iter().rev().copied().collect();
                    let _ = socket.send(Message::binary(reversed)).await;
                }
                Message::Ping(data) => {
                    let _ = events.send(format!("ping:{:?}", data.as_ref()));
                }
                Message::Close(frame) => {
                    let event = frame.map_or_else(
                        || "close:none".to_owned(),
                        |frame| format!("close:{}:{}", u16::from(frame.code), frame.reason),
                    );
                    let _ = events.send(event);
                }
                Message::Pong(_) | Message::Frame(_) => {}
            }
        }
        let _ = events.send("ended".to_owned());
    });
    let mut response = reply(StatusCode::SWITCHING_PROTOCOLS, "");
    let headers = response.headers_mut();
    headers.insert("upgrade", "websocket".parse().unwrap());
    headers.insert("connection", "Upgrade".parse().unwrap());
    headers.insert(
        "sec-websocket-accept",
        derive_accept_key(&key).parse().unwrap(),
    );
    response
}
