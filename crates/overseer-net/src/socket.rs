//! The socket driver: connection, keep-alive, reconnects, re-authentication and the event
//! stream.

mod keep_alive;
mod subscribe;
mod view;

use std::collections::HashMap;
use std::fmt;
use std::future::poll_fn;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use std::time::Duration;

use futures_util::stream::FusedStream;
use futures_util::{Sink, Stream, StreamExt};
use overseer_core::Token;
use overseer_core::api::Terrain;
use overseer_core::socket::{Channel, Event, SocketError, SocketSession};
use reqwest_websocket::{CloseCode, Message, WebSocket};

pub use view::RoomView;

use crate::rt::{BoxFuture, Instant, MaybeSend, Sleep, sleep_until};
use crate::{Client, Error, World};
use keep_alive::KeepAlive;

/// How a dropped socket connection is re-established.
///
/// The `n`th consecutive attempt waits `initial_delay · 2ⁿ⁻¹`, at most `max_delay`. The count
/// starts over once a connection is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReconnectPolicy {
    /// Delay before the first attempt.
    pub initial_delay: Duration,
    /// Upper bound of the delay.
    pub max_delay: Duration,
    /// Consecutive attempts before giving up; `None` retries forever and `Some(0)` never
    /// reconnects.
    pub max_attempts: Option<u32>,
}

impl Default for ReconnectPolicy {
    /// 1 s doubling up to 60 s, retrying forever.
    fn default() -> Self {
        Self {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            max_attempts: None,
        }
    }
}

impl ReconnectPolicy {
    /// The delay before attempt `attempt` (counted from 1).
    #[must_use]
    pub fn delay(&self, attempt: u32) -> Duration {
        let factor = 1_u32
            .checked_shl(attempt.saturating_sub(1))
            .unwrap_or(u32::MAX);
        self.initial_delay
            .saturating_mul(factor)
            .min(self.max_delay)
    }

    /// Whether attempt `attempt` (counted from 1) may be made.
    fn allows(&self, attempt: u32) -> bool {
        self.max_attempts.is_none_or(|max| attempt <= max)
    }
}

/// Something that happened on a [`Socket`].
#[derive(Debug)]
pub enum SocketEvent {
    /// A connection is open; subscriptions are being sent (after `auth ok` when signed in).
    Connected,
    /// The connection or a connection attempt was lost.
    Disconnected(DisconnectReason),
    /// The next connection attempt starts after `delay`.
    Reconnecting {
        /// Consecutive attempt number, from 1.
        attempt: u32,
        /// Wait before the attempt.
        delay: Duration,
    },
    /// A protocol event.
    Event(Event),
}

/// Why a connection was lost.
#[derive(Debug)]
pub enum DisconnectReason {
    /// The server sent a close frame.
    Closed {
        /// The close code.
        code: u16,
        /// The close reason.
        reason: String,
    },
    /// The connection ended without a close frame.
    Ended,
    /// Reading or writing failed.
    Transport(Error),
    /// Nothing was received for two keep-alive intervals. Never reported in browsers, which
    /// send no keep-alive pings.
    KeepAliveTimeout,
    /// A connection attempt failed.
    ConnectFailed(Error),
}

/// A socket connection that reconnects and resubscribes by itself.
///
/// Created by [`Client::connect`]. Drive it with [`Self::next`], or as a [`Stream`] of
/// `Result<SocketEvent, Error>` that ends when the reconnect policy gives up; subscriptions
/// requested at any time are sent by the next poll.
#[derive(Debug)]
pub struct Socket {
    client: Client,
    world: World,
    session: SocketSession,
    link: Link,
    /// The backoff deadline, or the keep-alive deadline of the open connection.
    timer: Pin<Box<Sleep>>,
    /// Token sent with the current connection's `auth`, to spot one rotated by `auth ok`.
    sent_token: Option<Token>,
    /// A sign-in after `auth failed` with credentials; its token replaces the session's.
    renewal: Option<Task<Result<Token, Error>>>,
    /// A sign-in after `auth failed` was made since the last `auth ok`.
    renewed: bool,
    /// Decoded terrain of the rooms opened with [`Self::watch_room`], by room channel.
    terrain: HashMap<Channel, Arc<Terrain>>,
}

/// A boxed future, kept across polls.
struct Task<T>(BoxFuture<'static, T>);

impl<T> Task<T> {
    fn new(future: impl Future<Output = T> + MaybeSend + 'static) -> Self {
        Self(Box::pin(future))
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<T> {
        self.0.as_mut().poll(cx)
    }
}

impl<T> fmt::Debug for Task<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Task")
    }
}

/// State of the transport.
#[derive(Debug)]
enum Link {
    /// An open connection; `announced` once [`SocketEvent::Connected`] was returned.
    Open {
        connection: Box<Connection>,
        announced: bool,
    },
    /// Waiting for the timer to start attempt `attempt`; `announced` once
    /// [`SocketEvent::Reconnecting`] was returned.
    Backoff {
        attempt: u32,
        delay: Duration,
        announced: bool,
    },
    /// Attempt `attempt` in progress.
    Opening {
        attempt: u32,
        future: Task<Result<WebSocket, Error>>,
    },
    /// Given up, or closed.
    Closed,
}

/// An open WebSocket and its keep-alive state.
#[derive(Debug)]
struct Connection {
    websocket: WebSocket,
    keep_alive: KeepAlive,
    /// Frames were handed to the connection but not flushed yet.
    unflushed: bool,
}

impl Connection {
    fn new(websocket: WebSocket) -> Self {
        Self {
            websocket,
            keep_alive: KeepAlive::new(),
            unflushed: false,
        }
    }
}

/// What one turn on an open connection produced.
enum Step {
    Event(Event),
    Undecodable(SocketError),
    Down(DisconnectReason),
}

impl Socket {
    pub(crate) fn new(client: Client, world: World, websocket: WebSocket) -> Self {
        let sent_token = client.token();
        Self {
            session: SocketSession::new(sent_token.clone()),
            client,
            world,
            link: Link::Open {
                connection: Box::new(Connection::new(websocket)),
                announced: false,
            },
            timer: Box::pin(sleep_until(Instant::now())),
            sent_token,
            renewal: None,
            renewed: false,
            terrain: HashMap::new(),
        }
    }

    /// The protocol state: subscriptions, token and room states.
    #[must_use]
    pub const fn session(&self) -> &SocketSession {
        &self.session
    }

    /// The client this socket was opened by.
    #[must_use]
    pub const fn client(&self) -> &Client {
        &self.client
    }

    /// The world channel names are built for.
    #[must_use]
    pub const fn world(&self) -> &World {
        &self.world
    }

    /// Replaces the shard used by helpers that do not name one. Single-world servers have no
    /// shards and ignore it.
    pub fn set_default_shard(&mut self, shard: impl Into<String>) {
        self.world.set_shard(shard);
    }

    /// Sends queued commands, then waits for the next event. Returns `Ok(None)` once the socket
    /// is closed for good: the reconnect policy gave up.
    ///
    /// Commands queued while handling a received frame (such as the subscriptions that follow
    /// `auth ok`) are handed to the connection before that frame's event is returned, so a
    /// server event published right after [`Event::Authenticated`] already finds them.
    /// Subscriptions made by the application between calls are sent by the next call, or at
    /// once with [`Self::flush`].
    ///
    /// Lost connections are reported with [`SocketEvent::Disconnected`] and reopened after
    /// [`SocketEvent::Reconnecting`]; the subscriptions are sent again on the new connection
    /// (room states start over). `auth ok` tokens replace the client's token. With credentials,
    /// `auth failed` signs in once and authenticates again instead of returning
    /// [`Event::AuthFailed`].
    ///
    /// Cancel-safe: queued commands leave the session only when the connection accepts them, no
    /// received frame is lost, and backoff deadlines, connection attempts and pending sign-ins
    /// are kept when the future is dropped. The [`Stream`] implementation yields the same
    /// events.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Socket`] for an undecodable frame (the connection stays usable) and the
    /// errors of a sign-in after `auth failed`. The socket remains usable after either.
    pub async fn next(&mut self) -> Result<Option<SocketEvent>, Error> {
        poll_fn(|cx| self.poll_event(cx)).await
    }

    /// Sends queued commands (subscriptions, unsubscriptions) now instead of with the next call
    /// to [`Self::next`]. Without an open connection they stay queued and are sent after it is
    /// (re)opened and authenticated.
    ///
    /// # Errors
    ///
    /// Returns [`Error::WebSocket`] when the connection fails; the next call to [`Self::next`]
    /// then reports the disconnection and reconnects.
    pub async fn flush(&mut self) -> Result<(), Error> {
        poll_fn(|cx| match &mut self.link {
            Link::Open { connection, .. } => poll_send(connection, &mut self.session, cx),
            Link::Backoff { .. } | Link::Opening { .. } | Link::Closed => Poll::Ready(Ok(())),
        })
        .await
    }

    /// Closes the connection normally; the socket does not reconnect.
    ///
    /// # Errors
    ///
    /// Returns [`Error::WebSocket`] when the close handshake fails.
    pub async fn close(mut self) -> Result<(), Error> {
        match std::mem::replace(&mut self.link, Link::Closed) {
            Link::Open { connection, .. } => {
                Ok(connection.websocket.close(CloseCode::Normal, None).await?)
            }
            Link::Backoff { .. } | Link::Opening { .. } | Link::Closed => Ok(()),
        }
    }

    /// The state machine behind [`Self::next`] and the [`Stream`] implementation; all progress
    /// is kept in `self`, so a dropped poller loses nothing.
    fn poll_event(&mut self, cx: &mut Context<'_>) -> Poll<Result<Option<SocketEvent>, Error>> {
        loop {
            match &mut self.link {
                Link::Closed => return Poll::Ready(Ok(None)),
                Link::Open { announced, .. } if !*announced => {
                    *announced = true;
                    return Poll::Ready(Ok(Some(SocketEvent::Connected)));
                }
                Link::Open { connection, .. } => {
                    if let Some(renewal) = &mut self.renewal {
                        let token = ready!(renewal.poll(cx));
                        self.renewal = None;
                        let token = token?;
                        self.sent_token = Some(token.clone());
                        self.session.authenticate(token);
                    }
                    let keep_alive = self.client.keep_alive();
                    let step = ready!(poll_drive(
                        connection,
                        &mut self.session,
                        keep_alive,
                        &mut self.timer,
                        cx
                    ));
                    match step {
                        Step::Event(event) => {
                            if let Some(event) = self.after_event(event) {
                                return Poll::Ready(Ok(Some(SocketEvent::Event(event))));
                            }
                        }
                        Step::Undecodable(error) => return Poll::Ready(Err(error.into())),
                        Step::Down(reason) => {
                            self.schedule(1);
                            return Poll::Ready(Ok(Some(SocketEvent::Disconnected(reason))));
                        }
                    }
                }
                Link::Backoff {
                    attempt,
                    delay,
                    announced,
                } if !*announced => {
                    *announced = true;
                    return Poll::Ready(Ok(Some(SocketEvent::Reconnecting {
                        attempt: *attempt,
                        delay: *delay,
                    })));
                }
                Link::Backoff { attempt, .. } => {
                    let attempt = *attempt;
                    ready!(self.timer.as_mut().poll(cx));
                    let client = self.client.clone();
                    self.link = Link::Opening {
                        attempt,
                        future: Task::new(async move { client.open().await }),
                    };
                }
                Link::Opening { attempt, future } => {
                    let attempt = *attempt;
                    match ready!(future.poll(cx)) {
                        Ok(websocket) => {
                            self.sent_token = self.client.token();
                            self.session.reconnect(self.sent_token.clone());
                            self.link = Link::Open {
                                connection: Box::new(Connection::new(websocket)),
                                announced: true,
                            };
                            return Poll::Ready(Ok(Some(SocketEvent::Connected)));
                        }
                        Err(error) => {
                            self.schedule(attempt + 1);
                            return Poll::Ready(Ok(Some(SocketEvent::Disconnected(
                                DisconnectReason::ConnectFailed(error),
                            ))));
                        }
                    }
                }
            }
        }
    }

    /// Waits before attempt `attempt`, or closes when the policy allows no more.
    fn schedule(&mut self, attempt: u32) {
        let policy = self.client.reconnect_policy();
        self.link = if policy.allows(attempt) {
            let delay = policy.delay(attempt);
            self.timer.as_mut().reset(Instant::now() + delay);
            Link::Backoff {
                attempt,
                delay,
                announced: false,
            }
        } else {
            Link::Closed
        };
    }

    /// Shares rotated tokens and handles `auth failed`; returns the event unless it is handled
    /// here.
    fn after_event(&mut self, event: Event) -> Option<Event> {
        match event {
            Event::Authenticated => {
                self.renewed = false;
                if self.session.token() != self.sent_token.as_ref() {
                    self.sent_token = self.session.token().cloned();
                    self.client.set_token(self.sent_token.clone());
                }
            }
            Event::AuthFailed if self.client.can_sign_in() && !self.renewed => {
                self.renewed = true;
                let client = self.client.clone();
                let rejected = self.session.token().cloned();
                self.renewal = Some(Task::new(
                    async move { client.renew(rejected.as_ref()).await },
                ));
                return None;
            }
            _ => {}
        }
        Some(event)
    }
}

impl Stream for Socket {
    type Item = Result<SocketEvent, Error>;

    /// The events of [`Socket::next`]; the stream ends when the reconnect policy gives up.
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().poll_event(cx).map(Result::transpose)
    }
}

impl FusedStream for Socket {
    fn is_terminated(&self) -> bool {
        matches!(self.link, Link::Closed)
    }
}

/// Sends queued commands and keep-alive pings and reads until a frame produces an event or the
/// connection is lost.
fn poll_drive(
    connection: &mut Connection,
    session: &mut SocketSession,
    keep_alive: Option<Duration>,
    timer: &mut Pin<Box<Sleep>>,
    cx: &mut Context<'_>,
) -> Poll<Step> {
    loop {
        // A connection that cannot take frames yet is still read.
        if let Poll::Ready(Err(error)) = poll_send(connection, session, cx) {
            return Poll::Ready(Step::Down(DisconnectReason::Transport(error)));
        }
        match connection.websocket.poll_next_unpin(cx) {
            Poll::Ready(None) => return Poll::Ready(Step::Down(DisconnectReason::Ended)),
            Poll::Ready(Some(Err(error))) => {
                return Poll::Ready(Step::Down(DisconnectReason::Transport(error.into())));
            }
            Poll::Ready(Some(Ok(message))) => {
                connection.keep_alive.received();
                match message {
                    Message::Text(frame) => match session.handle_frame(&frame) {
                        Ok(Some(event)) => {
                            // Commands this frame queued (subscriptions after `auth ok`) go out
                            // before the event is returned. A send error resurfaces on the next
                            // poll as a disconnection, so it is not reported here.
                            let _ = poll_send(connection, session, cx);
                            return Poll::Ready(Step::Event(event));
                        }
                        Ok(None) => {}
                        Err(error) => return Poll::Ready(Step::Undecodable(error)),
                    },
                    Message::Close { code, reason } => {
                        return Poll::Ready(Step::Down(DisconnectReason::Closed {
                            code: code.into(),
                            reason,
                        }));
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    Message::Binary(_) | Message::Ping(_) | Message::Pong(_) => {}
                    // Browsers answer pings themselves and deliver neither pings nor pongs.
                    #[cfg(target_arch = "wasm32")]
                    _ => {}
                }
                continue;
            }
            Poll::Pending => {}
        }
        if !ready!(connection.keep_alive.poll(keep_alive, timer, cx)) {
            return Poll::Ready(Step::Down(DisconnectReason::KeepAliveTimeout));
        }
    }
}

/// Moves a due ping and the queued commands into the connection and flushes them; a command
/// leaves the session only once the connection can take it.
fn poll_send(
    connection: &mut Connection,
    session: &mut SocketSession,
    cx: &mut Context<'_>,
) -> Poll<Result<(), Error>> {
    let mut websocket = Pin::new(&mut connection.websocket);
    #[cfg(not(target_arch = "wasm32"))]
    if connection.keep_alive.ping_due {
        ready!(Sink::<Message>::poll_ready(websocket.as_mut(), cx))?;
        websocket
            .as_mut()
            .start_send(Message::Ping(reqwest_websocket::Bytes::new()))?;
        connection.keep_alive.ping_due = false;
        connection.unflushed = true;
    }
    loop {
        ready!(Sink::<Message>::poll_ready(websocket.as_mut(), cx))?;
        let Some(frame) = session.poll_transmit() else {
            break;
        };
        websocket.as_mut().start_send(Message::Text(frame))?;
        connection.unflushed = true;
    }
    if connection.unflushed {
        ready!(Sink::<Message>::poll_flush(websocket, cx))?;
        connection.unflushed = false;
    }
    Poll::Ready(Ok(()))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{ReconnectPolicy, Socket};
    use crate::Client;

    /// Applications move sockets, and the futures of the socket and client helpers, into
    /// spawned tasks; checked at compile time.
    #[test]
    fn sockets_and_helper_futures_are_send() {
        fn send<T: Send>(_: &T) {}
        let _: fn(&mut Socket) = |socket| {
            send(socket);
            send(&socket.next());
            send(&socket.watch_room("W1N1"));
        };
        let _: fn(&Client) = |client| {
            send(&client.terrain(None, "W1N1"));
            send(&client.room_history(None, "W1N1", 0));
            send(&client.memory(None, None));
        };
    }

    #[test]
    fn delays_double_up_to_the_maximum() {
        let policy = ReconnectPolicy {
            initial_delay: Duration::from_millis(250),
            max_delay: Duration::from_secs(2),
            max_attempts: Some(3),
        };
        let delays: Vec<_> = (1..=6).map(|attempt| policy.delay(attempt)).collect();
        assert_eq!(
            delays,
            [250, 500, 1000, 2000, 2000, 2000].map(Duration::from_millis)
        );
        assert_eq!(policy.delay(u32::MAX), Duration::from_secs(2));
        assert!(policy.allows(3));
        assert!(!policy.allows(4));
        assert!(ReconnectPolicy::default().allows(u32::MAX));
    }
}
