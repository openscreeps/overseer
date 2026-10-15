//! The proxy against an in-process fake upstream: forwarding, header allow-lists, statuses,
//! redirects, static files, limits, CORS and the socket relay (the logs are checked in
//! `tests/logs`, alone in their process).

mod upstream;

use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_tungstenite::WebSocketStream;
use async_tungstenite::tokio::{TokioAdapter, client_async};
use async_tungstenite::tungstenite::client::IntoClientRequest as _;
use async_tungstenite::tungstenite::protocol::CloseFrame;
use async_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use async_tungstenite::tungstenite::{Error as WsError, Message};
use futures_util::StreamExt as _;
use http::{HeaderValue, StatusCode};
use overseer_proxy::{MAX_REQUEST_BODY, Proxy, ProxyConfig};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use upstream::{HISTORY, ROTATED_TOKEN, Upstream};
use url::Url;

/// Upper bound of every wait in these tests.
const PATIENCE: Duration = Duration::from_secs(10);

const ALLOWED: &str = "http://allowed.example";
const DENIED: &str = "http://denied.example";

/// A client socket.
type Socket = WebSocketStream<TokioAdapter<TcpStream>>;

async fn proxy(upstream: &Url, static_dir: Option<PathBuf>, origins: &[&str]) -> Proxy {
    Proxy::start(ProxyConfig {
        upstream: upstream.clone(),
        listen: "127.0.0.1:0".parse().unwrap(),
        static_dir,
        allowed_origins: origins.iter().map(|&origin| origin.to_owned()).collect(),
    })
    .await
    .unwrap()
}

/// A client that neither follows redirects nor decodes bodies.
fn client() -> reqwest::Client {
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .no_proxy()
        .build()
        .unwrap()
}

fn address(proxy: &Proxy) -> SocketAddr {
    proxy
        .url()
        .socket_addrs(|| None)
        .unwrap()
        .into_iter()
        .next()
        .unwrap()
}

/// Sends `request` (a raw HTTP/1.1 request) and returns the response status.
async fn raw_status(proxy: &Proxy, request: &str) -> u16 {
    let mut stream = TcpStream::connect(address(proxy)).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    read_status(&mut stream).await
}

/// Posts a chunked body of `length` bytes to `api/echo` while reading the response status
/// (the proxy may answer before the body is complete).
async fn chunked_status(proxy: &Proxy, length: usize) -> u16 {
    let stream = TcpStream::connect(address(proxy)).await.unwrap();
    let (mut reader, mut writer) = stream.into_split();
    let body = tokio::spawn(async move {
        let head = "POST /api/echo HTTP/1.1\r\nHost: proxy\r\nTransfer-Encoding: chunked\r\n\r\n";
        writer.write_all(head.as_bytes()).await?;
        let chunk = vec![b'x'; 1024 * 1024];
        let mut left = length;
        while left > 0 {
            let size = left.min(chunk.len());
            writer.write_all(format!("{size:x}\r\n").as_bytes()).await?;
            writer.write_all(&chunk[..size]).await?;
            writer.write_all(b"\r\n").await?;
            left -= size;
        }
        writer.write_all(b"0\r\n\r\n").await
    });
    let status = read_status(&mut reader).await;
    body.abort();
    status
}

/// Reads a response status line and returns the status.
async fn read_status(reader: &mut (impl AsyncRead + Unpin)) -> u16 {
    let mut head = Vec::new();
    let mut byte = [0; 1];
    while !head.ends_with(b"\r\n") {
        tokio::time::timeout(PATIENCE, reader.read_exact(&mut byte))
            .await
            .unwrap()
            .unwrap();
        head.push(byte[0]);
    }
    let line = String::from_utf8(head).unwrap();
    line.split(' ').nth(1).unwrap().parse().unwrap()
}

/// Opens the proxied socket, sending `origin` when given.
async fn open_socket(proxy: &Proxy, origin: Option<&str>) -> Result<Socket, WsError> {
    let mut request = proxy
        .url()
        .join("socket/websocket")
        .unwrap()
        .as_str()
        .replacen("http", "ws", 1)
        .into_client_request()
        .unwrap();
    if let Some(origin) = origin {
        request
            .headers_mut()
            .insert("origin", HeaderValue::from_str(origin).unwrap());
    }
    let stream = TcpStream::connect(address(proxy)).await.unwrap();
    tokio::time::timeout(PATIENCE, client_async(request, stream))
        .await
        .unwrap()
        .map(|(socket, _)| socket)
}

/// The next data or close frame from `socket`.
async fn next(socket: &mut Socket) -> Message {
    loop {
        let message = tokio::time::timeout(PATIENCE, socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if !matches!(message, Message::Ping(_) | Message::Pong(_)) {
            return message;
        }
    }
}

fn header<'a>(response: &'a reqwest::Response, name: &str) -> Option<&'a str> {
    response
        .headers()
        .get(name)
        .map(|value| value.to_str().unwrap())
}

#[tokio::test]
async fn api_requests_forward_only_allowed_headers_both_ways() {
    let upstream = Upstream::start("/").await;
    let proxy = proxy(&upstream.base, None, &[]).await;
    let response = client()
        .get(proxy.url().join("api/echo?shard=shard0&room=W1N1").unwrap())
        .header("X-Token", "token-1")
        .header("X-Username", "user-1")
        .header("X-Server-Password", "server-pass")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("Cookie", "session=browser")
        .header("Authorization", "Bearer nope")
        .header("User-Agent", "browser")
        .header("Referer", "http://page.example/")
        .header("X-Forwarded-For", "10.0.0.1")
        .header("X-Custom", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header(&response, "x-token"), Some(ROTATED_TOKEN));
    assert_eq!(header(&response, "x-ratelimit-limit"), Some("120"));
    assert_eq!(header(&response, "x-ratelimit-remaining"), Some("119"));
    assert_eq!(header(&response, "x-ratelimit-reset"), Some("1700000000"));
    assert_eq!(
        header(&response, "content-type"),
        Some("application/json; charset=utf-8")
    );
    for dropped in [
        "set-cookie",
        "server",
        "x-other",
        "access-control-allow-origin",
    ] {
        assert_eq!(header(&response, dropped), None, "{dropped} passed through");
    }
    assert_eq!(response.text().await.unwrap(), r#"{"ok":1}"#);

    let seen = upstream.seen();
    assert_eq!(seen.len(), 1);
    let request = &seen[0];
    assert_eq!(request.method, "GET");
    assert_eq!(request.target, "/api/echo?shard=shard0&room=W1N1");
    assert_eq!(request.header("x-token"), Some("token-1"));
    assert_eq!(request.header("x-username"), Some("user-1"));
    assert_eq!(request.header("x-server-password"), Some("server-pass"));
    assert_eq!(request.header("content-type"), Some("application/json"));
    assert_eq!(request.header("accept"), Some("application/json"));
    assert_eq!(
        request.header("user-agent"),
        Some(concat!("overseer-proxy/", env!("CARGO_PKG_VERSION")))
    );
    let mut names: Vec<_> = request
        .headers
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "accept",
            "content-type",
            "host",
            "user-agent",
            "x-server-password",
            "x-token",
            "x-username"
        ]
    );
    proxy.shutdown().await;
}

#[tokio::test]
async fn methods_queries_and_bodies_pass_unchanged() {
    let upstream = Upstream::start("/").await;
    let proxy = proxy(&upstream.base, None, &[]).await;
    let body = r#"{"branch":"default","modules":{"main":"module.exports.loop = () => {};"}}"#;
    let response = client()
        .post(proxy.url().join("api/echo?a=1&b=%20x").unwrap())
        .header("Content-Type", "application/json; charset=utf-8")
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let deleted = client()
        .delete(proxy.url().join("api/echo").unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    let seen = upstream.seen();
    assert_eq!(seen[0].method, "POST");
    assert_eq!(seen[0].target, "/api/echo?a=1&b=%20x");
    assert_eq!(seen[0].body, body.as_bytes());
    assert_eq!(
        seen[0].header("content-length"),
        Some(&*body.len().to_string())
    );
    assert_eq!(seen[1].method, "DELETE");
    proxy.shutdown().await;
}

#[tokio::test]
async fn a_path_prefix_of_the_upstream_is_kept() {
    let mut upstream = Upstream::start("/season/").await;
    let proxy = proxy(&upstream.base, None, &[]).await;
    assert_eq!(proxy.upstream().as_str(), upstream.base.as_str());
    let response = client()
        .get(proxy.url().join("api/echo").unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let history = client()
        .get(
            proxy
                .url()
                .join("room-history/shard0/W1N1/100.json")
                .unwrap(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(history.status(), StatusCode::OK);
    let mut socket = open_socket(&proxy, None).await.unwrap();
    assert_eq!(upstream.socket_event().await, "open");
    assert_eq!(next(&mut socket).await, Message::text("o"));
    assert_eq!(
        upstream.targets(),
        [
            "/season/api/echo",
            "/season/room-history/shard0/W1N1/100.json",
            "/season/socket/websocket"
        ]
    );
    proxy.shutdown().await;
}

#[tokio::test]
async fn single_world_history_by_query_is_forwarded() {
    let upstream = Upstream::start("/").await;
    let proxy = proxy(&upstream.base, None, &[]).await;
    let history = client()
        .get(proxy.url().join("room-history?room=W1N1&time=100").unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(history.status(), StatusCode::OK);
    let lookalike = client()
        .get(proxy.url().join("room-historyx?room=W1N1").unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(lookalike.status(), StatusCode::NOT_FOUND);
    assert_eq!(upstream.targets(), ["/room-history?room=W1N1&time=100"]);
    proxy.shutdown().await;
}

#[tokio::test]
async fn statuses_and_error_bodies_pass_through() {
    let upstream = Upstream::start("/").await;
    let proxy = proxy(&upstream.base, None, &[]).await;
    for code in [200_u16, 400, 401, 403, 404, 429, 500, 503] {
        let response = client()
            .get(proxy.url().join(&format!("api/status/{code}")).unwrap())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), code);
        assert_eq!(header(&response, "x-ratelimit-remaining"), Some("0"));
        assert_eq!(
            response.text().await.unwrap(),
            format!(r#"{{"error":"status {code}"}}"#)
        );
    }
    proxy.shutdown().await;
}

#[tokio::test]
async fn redirects_are_passed_without_their_target_and_not_followed() {
    let upstream = Upstream::start("/").await;
    let proxy = proxy(&upstream.base, None, &[]).await;
    let response = client()
        .get(proxy.url().join("api/redirect").unwrap())
        .header("X-Token", "token-1")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(header(&response, "location"), None);
    assert_eq!(upstream.targets(), ["/api/redirect"]);
    proxy.shutdown().await;
}

#[tokio::test]
async fn room_history_keeps_its_content_encoding() {
    let upstream = Upstream::start("/").await;
    let proxy = proxy(&upstream.base, None, &[]).await;
    let response = client()
        .get(
            proxy
                .url()
                .join("room-history/shard0/W1N1/100.json")
                .unwrap(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header(&response, "content-encoding"), Some("gzip"));
    assert_eq!(header(&response, "content-type"), Some("application/json"));
    assert_eq!(response.bytes().await.unwrap(), HISTORY);
    proxy.shutdown().await;
}

#[tokio::test]
async fn other_paths_are_not_found_and_never_forwarded() {
    let upstream = Upstream::start("/").await;
    let proxy = proxy(&upstream.base, None, &[]).await;
    for path in ["", "index.html", "api", "socket/info", "other/api/echo"] {
        let response = client()
            .get(proxy.url().join(path).unwrap())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
    // Dot segments would leave the forwarded paths once the upstream URL is resolved.
    for path in [
        "/api/../x",
        "/api/%2e%2e/%2E%2e/x",
        "/room-history/./../api/echo",
    ] {
        let request = format!("GET {path} HTTP/1.1\r\nHost: proxy\r\nConnection: close\r\n\r\n");
        assert_eq!(raw_status(&proxy, &request).await, 404, "{path}");
    }
    // A request in absolute form still goes to the upstream base only.
    let absolute = "GET http://other.example/api/echo HTTP/1.1\r\nHost: other.example\r\nConnection: close\r\n\r\n";
    assert_eq!(raw_status(&proxy, absolute).await, 200);
    assert_eq!(upstream.targets(), ["/api/echo"]);
    proxy.shutdown().await;
}

/// A fresh directory under the test target directory, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("overseer-proxy-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn write(&self, name: &str, contents: &str) {
        std::fs::File::create(self.0.join(name))
            .unwrap()
            .write_all(contents.as_bytes())
            .unwrap();
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn static_files_and_discovery_are_served_locally() {
    let upstream = Upstream::start("/season/").await;
    let site = TempDir::new("static");
    site.write("index.html", "<!doctype html><title>app</title>");
    site.write("app.js", "export {};");
    let proxy = proxy(&upstream.base, Some(site.0.clone()), &[]).await;
    let index = client().get(proxy.url()).send().await.unwrap();
    assert_eq!(index.status(), StatusCode::OK);
    assert_eq!(header(&index, "content-type"), Some("text/html"));
    assert_eq!(
        index.text().await.unwrap(),
        "<!doctype html><title>app</title>"
    );
    let script = client()
        .get(proxy.url().join("app.js").unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(script.text().await.unwrap(), "export {};");
    let missing = client()
        .get(proxy.url().join("missing.js").unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let discovery = client()
        .get(proxy.url().join("overseer-proxy.json").unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(discovery.status(), StatusCode::OK);
    assert_eq!(header(&discovery, "content-type"), Some("application/json"));
    let document: serde_json::Value =
        serde_json::from_str(&discovery.text().await.unwrap()).unwrap();
    assert_eq!(
        document,
        serde_json::json!({ "upstream": upstream.base.as_str() })
    );
    // Forwarded paths win over files of the same name.
    let api = client()
        .get(proxy.url().join("api/echo").unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(header(&api, "x-token"), Some(ROTATED_TOKEN));
    assert_eq!(upstream.targets(), ["/season/api/echo"]);
    proxy.shutdown().await;
}

#[tokio::test]
async fn request_bodies_are_bounded() {
    let upstream = Upstream::start("/").await;
    let proxy = proxy(&upstream.base, None, &[]).await;
    // A code upload at the server's 5 MiB limit, JSON-escaped, passes.
    let upload = "x".repeat(6 * 1024 * 1024);
    let response = client()
        .post(proxy.url().join("api/echo").unwrap())
        .body(upload.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(upstream.seen()[0].body.len(), upload.len());
    // One byte more than the limit is refused, whether its length is declared or not.
    let declared = format!(
        "POST /api/echo HTTP/1.1\r\nHost: proxy\r\nContent-Length: {}\r\n\r\n",
        MAX_REQUEST_BODY + 1
    );
    assert_eq!(raw_status(&proxy, &declared).await, 413);
    assert_eq!(chunked_status(&proxy, MAX_REQUEST_BODY + 1).await, 413);
    assert_eq!(upstream.seen().len(), 1);
    proxy.shutdown().await;
}

#[tokio::test]
async fn request_heads_are_bounded() {
    let upstream = Upstream::start("/").await;
    let proxy = proxy(&upstream.base, None, &[]).await;
    let huge = "a".repeat(overseer_proxy::MAX_REQUEST_HEAD);
    let request = format!("GET /api/echo HTTP/1.1\r\nHost: proxy\r\nX-Token: {huge}\r\n\r\n");
    assert_eq!(raw_status(&proxy, &request).await, 431);
    assert_eq!(upstream.targets(), Vec::<String>::new());
    proxy.shutdown().await;
}

#[tokio::test]
async fn cors_headers_are_sent_to_allowed_origins_only() {
    let upstream = Upstream::start("/").await;
    let proxy = proxy(&upstream.base, None, &[ALLOWED]).await;
    let url = proxy.url().join("api/echo").unwrap();
    let preflight = |origin: &'static str| {
        client()
            .request(reqwest::Method::OPTIONS, url.clone())
            .header("Origin", origin)
            .header("Access-Control-Request-Method", "POST")
            .header("Access-Control-Request-Headers", "x-token, content-type")
            .send()
    };
    let allowed = preflight(ALLOWED).await.unwrap();
    assert_eq!(allowed.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        header(&allowed, "access-control-allow-origin"),
        Some(ALLOWED)
    );
    assert_eq!(
        header(&allowed, "access-control-allow-headers"),
        Some("X-Token, X-Username, X-Server-Password, Content-Type")
    );
    assert!(
        header(&allowed, "access-control-allow-methods")
            .unwrap()
            .contains("POST")
    );
    assert_eq!(header(&allowed, "vary"), Some("Origin"));
    let denied = preflight(DENIED).await.unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert!(
        denied
            .headers()
            .keys()
            .all(|name| !name.as_str().starts_with("access-control-")),
        "{:?}",
        denied.headers()
    );

    let get = |origin: &'static str| {
        client()
            .get(url.clone())
            .header("Origin", origin)
            .header("X-Token", "token-1")
            .send()
    };
    let allowed = get(ALLOWED).await.unwrap();
    assert_eq!(
        header(&allowed, "access-control-allow-origin"),
        Some(ALLOWED)
    );
    assert_eq!(
        header(&allowed, "access-control-expose-headers"),
        Some("X-Token, X-RateLimit-Limit, X-RateLimit-Remaining, X-RateLimit-Reset")
    );
    assert_eq!(header(&allowed, "x-token"), Some(ROTATED_TOKEN));
    let denied = get(DENIED).await.unwrap();
    assert!(
        denied
            .headers()
            .keys()
            .all(|name| !name.as_str().starts_with("access-control-")),
        "{:?}",
        denied.headers()
    );
    // Preflights never reach the upstream.
    assert_eq!(upstream.targets(), ["/api/echo", "/api/echo"]);
    proxy.shutdown().await;
}

#[tokio::test]
async fn private_network_preflights_are_allowed_for_allowed_origins() {
    let upstream = Upstream::start("/").await;
    let proxy = proxy(&upstream.base, None, &[ALLOWED]).await;
    let response = client()
        .request(
            reqwest::Method::OPTIONS,
            proxy.url().join("api/echo").unwrap(),
        )
        .header("Origin", ALLOWED)
        .header("Access-Control-Request-Method", "GET")
        .header("Access-Control-Request-Private-Network", "true")
        .send()
        .await
        .unwrap();
    assert_eq!(
        header(&response, "access-control-allow-private-network"),
        Some("true")
    );
    proxy.shutdown().await;
}

#[tokio::test]
async fn socket_upgrades_from_foreign_origins_are_refused() {
    let mut upstream = Upstream::start("/").await;
    let proxy = proxy(&upstream.base, None, &[ALLOWED]).await;
    match open_socket(&proxy, Some(DENIED)).await {
        Err(WsError::Http(response)) => assert_eq!(response.status(), StatusCode::FORBIDDEN),
        other => panic!("expected 403, got {other:?}"),
    }
    assert_eq!(upstream.targets(), Vec::<String>::new());
    let own_origin = proxy.url().as_str().trim_end_matches('/').to_owned();
    let mut sockets = Vec::new();
    for origin in [None, Some(ALLOWED), Some(own_origin.as_str())] {
        let mut socket = open_socket(&proxy, origin).await.unwrap();
        assert_eq!(upstream.socket_event().await, "open", "{origin:?}");
        assert_eq!(next(&mut socket).await, Message::text("o"));
        sockets.push(socket);
    }
    proxy.shutdown().await;
}

#[tokio::test]
async fn a_vanished_client_closes_the_upstream_socket() {
    let mut upstream = Upstream::start("/").await;
    let proxy = proxy(&upstream.base, None, &[]).await;
    let mut socket = open_socket(&proxy, None).await.unwrap();
    assert_eq!(upstream.socket_event().await, "open");
    assert_eq!(next(&mut socket).await, Message::text("o"));
    drop(socket);
    assert_eq!(upstream.socket_event().await, "close:1001:");
    assert_eq!(upstream.socket_event().await, "ended");
    proxy.shutdown().await;
}

#[tokio::test]
async fn socket_frames_are_relayed_both_ways() {
    let mut upstream = Upstream::start("/").await;
    let proxy = proxy(&upstream.base, None, &[]).await;
    let mut socket = open_socket(&proxy, None).await.unwrap();
    assert_eq!(upstream.socket_event().await, "open");
    assert_eq!(next(&mut socket).await, Message::text("o"));
    socket
        .send(Message::text("subscribe room:W1N1"))
        .await
        .unwrap();
    assert_eq!(upstream.socket_event().await, "text:subscribe room:W1N1");
    assert_eq!(
        next(&mut socket).await,
        Message::text("echo:subscribe room:W1N1")
    );
    socket.send(Message::binary(vec![1, 2, 3])).await.unwrap();
    assert_eq!(upstream.socket_event().await, "binary:[1, 2, 3]");
    assert_eq!(next(&mut socket).await, Message::binary(vec![3, 2, 1]));
    socket.send(Message::Ping(vec![7].into())).await.unwrap();
    assert_eq!(upstream.socket_event().await, "ping:[7]");
    proxy.shutdown().await;
}

#[tokio::test]
async fn a_client_close_reaches_the_upstream() {
    let mut upstream = Upstream::start("/").await;
    let proxy = proxy(&upstream.base, None, &[]).await;
    let mut socket = open_socket(&proxy, None).await.unwrap();
    assert_eq!(upstream.socket_event().await, "open");
    assert_eq!(next(&mut socket).await, Message::text("o"));
    let frame = CloseFrame {
        code: CloseCode::Library(4002),
        reason: "done".into(),
    };
    socket.close(Some(frame)).await.unwrap();
    assert_eq!(upstream.socket_event().await, "close:4002:done");
    assert_eq!(upstream.socket_event().await, "ended");
    proxy.shutdown().await;
}

#[tokio::test]
async fn an_upstream_close_reaches_the_client() {
    let mut upstream = Upstream::start("/").await;
    let proxy = proxy(&upstream.base, None, &[]).await;
    let mut socket = open_socket(&proxy, None).await.unwrap();
    assert_eq!(upstream.socket_event().await, "open");
    assert_eq!(next(&mut socket).await, Message::text("o"));
    socket.send(Message::text("close-me")).await.unwrap();
    match next(&mut socket).await {
        Message::Close(Some(frame)) => {
            assert_eq!(u16::from(frame.code), 4001);
            assert_eq!(frame.reason.as_str(), "bye");
        }
        other => panic!("expected a close frame, got {other:?}"),
    }
    // The client's close reply completes the handshake upstream.
    assert_eq!(upstream.socket_event().await, "text:close-me");
    let ended =
        tokio::time::timeout(PATIENCE, async { while socket.next().await.is_some() {} }).await;
    assert!(ended.is_ok(), "client socket still open");
    proxy.shutdown().await;
}

#[tokio::test]
async fn shutdown_closes_open_sockets() {
    let mut upstream = Upstream::start("/").await;
    let proxy = proxy(&upstream.base, None, &[]).await;
    let mut socket = open_socket(&proxy, None).await.unwrap();
    assert_eq!(upstream.socket_event().await, "open");
    assert_eq!(next(&mut socket).await, Message::text("o"));
    tokio::time::timeout(PATIENCE, proxy.shutdown())
        .await
        .unwrap();
    let ended = tokio::time::timeout(PATIENCE, async {
        while let Some(Ok(_)) = socket.next().await {}
    })
    .await;
    assert!(ended.is_ok(), "client socket still open");
}
