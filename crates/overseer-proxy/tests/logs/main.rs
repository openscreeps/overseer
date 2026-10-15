//! The proxy's logs never hold a secret. Alone in its test process: `tracing` caches whether a
//! log statement is enabled per process, so tests on other threads without the capturing
//! subscriber would disable statements for this one.

#[path = "../proxy/upstream.rs"]
#[expect(dead_code, reason = "shared with the proxy tests, which use all of it")]
mod upstream;

use std::io::Write;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_tungstenite::tokio::client_async;
use async_tungstenite::tungstenite::Message;
use async_tungstenite::tungstenite::client::IntoClientRequest as _;
use futures_util::StreamExt as _;
use http::HeaderValue;
use overseer_proxy::{Proxy, ProxyConfig};
use tokio::net::TcpStream;
use upstream::{ROTATED_TOKEN, Upstream};

/// Upper bound of every wait in this test.
const PATIENCE: Duration = Duration::from_secs(10);

const ALLOWED: &str = "http://allowed.example";

/// A `tracing` writer collecting everything into one buffer.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Captured {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap_or_else(PoisonError::into_inner)).into_owned()
    }
}

impl Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Every log line at every level, from the proxy and its dependencies, while a signed-in
/// client uses HTTP and the socket: none holds a token, password, query, body or frame.
#[tokio::test]
async fn logs_never_contain_secrets() {
    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(captured.clone())
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    // This test's runtime is single-threaded: every task logs on this thread.
    let _guard = tracing::subscriber::set_default(subscriber);

    let mut upstream = Upstream::start("/").await;
    let proxy = Proxy::start(ProxyConfig {
        upstream: upstream.base.clone(),
        listen: "127.0.0.1:0".parse().unwrap(),
        static_dir: None,
        allowed_origins: vec![ALLOWED.to_owned()],
    })
    .await
    .unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = client
        .post(proxy.url().join("api/echo?path=SECRET-QUERY").unwrap())
        .header("Origin", ALLOWED)
        .header("X-Token", "SECRET-TOKEN")
        .header("X-Server-Password", "SECRET-SERVER-PASSWORD")
        .header("Content-Type", "application/json")
        .body(r#"{"email":"a@b.c","password":"SECRET-PASSWORD"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.headers().get("x-token").unwrap(),
        HeaderValue::from_static(ROTATED_TOKEN)
    );
    assert_eq!(response.text().await.unwrap(), r#"{"ok":1}"#);

    let mut request = proxy
        .url()
        .join("socket/websocket")
        .unwrap()
        .as_str()
        .replacen("http", "ws", 1)
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("origin", HeaderValue::from_static(ALLOWED));
    let address = proxy.url().socket_addrs(|| None).unwrap()[0];
    let stream = TcpStream::connect(address).await.unwrap();
    let (mut socket, _) = client_async(request, stream).await.unwrap();
    assert_eq!(upstream.socket_event().await, "open");
    socket
        .send(Message::text("auth SECRET-TOKEN"))
        .await
        .unwrap();
    let mut received = Vec::new();
    while received.len() < 2 {
        let message = tokio::time::timeout(PATIENCE, socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let Message::Text(text) = message {
            received.push(text.as_str().to_owned());
        }
    }
    assert_eq!(received, ["o", "echo:auth SECRET-TOKEN"]);
    assert_eq!(upstream.socket_event().await, "text:auth SECRET-TOKEN");
    socket.close(None).await.unwrap();
    assert_eq!(upstream.socket_event().await, "close:1000:");
    proxy.shutdown().await;

    let logs = captured.text();
    for expected in [
        "method=POST path=/api/echo status=200",
        "method=GET path=/socket/websocket status=101",
        "socket closed",
    ] {
        assert!(logs.contains(expected), "`{expected}` not logged:\n{logs}");
    }
    for secret in ["SECRET", ROTATED_TOKEN, "a@b.c", "echo:"] {
        assert!(!logs.contains(secret), "`{secret}` logged:\n{logs}");
    }
}
