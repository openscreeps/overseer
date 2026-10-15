//! Relaying the `socket/websocket` upgrade.

use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use async_tungstenite::WebSocketStream;
use async_tungstenite::tokio::TokioAdapter;
use async_tungstenite::tungstenite::handshake::derive_accept_key;
use async_tungstenite::tungstenite::protocol::frame::coding::CloseCode as ClientCloseCode;
use async_tungstenite::tungstenite::protocol::{CloseFrame, Role, WebSocketConfig};
use async_tungstenite::tungstenite::{Error as ClientError, Message as ClientMessage};
use futures_util::{Sink, SinkExt, Stream, StreamExt as _};
use http::header::{
    CONNECTION, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_KEY, SEC_WEBSOCKET_VERSION, UPGRADE,
};
use http::{HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode};
use hyper::body::Incoming;
use hyper::upgrade::OnUpgrade;
use hyper_util::rt::TokioIo;
use reqwest_websocket::{
    CloseCode as UpstreamCloseCode, Message as UpstreamMessage, Upgrade as _, WebSocket as Upstream,
};
use tokio::time::{Instant, timeout};
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::cors::Origin;
use crate::serve::{Body, SOCKET_PATH, State, millis, status};
use crate::{HANDSHAKE_TIMEOUT, MAX_SOCKET_MESSAGE};

/// How long the second direction may take to finish once the first has ended, and how long
/// closing each side may take.
const CLOSE_GRACE: Duration = Duration::from_secs(2);

/// Answers a socket upgrade: connects the upstream socket first, then switches protocols and
/// relays frames until either side closes.
pub(crate) async fn upgrade(
    state: &Arc<State>,
    request: Request<Incoming>,
    origin: &Origin,
) -> Response<Body> {
    // Browsers let any page open a socket anywhere; only the proxy can refuse foreign pages.
    if *origin == Origin::Denied {
        return status(StatusCode::FORBIDDEN);
    }
    if request.method() != Method::GET {
        return status(StatusCode::METHOD_NOT_ALLOWED);
    }
    let headers = request.headers();
    if !has_token(headers, &UPGRADE, "websocket") || !has_token(headers, &CONNECTION, "upgrade") {
        return status(StatusCode::UPGRADE_REQUIRED);
    }
    let (Some(key), true) = (
        headers.get(SEC_WEBSOCKET_KEY),
        headers
            .get(SEC_WEBSOCKET_VERSION)
            .is_some_and(|version| version == "13"),
    ) else {
        return status(StatusCode::BAD_REQUEST);
    };
    let accept = derive_accept_key(key.as_bytes());
    let path_and_query = request
        .uri()
        .path_and_query()
        .map_or(SOCKET_PATH, |path| path.as_str());
    let Some(url) = state.upstream_url(path_and_query) else {
        return status(StatusCode::NOT_FOUND);
    };
    let upstream = match timeout(HANDSHAKE_TIMEOUT, connect(&state.http, url)).await {
        Ok(Ok(upstream)) => upstream,
        Ok(Err(refused)) => return status(refused),
        Err(_) => return status(StatusCode::GATEWAY_TIMEOUT),
    };
    let client = hyper::upgrade::on(request);
    state
        .tasks
        .spawn(relay(client, upstream, state.stop.clone()));
    let mut response = status(StatusCode::SWITCHING_PROTOCOLS);
    let headers = response.headers_mut();
    headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
    headers.insert(CONNECTION, HeaderValue::from_static("Upgrade"));
    if let Ok(accept) = HeaderValue::try_from(accept) {
        headers.insert(SEC_WEBSOCKET_ACCEPT, accept);
    }
    response
}

/// Whether the comma-separated header `name` holds `token` (case-insensitive).
fn has_token(headers: &HeaderMap, name: &HeaderName, token: &str) -> bool {
    headers
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|item| item.trim().eq_ignore_ascii_case(token))
}

/// Opens the upstream socket; a refusal keeps the upstream's status, other failures are
/// `502 Bad Gateway`.
async fn connect(http: &reqwest::Client, url: url::Url) -> Result<Upstream, StatusCode> {
    let response = http
        .get(url)
        .upgrade()
        .send()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    if response.status() != StatusCode::SWITCHING_PROTOCOLS {
        return Err(response.status());
    }
    response
        .into_websocket()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)
}

/// Relays frames between the client and the upstream until both sides are closed, one side
/// has been gone for [`CLOSE_GRACE`], or the proxy stops.
async fn relay(client: OnUpgrade, upstream: Upstream, stop: CancellationToken) {
    let started = Instant::now();
    let Ok(client) = client.await else {
        return;
    };
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_SOCKET_MESSAGE))
        .max_frame_size(Some(MAX_SOCKET_MESSAGE));
    let client = WebSocketStream::from_raw_socket(
        TokioAdapter::new(TokioIo::new(client)),
        Role::Server,
        Some(config),
    )
    .await;
    let (mut client_sink, client_stream) = client.split();
    let (mut upstream_sink, upstream_stream) = upstream.split();
    {
        let mut outgoing = pin!(client_to_upstream(client_stream, &mut upstream_sink));
        let mut incoming = pin!(upstream_to_client(upstream_stream, &mut client_sink));
        tokio::select! {
            () = stop.cancelled() => {}
            () = &mut outgoing => { let _ = timeout(CLOSE_GRACE, incoming).await; }
            () = &mut incoming => { let _ = timeout(CLOSE_GRACE, outgoing).await; }
        }
    }
    // Sends a close frame where none was sent yet (the proxy stopping) and flushes the replies.
    let _ = timeout(CLOSE_GRACE, SinkExt::close(&mut client_sink)).await;
    let _ = timeout(CLOSE_GRACE, upstream_sink.close()).await;
    info!(
        path = SOCKET_PATH,
        duration_ms = millis(started),
        "socket closed"
    );
}

/// Client frames to the upstream, until the client closes or goes away.
async fn client_to_upstream<S, K>(mut client: S, upstream: &mut K)
where
    S: Stream<Item = Result<ClientMessage, ClientError>> + Unpin,
    K: Sink<UpstreamMessage> + Unpin,
{
    while let Some(Ok(message)) = client.next().await {
        let message = match message {
            ClientMessage::Text(text) => UpstreamMessage::Text(text.as_str().to_owned()),
            ClientMessage::Binary(data) => UpstreamMessage::Binary(data),
            ClientMessage::Ping(data) => UpstreamMessage::Ping(data),
            ClientMessage::Pong(data) => UpstreamMessage::Pong(data),
            ClientMessage::Close(frame) => {
                let (code, reason) = frame.map_or_else(
                    || (UpstreamCloseCode::Normal, String::new()),
                    |frame| {
                        (
                            u16::from(frame.code).into(),
                            frame.reason.as_str().to_owned(),
                        )
                    },
                );
                let _ = upstream.send(UpstreamMessage::Close { code, reason }).await;
                return;
            }
            // Raw frames are only produced when writing.
            ClientMessage::Frame(_) => continue,
        };
        if upstream.send(message).await.is_err() {
            return;
        }
    }
    let going_away = UpstreamMessage::Close {
        code: UpstreamCloseCode::Away,
        reason: String::new(),
    };
    let _ = upstream.send(going_away).await;
}

/// Upstream frames to the client, until the upstream closes or goes away.
async fn upstream_to_client<S, K>(mut upstream: S, client: &mut K)
where
    S: Stream<Item = Result<UpstreamMessage, reqwest_websocket::Error>> + Unpin,
    K: Sink<ClientMessage> + Unpin,
{
    while let Some(Ok(message)) = upstream.next().await {
        let message = match message {
            UpstreamMessage::Text(text) => ClientMessage::text(text),
            UpstreamMessage::Binary(data) => ClientMessage::Binary(data),
            UpstreamMessage::Ping(data) => ClientMessage::Ping(data),
            UpstreamMessage::Pong(data) => ClientMessage::Pong(data),
            UpstreamMessage::Close { code, reason } => {
                let code = ClientCloseCode::from(u16::from(code));
                // Codes such as 1005 (no status) describe a close but may not be sent.
                let frame = code.is_allowed().then(|| CloseFrame {
                    code,
                    reason: reason.into(),
                });
                let _ = client.send(ClientMessage::Close(frame)).await;
                return;
            }
        };
        if client.send(message).await.is_err() {
            return;
        }
    }
    let lost = ClientMessage::Close(Some(CloseFrame {
        code: ClientCloseCode::Error,
        reason: "upstream connection lost".into(),
    }));
    let _ = client.send(lost).await;
}
