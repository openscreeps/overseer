//! Socket connection state: frames to send, received frames, subscriptions and room states.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;

use serde::Deserialize;
use serde::de::{self, DeserializeSeed, Deserializer, IgnoredAny, SeqAccess, Visitor};

use super::channel::Channel;
use super::room::RoomState;
use super::visual;
use super::{community, official, openscreeps};
use crate::Token;
use crate::gz::{self, GzError};
use crate::model::code::{ActiveBranchChange, CodeChange};
use crate::model::console::ConsoleEvent;
use crate::model::messages::MessageUpdate;
use crate::model::platform::ServerKind;
use crate::model::socket::{RoomEvent, UserCpu, UserResources};
use crate::model::world::{MapVisualItem, RoomMap};

/// Something the server sent, decoded by [`SocketSession::handle_frame`].
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Greeting `time`: server clock in milliseconds since the Unix epoch.
    Time(u64),
    /// Greeting `protocol`: socket protocol version.
    Protocol(u32),
    /// Greeting `package`: official client package version (official server only).
    Package(u32),
    /// `auth ok`: the connection is signed in and [`SocketSession::token`] holds the token to
    /// keep using (standalone servers rotate it). Queued subscriptions are now being sent.
    Authenticated,
    /// `auth failed`: the token was rejected; no subscription is sent until
    /// [`SocketSession::authenticate`] succeeds.
    AuthFailed,
    /// `server down`: the server is under maintenance (official server only).
    ServerDown,
    /// A line starting with `cannot subscribe` (the whole line; official server only).
    CannotSubscribe(String),
    /// An event of a subscribed channel.
    Channel {
        /// The subscribed channel.
        channel: Channel,
        /// Its data.
        payload: Payload,
    },
    /// An `err@<channel>` event, for example `subscribe limit reached` for a room. The
    /// subscription stays registered.
    ChannelError {
        /// The subscribed channel.
        channel: Channel,
        /// The error text.
        message: String,
    },
    /// An event for a channel not subscribed on this connection, for example one still in
    /// flight after unsubscribing. The data is dropped.
    Unsolicited {
        /// The event's channel name as received (with `err@` for error events).
        channel: String,
    },
}

/// Data of a channel event.
#[derive(Clone, Debug, PartialEq)]
pub enum Payload {
    /// `server-message`: the announcement text.
    ServerMessage(String),
    /// `room:`: the update was applied; read the state with [`SocketSession::room`].
    Room,
    /// `roomMap2:`: the complete map view of the room.
    RoomMap(RoomMap),
    /// `mapVisual:`: the user's map visual items (one per line of the sent string); empty when
    /// there are none.
    MapVisual(Vec<MapVisualItem>),
    /// `user:<user>/cpu`.
    Cpu(UserCpu),
    /// `user:<user>/console`.
    Console(ConsoleEvent),
    /// `user:<user>/memory/…`: the JavaScript string form of the value (`undefined` when
    /// absent, `[object Object]` for objects, `Incorrect memory path`).
    Memory(String),
    /// `user:<user>/resources`.
    Resources(UserResources),
    /// `user:<user>/code`.
    Code(CodeChange),
    /// `user:<user>/set-active-branch`.
    ActiveBranch(ActiveBranchChange),
    /// `user:<user>/newMessage` and `user:<user>/message:<respondent>`.
    Message(MessageUpdate),
}

/// Authentication state of the current connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    /// `auth` is queued or sent; subscriptions wait for `auth ok`.
    Authenticating,
    /// Signed in, or anonymous: subscriptions are sent as they are requested.
    Ready,
    /// `auth failed`; subscriptions wait for a successful [`SocketSession::authenticate`].
    AuthFailed,
}

/// A received frame that could not be handled. The session state is unchanged.
#[derive(Debug, thiserror::Error)]
pub enum SocketError {
    /// A `gz:` frame did not inflate.
    #[error("undecodable compressed frame")]
    Gz(#[from] GzError),
    /// A frame starting with `[` is not a `[channel, data]` event.
    #[error("malformed channel event")]
    Json(#[source] serde_json::Error),
    /// A protocol line this client does not know.
    #[error("unrecognized protocol line: {0}")]
    UnrecognizedLine(String),
    /// The data of a subscribed channel's event does not have the documented shape.
    #[error("undecodable data of channel {channel}")]
    Payload {
        /// The subscribed channel.
        channel: Channel,
        /// The decoding error.
        source: serde_json::Error,
    },
}

/// A frame waiting in the transmit queue.
#[derive(Debug)]
enum Outgoing {
    /// `gzip on`.
    Gzip,
    /// `auth <token>`, rendered when polled so the token never sits in the queue.
    Auth,
    /// Any other command.
    Line(String),
}

/// One desired subscription.
#[derive(Debug)]
struct Subscription {
    channel: Channel,
    /// `subscribe` was queued on the current connection.
    sent: bool,
    /// Room state of a `room:` subscription, from its first event on this connection.
    room: Option<RoomState>,
}

/// The client side of one socket connection, without I/O.
///
/// The driver opens the transport ([`ServerUrl::socket`](crate::ServerUrl::socket)), sends
/// every frame from [`poll_transmit`](Self::poll_transmit) as one text frame, passes every
/// received text frame to [`handle_frame`](Self::handle_frame) and calls
/// [`reconnect`](Self::reconnect) when it opens a new transport. Desired subscriptions survive
/// reconnects; the session resubscribes once the new connection is authenticated.
///
/// Channel events are decoded with the socket contract of the server's [`ServerKind`] and
/// converted into the [domain types](crate::model::socket).
#[derive(Debug)]
pub struct SocketSession {
    kind: ServerKind,
    state: SessionState,
    token: Option<Token>,
    outgoing: VecDeque<Outgoing>,
    /// Desired subscriptions in the order they were requested.
    subscriptions: BTreeMap<u64, Subscription>,
    /// Wire name of each desired subscription to its key in `subscriptions`.
    names: HashMap<String, u64>,
    next_key: u64,
}

impl SocketSession {
    /// New connection to a server speaking `kind`'s contract. Queues `gzip on`, then
    /// `auth <token>` when a token is given (state
    /// [`Authenticating`](SessionState::Authenticating)); without a token the session is
    /// [`Ready`](SessionState::Ready) (anonymous) immediately.
    #[must_use]
    pub fn new(token: Option<Token>, kind: ServerKind) -> Self {
        let mut session = Self {
            kind,
            state: SessionState::Ready,
            token,
            outgoing: VecDeque::new(),
            subscriptions: BTreeMap::new(),
            names: HashMap::new(),
            next_key: 0,
        };
        session.connect();
        session
    }

    /// The contract channel events are decoded with.
    #[must_use]
    pub const fn kind(&self) -> ServerKind {
        self.kind
    }

    /// Authentication state of the current connection.
    #[must_use]
    pub const fn state(&self) -> SessionState {
        self.state
    }

    /// The token in use: the latest one given or returned by `auth ok`.
    #[must_use]
    pub const fn token(&self) -> Option<&Token> {
        self.token.as_ref()
    }

    /// Remembers the subscription and sends `subscribe <name>` now if the session is
    /// [`Ready`](SessionState::Ready) and it was not sent on this connection yet.
    pub fn subscribe(&mut self, channel: Channel) {
        let name = channel.to_string();
        if self.names.contains_key(&name) {
            return;
        }
        let sent = self.state == SessionState::Ready;
        if sent {
            self.outgoing
                .push_back(Outgoing::Line(format!("subscribe {name}")));
        }
        let key = self.next_key;
        self.next_key += 1;
        self.names.insert(name, key);
        self.subscriptions.insert(
            key,
            Subscription {
                channel,
                sent,
                room: None,
            },
        );
    }

    /// Forgets the subscription and drops its room state; sends `unsubscribe <name>` only if
    /// `subscribe` was sent on this connection.
    pub fn unsubscribe(&mut self, channel: &Channel) {
        let frame = format!("unsubscribe {channel}");
        let Some(key) = self.names.remove(&frame["unsubscribe ".len()..]) else {
            return;
        };
        if self
            .subscriptions
            .remove(&key)
            .is_some_and(|subscription| subscription.sent)
        {
            self.outgoing.push_back(Outgoing::Line(frame));
        }
    }

    /// The desired subscriptions, in the order they were requested.
    pub fn subscriptions(&self) -> impl Iterator<Item = &Channel> {
        self.subscriptions
            .values()
            .map(|subscription| &subscription.channel)
    }

    /// Next text frame to send on the transport, in order.
    pub fn poll_transmit(&mut self) -> Option<String> {
        while let Some(frame) = self.outgoing.pop_front() {
            match frame {
                Outgoing::Gzip => return Some("gzip on".to_owned()),
                Outgoing::Auth => {
                    if let Some(token) = &self.token {
                        return Some(format!("auth {}", token.as_str()));
                    }
                }
                Outgoing::Line(line) => return Some(line),
            }
        }
        None
    }

    /// Handles one received text frame, inflating `gz:` frames first.
    ///
    /// Returns `Ok(None)` for an empty frame. Room events update the subscription's
    /// [`RoomState`] before [`Payload::Room`] is returned.
    ///
    /// # Errors
    ///
    /// Returns [`SocketError`] for undecodable compressed frames, malformed events, unknown
    /// protocol lines and event data of subscribed channels that does not decode. The session
    /// state is left unchanged.
    pub fn handle_frame(&mut self, frame: &str) -> Result<Option<Event>, SocketError> {
        if frame.starts_with(gz::PREFIX) {
            let text = gz::decode(frame)?;
            self.handle_text(&text)
        } else {
            self.handle_text(frame)
        }
    }

    /// The transport reconnected: clears per-connection state (sent subscriptions, room states,
    /// queued frames), keeps the desired subscriptions and queues `gzip on` and `auth` again.
    /// `token: None` keeps the current token.
    pub fn reconnect(&mut self, token: Option<Token>) {
        if token.is_some() {
            self.token = token;
        }
        self.outgoing.clear();
        for subscription in self.subscriptions.values_mut() {
            subscription.sent = false;
            subscription.room = None;
        }
        self.connect();
    }

    /// Queues `auth <token>` and waits for the reply (state
    /// [`Authenticating`](SessionState::Authenticating)), after `auth failed` or to switch
    /// accounts on a fresh connection.
    pub fn authenticate(&mut self, token: Token) {
        self.token = Some(token);
        self.outgoing.push_back(Outgoing::Auth);
        self.state = SessionState::Authenticating;
    }

    /// Room state of a subscribed `room:` channel, once its first event arrived on this
    /// connection.
    #[must_use]
    pub fn room(&self, channel: &Channel) -> Option<&RoomState> {
        self.subscriptions
            .values()
            .find(|subscription| subscription.channel == *channel)?
            .room
            .as_ref()
    }

    /// Queues the opening commands of a connection.
    fn connect(&mut self) {
        self.outgoing.push_back(Outgoing::Gzip);
        if self.token.is_some() {
            self.outgoing.push_back(Outgoing::Auth);
            self.state = SessionState::Authenticating;
        } else {
            self.ready();
        }
    }

    /// Enters [`SessionState::Ready`] and sends every subscription not sent yet, in order.
    fn ready(&mut self) {
        self.state = SessionState::Ready;
        for subscription in self
            .subscriptions
            .values_mut()
            .filter(|subscription| !subscription.sent)
        {
            subscription.sent = true;
            self.outgoing.push_back(Outgoing::Line(format!(
                "subscribe {}",
                subscription.channel
            )));
        }
    }

    fn handle_text(&mut self, text: &str) -> Result<Option<Event>, SocketError> {
        if text.starts_with('[') {
            return self.handle_event(text).map(Some);
        }
        let event = if text.is_empty() {
            return Ok(None);
        } else if let Some(time) = text.strip_prefix("time ") {
            Event::Time(parse_number(text, time)?)
        } else if let Some(protocol) = text.strip_prefix("protocol ") {
            Event::Protocol(parse_number(text, protocol)?)
        } else if let Some(package) = text.strip_prefix("package ") {
            Event::Package(parse_number(text, package)?)
        } else if let Some(rest) = text.strip_prefix("auth ok") {
            let token = match rest.strip_prefix(' ') {
                Some(token) if !token.is_empty() && !token.contains(char::is_whitespace) => {
                    Some(token)
                }
                None if rest.is_empty() => None,
                // Not echoed: the line may hold a token.
                _ => return Err(SocketError::UnrecognizedLine("auth ok …".to_owned())),
            };
            if let Some(token) = token {
                self.token = Some(Token::new(token));
            }
            self.ready();
            Event::Authenticated
        } else if text == "auth failed" {
            self.state = SessionState::AuthFailed;
            Event::AuthFailed
        } else if text == "server down" {
            Event::ServerDown
        } else if text.starts_with("cannot subscribe") {
            Event::CannotSubscribe(text.to_owned())
        } else {
            return Err(SocketError::UnrecognizedLine(text.to_owned()));
        };
        Ok(Some(event))
    }

    fn handle_event(&mut self, text: &str) -> Result<Event, SocketError> {
        let mut failed = None;
        let mut deserializer = serde_json::Deserializer::from_str(text);
        let received = FrameSeed {
            kind: self.kind,
            names: &self.names,
            subscriptions: &self.subscriptions,
            failed: &mut failed,
        }
        .deserialize(&mut deserializer)
        .and_then(|received| deserializer.end().map(|()| received))
        .map_err(|error| match failed.take() {
            Some(channel) => SocketError::Payload {
                channel,
                source: error,
            },
            None => SocketError::Json(error),
        })?;
        Ok(match received {
            Received::Event { channel, payload } => Event::Channel { channel, payload },
            Received::Room {
                key,
                channel,
                update,
            } => {
                if let Some(subscription) = self.subscriptions.get_mut(&key) {
                    subscription.room.get_or_insert_default().apply(update);
                }
                Event::Channel {
                    channel,
                    payload: Payload::Room,
                }
            }
            Received::Error { channel, message } => Event::ChannelError { channel, message },
            Received::Unsolicited(channel) => Event::Unsolicited { channel },
        })
    }
}

fn parse_number<T: std::str::FromStr>(line: &str, number: &str) -> Result<T, SocketError> {
    number
        .parse()
        .map_err(|_| SocketError::UnrecognizedLine(line.to_owned()))
}

/// A decoded `[channel, data]` event, before it touches the session.
enum Received {
    Event {
        channel: Channel,
        payload: Payload,
    },
    Room {
        key: u64,
        channel: Channel,
        update: RoomEvent,
    },
    Error {
        channel: Channel,
        message: String,
    },
    Unsolicited(String),
}

/// Decodes `[channel, data]`, choosing the data type by the subscribed channel.
struct FrameSeed<'s> {
    kind: ServerKind,
    names: &'s HashMap<String, u64>,
    subscriptions: &'s BTreeMap<u64, Subscription>,
    /// Set when the data of a subscribed channel fails to decode.
    failed: &'s mut Option<Channel>,
}

impl<'de> DeserializeSeed<'de> for FrameSeed<'_> {
    type Value = Received;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Received, D::Error> {
        deserializer.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for FrameSeed<'_> {
    type Value = Received;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a `[channel, data]` event")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Received, A::Error> {
        let Name(name) = seq
            .next_element()?
            .ok_or_else(|| de::Error::invalid_length(0, &self))?;
        let unprefixed = name.strip_prefix("err@");
        let error = unprefixed.is_some();
        let subscribed = unprefixed.unwrap_or(&name);
        let found = self
            .names
            .get(subscribed)
            .and_then(|key| Some((*key, self.subscriptions.get(key)?)));
        let Some((key, subscription)) = found else {
            seq.next_element::<IgnoredAny>()?
                .ok_or_else(|| de::Error::invalid_length(1, &self))?;
            return Ok(Received::Unsolicited(name.into_owned()));
        };
        let channel = &subscription.channel;
        let data = if error {
            seq.next_element::<String>()
                .map(|message| message.map(Data::Error))
        } else {
            seq.next_element_seed(DataSeed {
                channel,
                kind: self.kind,
            })
        };
        let data = match data {
            Ok(Some(data)) => data,
            Ok(None) => return Err(de::Error::invalid_length(1, &self)),
            Err(source) => {
                *self.failed = Some(channel.clone());
                return Err(source);
            }
        };
        let channel = channel.clone();
        Ok(match data {
            Data::Payload(payload) => Received::Event { channel, payload },
            Data::Room(update) => Received::Room {
                key,
                channel,
                update,
            },
            Data::Error(message) => Received::Error { channel, message },
        })
    }
}

/// A channel name, borrowed from the frame when it holds no escapes.
struct Name<'de>(Cow<'de, str>);

impl<'de> Deserialize<'de> for Name<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NameVisitor;

        impl<'de> Visitor<'de> for NameVisitor {
            type Value = Name<'de>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a channel name")
            }

            fn visit_borrowed_str<E: de::Error>(self, value: &'de str) -> Result<Name<'de>, E> {
                Ok(Name(Cow::Borrowed(value)))
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Name<'de>, E> {
                Ok(Name(Cow::Owned(value.to_owned())))
            }

            fn visit_string<E: de::Error>(self, value: String) -> Result<Name<'de>, E> {
                Ok(Name(Cow::Owned(value)))
            }
        }

        deserializer.deserialize_str(NameVisitor)
    }
}

/// Decoded data of a subscribed channel's event.
enum Data {
    Payload(Payload),
    Room(RoomEvent),
    Error(String),
}

/// Decodes the data of one subscribed channel's event with the contract of `kind`.
struct DataSeed<'c> {
    channel: &'c Channel,
    kind: ServerKind,
}

/// Deserializes `$type` of the socket contract of `$kind` from `$deserializer` and converts it
/// into the domain type: for the payloads whose shape differs between contracts.
macro_rules! by_contract {
    ($kind:expr, $deserializer:expr, $type:ident) => {
        match $kind {
            ServerKind::Official => official::types::$type::deserialize($deserializer)?.into(),
            ServerKind::OpenScreeps => {
                openscreeps::types::$type::deserialize($deserializer)?.into()
            }
            ServerKind::Community => community::types::$type::deserialize($deserializer)?.into(),
        }
    };
}

impl<'de> DeserializeSeed<'de> for DataSeed<'_> {
    type Value = Data;

    /// Payloads every contract defines identically are decoded with the official type, which
    /// the other contracts re-export.
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Data, D::Error> {
        let payload = match self.channel {
            Channel::ServerMessage => Payload::ServerMessage(String::deserialize(deserializer)?),
            Channel::Room { .. } => {
                let event: RoomEvent = by_contract!(self.kind, deserializer, RoomEvent);
                return Ok(Data::Room(event));
            }
            Channel::RoomMap { .. } => Payload::RoomMap(RoomMap::deserialize(deserializer)?),
            Channel::MapVisual { .. } => {
                let text = Option::<String>::deserialize(deserializer)?.unwrap_or_default();
                let items = visual::items::<MapVisualItem>(&text).collect::<Result<_, _>>();
                Payload::MapVisual(items.map_err(de::Error::custom)?)
            }
            Channel::Cpu { .. } => Payload::Cpu(UserCpu::deserialize(deserializer)?),
            Channel::Console { .. } => {
                Payload::Console(by_contract!(self.kind, deserializer, UserConsole))
            }
            Channel::Memory { .. } => Payload::Memory(String::deserialize(deserializer)?),
            Channel::Resources { .. } => {
                Payload::Resources(UserResources::deserialize(deserializer)?)
            }
            Channel::Code { .. } => {
                Payload::Code(official::types::UserCode::deserialize(deserializer)?.into())
            }
            Channel::ActiveBranch { .. } => Payload::ActiveBranch(
                official::types::UserActiveBranch::deserialize(deserializer)?.into(),
            ),
            Channel::NewMessage { .. } | Channel::Conversation { .. } => Payload::Message(
                official::types::UserMessageEvent::deserialize(deserializer)?.message,
            ),
        };
        Ok(Data::Payload(payload))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Event, Payload, SessionState, SocketError, SocketSession};
    use crate::Token;
    use crate::model::console::ConsoleEvent;
    use crate::model::platform::ServerKind;
    use crate::model::socket::{UserCpu, UserCpuCpu};
    use crate::model::world::MapVisualItem;
    use crate::socket::Channel;

    /// The contract of the tests that do not depend on it.
    const KIND: ServerKind = ServerKind::Official;

    const USER: &str = "5a1b";

    fn drain(session: &mut SocketSession) -> Vec<String> {
        std::iter::from_fn(|| session.poll_transmit()).collect()
    }

    fn room(name: &str) -> Channel {
        Channel::Room {
            shard: Some("shard3".into()),
            room: name.into(),
        }
    }

    fn cpu() -> Channel {
        Channel::Cpu { user: USER.into() }
    }

    fn signed_in() -> SocketSession {
        signed_in_to(KIND)
    }

    fn signed_in_to(kind: ServerKind) -> SocketSession {
        let mut session = SocketSession::new(Some(Token::new("tok-1")), kind);
        session.handle_frame("auth ok tok-1").unwrap();
        drain(&mut session);
        session
    }

    #[test]
    fn subscriptions_wait_for_auth_ok_and_adopt_the_rotated_token() {
        let mut session = SocketSession::new(Some(Token::new("tok-1")), KIND);
        assert_eq!(session.state(), SessionState::Authenticating);
        session.subscribe(cpu());
        session.subscribe(room("W8N3"));
        session.subscribe(cpu());
        assert_eq!(drain(&mut session), ["gzip on", "auth tok-1"]);

        let event = session.handle_frame("auth ok tok-2").unwrap();
        assert_eq!(event, Some(Event::Authenticated));
        assert_eq!(session.state(), SessionState::Ready);
        assert_eq!(session.token().map(Token::as_str), Some("tok-2"));
        assert_eq!(
            drain(&mut session),
            ["subscribe user:5a1b/cpu", "subscribe room:shard3/W8N3"]
        );

        session.subscribe(room("W8N3"));
        session.subscribe(Channel::ServerMessage);
        assert_eq!(
            drain(&mut session),
            ["subscribe server-message"],
            "no duplicate subscribe"
        );
        assert_eq!(session.subscriptions().count(), 3);
    }

    #[test]
    fn anonymous_sessions_are_ready_at_once() {
        let mut session = SocketSession::new(None, KIND);
        assert_eq!(session.state(), SessionState::Ready);
        session.subscribe(Channel::ServerMessage);
        assert_eq!(drain(&mut session), ["gzip on", "subscribe server-message"]);
    }

    #[test]
    fn unsubscribe_sends_only_what_was_subscribed() {
        let mut session = SocketSession::new(Some(Token::new("tok-1")), KIND);
        session.subscribe(cpu());
        session.unsubscribe(&cpu());
        session.unsubscribe(&room("W1N1"));
        session.handle_frame("auth ok tok-1").unwrap();
        assert_eq!(drain(&mut session), ["gzip on", "auth tok-1"]);

        session.subscribe(cpu());
        session.unsubscribe(&cpu());
        assert_eq!(
            drain(&mut session),
            ["subscribe user:5a1b/cpu", "unsubscribe user:5a1b/cpu"]
        );
        assert_eq!(session.subscriptions().count(), 0);
    }

    #[test]
    fn auth_failed_sends_nothing_until_authenticated_again() {
        let mut session = SocketSession::new(Some(Token::new("bad")), KIND);
        session.subscribe(cpu());
        drain(&mut session);
        assert_eq!(
            session.handle_frame("auth failed").unwrap(),
            Some(Event::AuthFailed)
        );
        assert_eq!(session.state(), SessionState::AuthFailed);
        session.subscribe(Channel::ServerMessage);
        assert_eq!(drain(&mut session), [] as [String; 0]);

        session.authenticate(Token::new("good"));
        assert_eq!(session.state(), SessionState::Authenticating);
        assert_eq!(drain(&mut session), ["auth good"]);
        session.handle_frame("auth ok good").unwrap();
        assert_eq!(
            drain(&mut session),
            ["subscribe user:5a1b/cpu", "subscribe server-message"]
        );
    }

    #[test]
    fn reconnect_resubscribes_after_the_new_auth_ok() {
        let mut session = signed_in();
        session.subscribe(cpu());
        session.subscribe(room("W8N3"));
        session
            .handle_frame(r#"["room:shard3/W8N3",{"objects":{"a":{"_id":"a"}}}]"#)
            .unwrap();
        assert!(session.room(&room("W8N3")).is_some());
        session.subscribe(Channel::ServerMessage);

        session.reconnect(None);
        assert_eq!(session.state(), SessionState::Authenticating);
        assert!(
            session.room(&room("W8N3")).is_none(),
            "room state is per connection"
        );
        assert_eq!(
            drain(&mut session),
            ["gzip on", "auth tok-1"],
            "queued frames are dropped"
        );
        session.handle_frame("auth ok tok-3").unwrap();
        assert_eq!(
            drain(&mut session),
            [
                "subscribe user:5a1b/cpu",
                "subscribe room:shard3/W8N3",
                "subscribe server-message"
            ]
        );

        session.reconnect(Some(Token::new("tok-4")));
        assert_eq!(drain(&mut session), ["gzip on", "auth tok-4"]);
    }

    #[test]
    fn greeting_and_status_lines() {
        let mut session = SocketSession::new(None, KIND);
        let mut handle = |frame| session.handle_frame(frame).unwrap();
        assert_eq!(
            handle("time 1792035138367"),
            Some(Event::Time(1_792_035_138_367))
        );
        assert_eq!(handle("protocol 14"), Some(Event::Protocol(14)));
        assert_eq!(handle("package 247"), Some(Event::Package(247)));
        assert_eq!(handle("server down"), Some(Event::ServerDown));
        assert_eq!(
            handle("cannot subscribe room:W1N1"),
            Some(Event::CannotSubscribe("cannot subscribe room:W1N1".into()))
        );
        assert_eq!(handle(""), None);
    }

    #[test]
    fn unknown_lines_fail_without_touching_state() {
        let mut session = SocketSession::new(Some(Token::new("tok-1")), KIND);
        session.subscribe(cpu());
        for line in ["hello", "time soon", "auth okay", "auth ok two tokens"] {
            let error = session.handle_frame(line).unwrap_err();
            assert!(
                matches!(error, SocketError::UnrecognizedLine(_)),
                "{line}: {error:?}"
            );
        }
        assert_eq!(session.state(), SessionState::Authenticating);
        assert_eq!(session.token().map(Token::as_str), Some("tok-1"));
        assert_eq!(drain(&mut session), ["gzip on", "auth tok-1"]);
    }

    #[test]
    fn compressed_frames_are_inflated() {
        let mut session = signed_in();
        let channel = room("W8N3");
        session.subscribe(channel.clone());
        session.subscribe(Channel::Console { user: USER.into() });
        // zlib (Python `zlib.compress`) of
        // ["room:shard3/W8N3",{"objects":{"a1":{"_id":"a1","type":"source","energy":3000,
        //  "effects":[{"effect":1001}]}},"users":{"3":{"_id":"3","username":"Source Keeper"}},
        //  "flags":null,"info":{"mode":"world"},"decorations":[]}]
        let frame = "gz:eJxFjcEKwjAQRH9F5hwwJRfJLwhePHgoRWKzqZU2K5sWKSX/LmkFL8swzHtbQ5hHm55OvDneThcDtYIfL2qnBLvCVeXeew9bssK0vAkWiWdpCQoUSboF1mitFSiEnazXX4attK5yk7PCnEg2q/lLDfY+urF4r5v3cCZ6k6BAYXBdgo3zMCj0MXBhR/Zl/WEZPLKCp5bFTT3H8rvJzRcCxUXI";
        assert_eq!(
            session.handle_frame(frame).unwrap(),
            Some(Event::Channel {
                channel: channel.clone(),
                payload: Payload::Room
            })
        );
        let state = session.room(&channel).unwrap();
        assert_eq!(
            state.object("a1").and_then(|object| object.get("effects")),
            Some(&json!({"0": {"effect": 1001}}))
        );
        assert_eq!(
            state.user("3").map(|user| user.username.as_str()),
            Some("Source Keeper")
        );
        assert_eq!(state.mode(), Some("world"));

        // zlib of ["user:5a1b/console",{"messages":{"log":["tick"],"results":[]},"shard":"shard3"}]
        let frame = "gz:eJwlxbEKgCAUBdB/ubMQES3vV8TB7KGRJXh1Ev+9obMci06tsvv1WEJ5WbLCDDxK+qiEDOQSIRbtCjecQVX23Aixbhow+XpC/jdM9wFogxtT";
        let Some(Event::Channel {
            channel: Channel::Console { user },
            payload: Payload::Console(console),
        }) = session.handle_frame(frame).unwrap()
        else {
            panic!("not a console event");
        };
        assert_eq!(user, USER);
        assert_eq!(
            console,
            ConsoleEvent::Output {
                log: vec!["tick".to_owned()],
                results: Vec::new(),
                shard: Some("shard3".to_owned()),
            }
        );
        assert!(matches!(
            session.handle_frame("gz:AAAA"),
            Err(SocketError::Gz(_))
        ));
    }

    #[test]
    fn payloads_decode_with_the_contract_of_the_server() {
        let console = Channel::Console { user: USER.into() };
        let room = Channel::Room {
            shard: None,
            room: "W1N1".into(),
        };
        // Console output names the shard on official servers only.
        let output = r#"["user:5a1b/console",{"messages":{"log":[],"results":["1"]},"shard":"s"}]"#;
        // Only the official contract reads `decorations`; this one does not match it.
        let decorated = r#"["room:W1N1",{"objects":{},"decorations":[{"x":1}],"gameTime":5}]"#;
        for kind in [
            ServerKind::Official,
            ServerKind::OpenScreeps,
            ServerKind::Community,
        ] {
            let mut session = signed_in_to(kind);
            assert_eq!(session.kind(), kind);
            session.subscribe(console.clone());
            session.subscribe(room.clone());
            let official = kind == ServerKind::Official;
            assert_eq!(
                session.handle_frame(output).unwrap(),
                Some(Event::Channel {
                    channel: console.clone(),
                    payload: Payload::Console(ConsoleEvent::Output {
                        log: Vec::new(),
                        results: vec!["1".to_owned()],
                        shard: official.then(|| "s".to_owned()),
                    }),
                }),
                "{kind:?}"
            );
            let decoded = session.handle_frame(decorated);
            if official {
                assert!(
                    matches!(decoded, Err(SocketError::Payload { .. })),
                    "{decoded:?}"
                );
            } else {
                assert!(matches!(decoded, Ok(Some(_))), "{kind:?}: {decoded:?}");
                let state = session.room(&room).unwrap();
                assert_eq!(state.game_time(), Some(5));
                assert_eq!(state.decorations(), []);
            }
        }
    }

    #[test]
    fn room_errors_route_to_the_subscription_which_stays() {
        let mut session = signed_in();
        let channel = Channel::Room {
            shard: None,
            room: "W8N3".into(),
        };
        session.subscribe(channel.clone());
        assert_eq!(
            session
                .handle_frame(r#"["err@room:W8N3","subscribe limit reached"]"#)
                .unwrap(),
            Some(Event::ChannelError {
                channel: channel.clone(),
                message: "subscribe limit reached".into()
            })
        );
        assert_eq!(session.subscriptions().collect::<Vec<_>>(), [&channel]);
        assert!(session.room(&channel).is_none());
    }

    #[test]
    fn events_of_unsubscribed_channels_are_unsolicited() {
        let mut session = signed_in();
        session.subscribe(cpu());
        session.unsubscribe(&cpu());
        for (frame, name) in [
            (r#"["user:5a1b/cpu",{"cpu":3}]"#, "user:5a1b/cpu"),
            (
                r#"["err@room:shard3/W1N1","subscribe limit reached"]"#,
                "err@room:shard3/W1N1",
            ),
            (
                r#"["user:5a1b/steam-purchase",{}]"#,
                "user:5a1b/steam-purchase",
            ),
        ] {
            assert_eq!(
                session.handle_frame(frame).unwrap(),
                Some(Event::Unsolicited {
                    channel: name.into()
                })
            );
        }
    }

    #[test]
    fn channel_payloads_are_typed_and_bad_data_is_a_payload_error() {
        let mut session = signed_in();
        session.subscribe(cpu());
        session.subscribe(Channel::ServerMessage);
        let room = room("W8N3");
        session.subscribe(room.clone());
        assert_eq!(
            session
                .handle_frame(r#"["user:5a1b/cpu",{"cpu":"error","memory":754}]"#)
                .unwrap(),
            Some(Event::Channel {
                channel: cpu(),
                payload: Payload::Cpu(UserCpu {
                    cpu: UserCpuCpu::Error,
                    memory: Some(754)
                }),
            })
        );
        assert_eq!(
            session
                .handle_frame(r#"["server-message","Maintenance at 10:00"]"#)
                .unwrap(),
            Some(Event::Channel {
                channel: Channel::ServerMessage,
                payload: Payload::ServerMessage("Maintenance at 10:00".into()),
            })
        );

        let map_visual = Channel::MapVisual {
            user: USER.into(),
            shard: Some("shard3".into()),
        };
        session.subscribe(map_visual.clone());
        let frame = r#"["mapVisual:5a1b/shard3","{\"t\":\"c\",\"x\":25,\"y\":25,\"n\":\"W8N3\",\"s\":{\"radius\":5}}\n{\"t\":\"t\",\"text\":\"base\",\"x\":25,\"y\":20,\"n\":\"W8N3\",\"s\":{}}\n"]"#;
        let Some(Event::Channel {
            payload: Payload::MapVisual(items),
            ..
        }) = session.handle_frame(frame).unwrap()
        else {
            panic!("not a map visual event");
        };
        assert!(
            matches!(&items[..], [MapVisualItem::Circle(circle), MapVisualItem::Text(text)]
                if circle.n == "W8N3" && text.text == "base"),
            "{items:?}"
        );
        assert_eq!(
            session
                .handle_frame(r#"["mapVisual:5a1b/shard3",null]"#)
                .unwrap(),
            Some(Event::Channel {
                channel: map_visual.clone(),
                payload: Payload::MapVisual(Vec::new()),
            })
        );
        assert!(matches!(
            session.handle_frame(r#"["mapVisual:5a1b/shard3","{\"t\":\"x\"}"]"#),
            Err(SocketError::Payload { .. })
        ));

        session
            .handle_frame(r#"["room:shard3/W8N3",{"objects":{"a":{"_id":"a","energy":1}}}]"#)
            .unwrap();
        let error = session
            .handle_frame(
                r#"["room:shard3/W8N3",{"objects":{"a":{"energy":2}},"gameTime":"soon"}]"#,
            )
            .unwrap_err();
        assert!(matches!(&error, SocketError::Payload { channel, .. } if *channel == room));
        assert_eq!(
            session
                .room(&room)
                .unwrap()
                .object("a")
                .unwrap()
                .get("energy"),
            Some(&json!(1)),
            "a rejected update is not applied"
        );
        assert!(matches!(
            session.handle_frame(r#"["user:5a1b/cpu",{"cpu":true}]"#),
            Err(SocketError::Payload { .. })
        ));
        for frame in [
            "[1,2]",
            r#"["server-message"]"#,
            r#"["server-message","a","b"]"#,
            "[broken",
        ] {
            assert!(
                matches!(session.handle_frame(frame), Err(SocketError::Json(_))),
                "{frame}"
            );
        }
    }
}
