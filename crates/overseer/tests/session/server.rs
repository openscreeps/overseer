//! An in-process server: JSON HTTP routes and scripted WebSocket connections.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_tungstenite::WebSocketStream;
use async_tungstenite::tokio::{TokioAdapter, accept_async};
use async_tungstenite::tungstenite::Message;
use futures_util::StreamExt;
use overseer::core::ServerUrl;
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
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = ServerUrl::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let (sender, sockets) = mpsc::unbounded_channel();
        let state = Arc::new(State {
            routes: Box::new(move |request| routes(request).into()),
            upgrades: Box::new(upgrades),
            upgrade_count: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            sockets: sender,
        });
        let accepting = Arc::clone(&state);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve(stream, Arc::clone(&accepting)));
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

    /// The next accepted WebSocket connection.
    pub async fn socket(&mut self) -> Ws {
        self.sockets.recv().await.expect("the server is running")
    }

    /// The HTTP requests served so far.
    pub fn requests(&self) -> Vec<Request> {
        self.state
            .requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

async fn serve(mut stream: TcpStream, state: Arc<State>) {
    let Some(head) = peek_head(&stream).await else {
        return;
    };
    let mut consumed = vec![0; head.len()];
    let mut lines = head.lines();
    let mut request_line = lines.next().unwrap_or_default().split_whitespace();
    let method = request_line.next().unwrap_or_default().to_owned();
    let target = request_line.next().unwrap_or_default();
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let (path, query) = (path.to_owned(), query.to_owned());
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
        return;
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
        return;
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
    state
        .requests
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(request);
    let headers = headers
        .iter()
        .fold(String::new(), |mut lines, (name, value)| {
            let _ = write!(lines, "{name}: {value}\r\n");
            lines
        });
    let head = format!(
        "HTTP/1.1 {status} Reply\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(&[head.into_bytes(), body].concat()).await;
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
