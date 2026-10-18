//! An in-process server: JSON HTTP routes and scripted WebSocket connections.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_tungstenite::WebSocketStream;
use async_tungstenite::tokio::{TokioAdapter, accept_async};
use async_tungstenite::tungstenite::Message;
use futures_util::StreamExt;
use overseer_core::ServerUrl;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

/// A server-side WebSocket connection.
pub type Ws = WebSocketStream<TokioAdapter<TcpStream>>;

/// A recorded HTTP request.
#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    /// The path, without the query string.
    pub path: String,
    /// The query string, without `?`; empty when absent.
    pub query: String,
    /// The `X-Token` header.
    pub token: Option<String>,
    pub body: String,
}

/// An HTTP reply with a JSON body (possibly content-encoded, see `headers`).
pub struct Response {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

impl From<(u16, String)> for Response {
    fn from((status, body): (u16, String)) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into_bytes(),
        }
    }
}

type Routes = dyn Fn(&Request) -> Response + Send + Sync;
type Upgrades = dyn Fn(usize) -> bool + Send + Sync;

struct State {
    routes: Box<Routes>,
    /// Whether the upgrade with this index (from 0) is accepted; others get HTTP 503.
    upgrades: Box<Upgrades>,
    upgrade_count: AtomicUsize,
    /// Keep-alive: connections serve further requests, and a request arriving on a connection
    /// idle for longer than this is dropped (see [`Server::start_keep_alive`]). `None` closes
    /// every connection after one request.
    keep_alive: Option<Duration>,
    connections: AtomicUsize,
    /// Paths of the requests dropped on idle keep-alive connections.
    dropped: Mutex<Vec<String>>,
    requests: Mutex<Vec<Request>>,
    sockets: mpsc::UnboundedSender<Ws>,
}

/// A running server; it stops with the test's runtime.
pub struct Server {
    url: ServerUrl,
    state: Arc<State>,
    sockets: mpsc::UnboundedReceiver<Ws>,
}

impl Server {
    /// Serves `routes` over HTTP (one request per connection) and hands accepted
    /// `/socket/websocket` upgrades to [`Self::socket`].
    pub async fn start<R: Into<Response>>(
        routes: impl Fn(&Request) -> R + Send + Sync + 'static,
        upgrades: impl Fn(usize) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self::launch(routes, upgrades, None, None).await
    }

    /// Like [`Self::start`], but keeps connections open for further requests, like Node's
    /// keep-alive. A request arriving on a connection that idled for longer than `idle` is
    /// dropped unanswered with its connection, as when Node's keep-alive timeout closes the
    /// connection while the client reuses it: the client cannot have noticed the close yet.
    pub async fn start_keep_alive<R: Into<Response>>(
        routes: impl Fn(&Request) -> R + Send + Sync + 'static,
        upgrades: impl Fn(usize) -> bool + Send + Sync + 'static,
        idle: Duration,
    ) -> Self {
        Self::launch(routes, upgrades, Some(idle), None).await
    }

    /// Like [`Self::start`], but accepts only the first `connections` connections. Then it
    /// stops listening, so that later connection attempts fail.
    pub async fn start_refusing_after<R: Into<Response>>(
        routes: impl Fn(&Request) -> R + Send + Sync + 'static,
        connections: usize,
    ) -> Self {
        Self::launch(routes, |_| true, None, Some(connections)).await
    }

    async fn launch<R: Into<Response>>(
        routes: impl Fn(&Request) -> R + Send + Sync + 'static,
        upgrades: impl Fn(usize) -> bool + Send + Sync + 'static,
        keep_alive: Option<Duration>,
        connections: Option<usize>,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = ServerUrl::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let (sender, sockets) = mpsc::unbounded_channel();
        let state = Arc::new(State {
            routes: Box::new(move |request| routes(request).into()),
            upgrades: Box::new(upgrades),
            upgrade_count: AtomicUsize::new(0),
            keep_alive,
            connections: AtomicUsize::new(0),
            dropped: Mutex::new(Vec::new()),
            requests: Mutex::new(Vec::new()),
            sockets: sender,
        });
        let accepting = Arc::clone(&state);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let accepted = accepting.connections.fetch_add(1, Ordering::SeqCst) + 1;
                tokio::spawn(serve(stream, Arc::clone(&accepting)));
                if connections.is_some_and(|limit| accepted >= limit) {
                    // Drops the listener.
                    break;
                }
            }
        });
        Self {
            url,
            state,
            sockets,
        }
    }

    pub fn url(&self) -> ServerUrl {
        self.url.clone()
    }

    /// The server as a stand-in for screeps.com ([`ServerUrl::proxied`] in front of it):
    /// channels, history and requests name shards.
    pub fn official_url(&self) -> ServerUrl {
        ServerUrl::proxied(self.url.base().as_str(), &ServerUrl::official()).unwrap()
    }

    /// The next accepted WebSocket connection.
    pub async fn socket(&mut self) -> Ws {
        self.sockets.recv().await.expect("the server is running")
    }

    /// The HTTP requests served so far.
    pub fn requests(&self) -> Vec<Request> {
        lock(&self.state.requests).clone()
    }

    /// The `/socket/websocket` requests answered so far (accepted or rejected), not counting
    /// dropped ones.
    pub fn upgrades(&self) -> usize {
        self.state.upgrade_count.load(Ordering::SeqCst)
    }

    /// The TCP connections accepted so far.
    pub fn connections(&self) -> usize {
        self.state.connections.load(Ordering::SeqCst)
    }

    /// The paths of the requests dropped on idle keep-alive connections so far.
    pub fn dropped(&self) -> Vec<String> {
        lock(&self.state.dropped).clone()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

async fn serve(mut stream: TcpStream, state: Arc<State>) {
    let mut idle_since = None;
    while let Some(open) = serve_one(stream, &state, &mut idle_since).await {
        stream = open;
    }
}

/// Serves one request; returns the connection when it stays open for another one.
async fn serve_one(
    mut stream: TcpStream,
    state: &State,
    idle_since: &mut Option<Instant>,
) -> Option<TcpStream> {
    let head = peek_head(&stream).await?;
    let mut consumed = vec![0; head.len()];
    let mut lines = head.lines();
    let mut request_line = lines.next().unwrap_or_default().split_whitespace();
    let method = request_line.next().unwrap_or_default().to_owned();
    let target = request_line.next().unwrap_or_default();
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let (path, query) = (path.to_owned(), query.to_owned());
    if let (Some(idle), Some(since)) = (state.keep_alive, *idle_since)
        && since.elapsed() > idle
    {
        // Closed with the request unread and unanswered.
        lock(&state.dropped).push(path);
        return None;
    }
    if path == "/socket/websocket" {
        let index = state.upgrade_count.fetch_add(1, Ordering::SeqCst);
        if (state.upgrades)(index) {
            if let Ok(websocket) = accept_async(stream).await {
                let _ = state.sockets.send(websocket);
            }
        } else if stream.read_exact(&mut consumed).await.is_ok() {
            let _ = stream
                .write_all(
                    b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
        }
        return None;
    }
    let mut token = None;
    let mut length = 0;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let value = value.trim();
            if name.eq_ignore_ascii_case("x-token") {
                token = Some(value.to_owned());
            } else if name.eq_ignore_ascii_case("content-length") {
                length = value.parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0; length];
    if stream.read_exact(&mut consumed).await.is_err()
        || stream.read_exact(&mut body).await.is_err()
    {
        return None;
    }
    let request = Request {
        method,
        path,
        query,
        token,
        body: String::from_utf8_lossy(&body).into_owned(),
    };
    let Response {
        status,
        headers,
        body,
    } = (state.routes)(&request);
    lock(&state.requests).push(request);
    let headers = headers
        .iter()
        .fold(String::new(), |mut lines, (name, value)| {
            let _ = write!(lines, "{name}: {value}\r\n");
            lines
        });
    let connection = if state.keep_alive.is_some() {
        "keep-alive"
    } else {
        "close"
    };
    let head = format!(
        "HTTP/1.1 {status} Reply\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{headers}Connection: {connection}\r\n\r\n",
        body.len()
    );
    stream
        .write_all(&[head.into_bytes(), body].concat())
        .await
        .ok()?;
    *idle_since = Some(Instant::now());
    state.keep_alive.map(|_| stream)
}

/// The request head up to and including the blank line, left in the stream.
async fn peek_head(stream: &TcpStream) -> Option<String> {
    let mut buffer = vec![0; 16 * 1024];
    loop {
        let read = stream.peek(&mut buffer).await.ok()?;
        if read == 0 {
            return None;
        }
        if let Some(end) = buffer[..read]
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
        {
            return Some(String::from_utf8_lossy(&buffer[..end + 4]).into_owned());
        }
        assert!(read < buffer.len(), "request head too large");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

/// Receives the next `count` text frames, skipping control frames.
pub async fn receive(websocket: &mut Ws, count: usize) -> Vec<String> {
    let mut lines = Vec::with_capacity(count);
    while lines.len() < count {
        match websocket
            .next()
            .await
            .expect("the client is connected")
            .unwrap()
        {
            Message::Text(text) => lines.push(text.as_str().to_owned()),
            Message::Ping(_) | Message::Pong(_) => {}
            other => panic!("unexpected frame {other:?}"),
        }
    }
    lines
}

/// Sends one text frame.
pub async fn send(websocket: &mut Ws, text: &str) {
    websocket.send(Message::text(text)).await.unwrap();
}
