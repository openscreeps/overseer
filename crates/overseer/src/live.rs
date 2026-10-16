//! Live data: reference-counted socket subscriptions.
//!
//! A [`Watch`] holds the latest value of a state channel (a room, a Memory path, CPU usage); a
//! [`Feed`] delivers every item of an event channel (console output, new messages). Each handle
//! is one reference to its socket [`Channel`]: the first reference subscribes, cloning a handle
//! adds a reference, and dropping the last one unsubscribes. The background task resubscribes
//! every referenced channel after a reconnect. Both handles are [`Stream`]s and end with
//! [`Closed`] once the session shuts down.

use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, ready};

use futures_util::Stream;
use overseer_net::core::socket::{Channel, Payload};
use tokio::sync::{broadcast, mpsc, watch};

use crate::Notice;

/// Items a [`Feed`] buffers for a slow reader; older items are skipped and reported with
/// [`Notice::Lagged`].
pub const FEED_CAPACITY: usize = 256;

/// The session is closed: no further value will arrive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the session is closed")]
pub struct Closed;

/// The latest value of a state channel.
///
/// Created by the feature handles (for example [`Rooms::watch`](crate::Rooms::watch)). Values
/// are shared ([`Arc`]) between every handle of the channel and built once per update.
/// [`Self::changed`] (and the [`Stream`]) returns each value this handle has not seen yet: a new
/// handle on a channel that already has a value returns it at once, and a reader that falls
/// behind skips to the latest value.
pub struct Watch<T> {
    receiver: watch::Receiver<Option<Arc<T>>>,
    /// Waits for the next change on a clone of `receiver`; behind a mutex only to make the
    /// handle `Sync` (it is only accessed through `&mut self`).
    pending: Mutex<Option<Pin<Box<dyn Future<Output = bool> + Send>>>>,
    /// Dropped last: the registry prunes delivery targets without receivers when it is released.
    registration: Registration,
}

impl<T: Send + Sync + 'static> Watch<T> {
    pub(crate) fn new(
        receiver: watch::Receiver<Option<Arc<T>>>,
        registration: Registration,
    ) -> Self {
        Self {
            receiver,
            pending: Mutex::new(None),
            registration,
        }
    }

    /// The watched channel.
    #[must_use]
    pub fn channel(&self) -> &Channel {
        &self.registration.channel
    }

    /// The latest value; `None` until the first one arrived.
    #[must_use]
    pub fn latest(&self) -> Option<Arc<T>> {
        self.receiver.borrow().clone()
    }

    /// Waits for a value this handle has not returned yet and returns it (see [`Watch`]).
    /// Cancel-safe.
    ///
    /// # Errors
    ///
    /// Returns [`Closed`] once the session is shut down.
    pub async fn changed(&mut self) -> Result<Arc<T>, Closed> {
        poll_fn(|cx| self.poll_changed(cx)).await
    }

    fn poll_changed(&mut self, cx: &mut Context<'_>) -> Poll<Result<Arc<T>, Closed>> {
        let pending = self
            .pending
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(waiting) = pending {
                let open = ready!(waiting.as_mut().poll(cx));
                *pending = None;
                if !open {
                    return Poll::Ready(Err(Closed));
                }
            }
            match self.receiver.has_changed() {
                Err(_) => return Poll::Ready(Err(Closed)),
                Ok(true) => {
                    if let Some(value) = self.receiver.borrow_and_update().clone() {
                        return Poll::Ready(Ok(value));
                    }
                }
                Ok(false) => {
                    let mut receiver = self.receiver.clone();
                    *pending = Some(Box::pin(async move { receiver.changed().await.is_ok() }));
                }
            }
        }
    }
}

impl<T> Clone for Watch<T> {
    /// Another handle on the same channel, adding a reference; it has seen what this one has.
    fn clone(&self) -> Self {
        Self {
            receiver: self.receiver.clone(),
            pending: Mutex::new(None),
            registration: self.registration.clone(),
        }
    }
}

impl<T: Send + Sync + 'static> Stream for Watch<T> {
    type Item = Arc<T>;

    /// The values of [`Watch::changed`]; ends when the session is closed.
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().poll_changed(cx).map(Result::ok)
    }
}

impl<T> fmt::Debug for Watch<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Watch")
            .field("channel", &self.registration.channel)
            .finish_non_exhaustive()
    }
}

/// Every item of an event channel, in order.
///
/// Created by the feature handles (for example [`Console::feed`](crate::Console::feed)). Each
/// handle receives the items that arrive while it exists; up to [`FEED_CAPACITY`] items wait for
/// a slow reader, older ones are skipped and reported with [`Notice::Lagged`].
pub struct Feed<T> {
    receiving: Mutex<Receiving<T>>,
    /// Creates the receivers of clones without keeping the channel open.
    sender: broadcast::WeakSender<T>,
    /// Dropped last: the registry prunes delivery targets without receivers when it is released.
    registration: Registration,
}

/// The receiver of a [`Feed`], or the wait for its next item that owns it.
enum Receiving<T> {
    Idle(broadcast::Receiver<T>),
    Waiting(Pin<Box<dyn Future<Output = Received<T>> + Send>>),
}

type Received<T> = (
    Result<T, broadcast::error::RecvError>,
    broadcast::Receiver<T>,
);

impl<T: Clone + Send + 'static> Feed<T> {
    pub(crate) fn new(
        receiver: broadcast::Receiver<T>,
        sender: broadcast::WeakSender<T>,
        registration: Registration,
    ) -> Self {
        Self {
            receiving: Mutex::new(Receiving::Idle(receiver)),
            sender,
            registration,
        }
    }

    /// The channel.
    #[must_use]
    pub fn channel(&self) -> &Channel {
        &self.registration.channel
    }

    /// The next item. Items lost because this handle fell more than [`FEED_CAPACITY`] items
    /// behind are skipped and their count is reported with [`Notice::Lagged`]. Cancel-safe.
    ///
    /// # Errors
    ///
    /// Returns [`Closed`] once the session is shut down and every buffered item was read.
    pub async fn recv(&mut self) -> Result<T, Closed> {
        poll_fn(|cx| self.poll_recv(cx)).await
    }

    fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Result<T, Closed>> {
        use broadcast::error::{RecvError, TryRecvError};

        let receiving = self
            .receiving
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        loop {
            let outcome = match receiving {
                Receiving::Idle(receiver) => match receiver.try_recv() {
                    Ok(item) => return Poll::Ready(Ok(item)),
                    Err(TryRecvError::Lagged(skipped)) => Err(RecvError::Lagged(skipped)),
                    Err(TryRecvError::Closed) => return Poll::Ready(Err(Closed)),
                    Err(TryRecvError::Empty) => {
                        // Only an empty buffer needs a boxed wait; the placeholder is a
                        // zero-sized future and does not allocate.
                        let placeholder = Receiving::Waiting(Box::pin(std::future::pending()));
                        if let Receiving::Idle(mut receiver) =
                            std::mem::replace(receiving, placeholder)
                        {
                            *receiving = Receiving::Waiting(Box::pin(async move {
                                let outcome = receiver.recv().await;
                                (outcome, receiver)
                            }));
                        }
                        continue;
                    }
                },
                Receiving::Waiting(waiting) => {
                    let (outcome, receiver) = ready!(waiting.as_mut().poll(cx));
                    *receiving = Receiving::Idle(receiver);
                    outcome
                }
            };
            match outcome {
                Ok(item) => return Poll::Ready(Ok(item)),
                Err(RecvError::Lagged(skipped)) => {
                    self.registration.registry.notify(Notice::Lagged {
                        channel: self.registration.channel.clone(),
                        skipped,
                    });
                }
                Err(RecvError::Closed) => return Poll::Ready(Err(Closed)),
            }
        }
    }
}

impl<T: Clone + Send + 'static> Clone for Feed<T> {
    /// Another handle on the same channel, adding a reference; it receives the items that arrive
    /// from now on.
    fn clone(&self) -> Self {
        let receiver = self
            .sender
            .upgrade()
            .map_or_else(|| broadcast::channel(1).1, |sender| sender.subscribe());
        Self::new(receiver, self.sender.clone(), self.registration.clone())
    }
}

impl<T: Clone + Send + 'static> Stream for Feed<T> {
    type Item = T;

    /// The items of [`Feed::recv`]; ends when the session is closed.
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().poll_recv(cx).map(Result::ok)
    }
}

impl<T> fmt::Debug for Feed<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Feed")
            .field("channel", &self.registration.channel)
            .finish_non_exhaustive()
    }
}

/// What the background task is asked to do.
#[derive(Debug)]
pub(crate) enum Command {
    /// The first reference to a channel was taken.
    Subscribe(Channel),
    /// The last reference to a channel was released.
    Unsubscribe(Channel),
    /// A room channel's sink got its terrain: publish the snapshot of the room's state (and a
    /// refusal) received meanwhile.
    Republish(Channel),
    /// Close the socket and end the task.
    Shutdown,
}

/// Where the data of one channel goes: the senders behind the handles of a channel.
pub(crate) trait Sink: Any + Send + Sync {
    /// Delivers one event of the channel.
    fn deliver(&mut self, payload: &Payload);

    /// Whether no handle receives from this sink any more.
    fn is_unused(&self) -> bool;
}

/// A [`Watch`]'s sender and the conversion of the channel's payloads.
struct WatchSink<T> {
    sender: watch::Sender<Option<Arc<T>>>,
    extract: fn(Payload) -> Option<T>,
}

impl<T: Send + Sync + 'static> Sink for WatchSink<T> {
    fn deliver(&mut self, payload: &Payload) {
        if let Some(value) = (self.extract)(payload.clone()) {
            self.sender.send_replace(Some(Arc::new(value)));
        }
    }

    fn is_unused(&self) -> bool {
        self.sender.receiver_count() == 0
    }
}

/// A [`Feed`]'s sender and the conversion of the channel's payloads.
struct FeedSink<T> {
    sender: broadcast::Sender<T>,
    extract: fn(Payload) -> Option<T>,
}

impl<T: Clone + Send + 'static> Sink for FeedSink<T> {
    fn deliver(&mut self, payload: &Payload) {
        if let Some(item) = (self.extract)(payload.clone()) {
            // Fails only without receivers; the sink is pruned then.
            let _ = self.sender.send(item);
        }
    }

    fn is_unused(&self) -> bool {
        self.sender.receiver_count() == 0
    }
}

/// The referenced channels and their sinks, shared by the handles and the background task.
///
/// Locked only for bookkeeping and sends, never across an `.await`.
pub(crate) struct Registry {
    state: Mutex<State>,
    commands: mpsc::UnboundedSender<Command>,
    notices: broadcast::Sender<Notice>,
}

struct State {
    /// Set by shutdown: no channel can be referenced any more.
    closed: bool,
    entries: HashMap<Channel, Entry>,
}

/// One referenced channel.
pub(crate) struct Entry {
    /// Live handles (and the session's own reference to `server-message`).
    refs: usize,
    /// The latest payload, seeding new [`Watch`]es.
    last: Option<Payload>,
    /// Delivery targets.
    pub(crate) sinks: Vec<Box<dyn Sink>>,
}

impl Entry {
    /// The first sink of type `S`.
    pub(crate) fn sink<S: Sink>(&mut self) -> Option<&mut S> {
        self.sinks
            .iter_mut()
            .find_map(|sink| (&mut **sink as &mut dyn Any).downcast_mut::<S>())
    }
}

/// Notices kept for slow readers of [`Overseer::notices`](crate::Overseer::notices).
const NOTICE_CAPACITY: usize = 256;

impl Registry {
    /// A registry whose subscription commands go to `commands`. The session keeps its own
    /// reference to `server-message` (announcements become [`Notice::ServerMessage`]).
    pub(crate) fn new(commands: mpsc::UnboundedSender<Command>) -> Self {
        let mut entries = HashMap::new();
        entries.insert(
            Channel::ServerMessage,
            Entry {
                refs: 1,
                last: None,
                sinks: Vec::new(),
            },
        );
        let _ = commands.send(Command::Subscribe(Channel::ServerMessage));
        Self {
            state: Mutex::new(State {
                closed: false,
                entries,
            }),
            commands,
            notices: broadcast::channel(NOTICE_CAPACITY).0,
        }
    }

    /// Takes a reference to `channel`, subscribing on the first, and runs `attach` on its entry
    /// to add or find a sink. `None` when the registry is closed (the reference is inert then).
    pub(crate) fn acquire<R>(
        self: &Arc<Self>,
        channel: Channel,
        attach: impl FnOnce(&mut Entry) -> R,
    ) -> (Registration, Option<R>) {
        let attached = {
            let mut state = self.lock();
            if state.closed {
                None
            } else {
                let entry = state.entries.entry(channel.clone()).or_insert_with(|| {
                    // Sent under the lock, so subscriptions and releases keep their order.
                    let _ = self.commands.send(Command::Subscribe(channel.clone()));
                    Entry {
                        refs: 0,
                        last: None,
                        sinks: Vec::new(),
                    }
                });
                entry.refs += 1;
                Some(attach(entry))
            }
        };
        let registration = Registration {
            registry: Arc::clone(self),
            channel,
        };
        (registration, attached)
    }

    /// Runs `f` on the entry of a referenced channel.
    pub(crate) fn with_entry<R>(
        &self,
        channel: &Channel,
        f: impl FnOnce(&mut Entry) -> R,
    ) -> Option<R> {
        self.lock().entries.get_mut(channel).map(f)
    }

    /// Delivers an event of `channel` to its sinks.
    pub(crate) fn deliver(&self, channel: &Channel, payload: Payload) {
        if let Some(entry) = self.lock().entries.get_mut(channel) {
            for sink in &mut entry.sinks {
                sink.deliver(&payload);
            }
            entry.last = Some(payload);
        }
    }

    /// Publishes a notice to every [`Overseer::notices`](crate::Overseer::notices) receiver.
    pub(crate) fn notify(&self, notice: Notice) {
        // Fails only when nobody listens.
        let _ = self.notices.send(notice);
    }

    /// A receiver of the notices published from now on.
    pub(crate) fn notices(&self) -> broadcast::Receiver<Notice> {
        self.notices.subscribe()
    }

    /// Asks the background task to do something; ignored once it ended.
    pub(crate) fn command(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    /// Whether the session is closed.
    pub(crate) fn is_closed(&self) -> bool {
        self.lock().closed
    }

    /// Closes the registry: drops every sink, so every handle ends with [`Closed`].
    pub(crate) fn close(&self) {
        let entries = {
            let mut state = self.lock();
            state.closed = true;
            std::mem::take(&mut state.entries)
        };
        // Sinks may run code on drop; not under the lock.
        drop(entries);
    }

    fn add_ref(&self, channel: &Channel) {
        if let Some(entry) = self.lock().entries.get_mut(channel) {
            entry.refs += 1;
        }
    }

    fn release(&self, channel: &Channel) {
        let removed = {
            let mut state = self.lock();
            let Some(entry) = state.entries.get_mut(channel) else {
                return;
            };
            entry.refs -= 1;
            if entry.refs == 0 {
                let _ = self.commands.send(Command::Unsubscribe(channel.clone()));
                state.entries.remove(channel)
            } else {
                entry.sinks.retain(|sink| !sink.is_unused());
                None
            }
        };
        drop(removed);
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // Critical sections only move values in and out of maps and senders.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One reference to a channel; cloning adds one and dropping releases it.
pub(crate) struct Registration {
    registry: Arc<Registry>,
    channel: Channel,
}

impl Clone for Registration {
    fn clone(&self) -> Self {
        self.registry.add_ref(&self.channel);
        Self {
            registry: Arc::clone(&self.registry),
            channel: self.channel.clone(),
        }
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.registry.release(&self.channel);
    }
}

/// A [`Watch`] of `channel` fed by `extract`, seeded with the channel's latest payload.
pub(crate) fn watch<T: Send + Sync + 'static>(
    registry: &Arc<Registry>,
    channel: Channel,
    extract: fn(Payload) -> Option<T>,
) -> Watch<T> {
    let (registration, receiver) = registry.acquire(channel, |entry| {
        let seed = entry.last.clone().and_then(extract).map(Arc::new);
        let seeded = seed.is_some();
        let (sender, mut receiver) = watch::channel(seed);
        if seeded {
            receiver.mark_changed();
        }
        entry.sinks.push(Box::new(WatchSink { sender, extract }));
        receiver
    });
    let receiver = receiver.unwrap_or_else(|| watch::channel(None).1);
    Watch::new(receiver, registration)
}

/// A [`Feed`] of `channel` fed by `extract`.
pub(crate) fn feed<T: Clone + Send + 'static>(
    registry: &Arc<Registry>,
    channel: Channel,
    extract: fn(Payload) -> Option<T>,
) -> Feed<T> {
    let (registration, attached) = registry.acquire(channel, |entry| {
        let (sender, receiver) = broadcast::channel(FEED_CAPACITY);
        let weak = sender.downgrade();
        entry.sinks.push(Box::new(FeedSink { sender, extract }));
        (receiver, weak)
    });
    let (receiver, sender) = attached.unwrap_or_else(|| {
        let (sender, receiver) = broadcast::channel(1);
        (receiver, sender.downgrade())
    });
    Feed::new(receiver, sender, registration)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures_util::StreamExt;
    use overseer_net::core::socket::{Channel, Payload};
    use tokio::sync::mpsc;

    use super::{Closed, Command, FEED_CAPACITY, Registry, feed, watch};
    use crate::Notice;

    /// No command.
    const NOTHING: [String; 0] = [];

    fn registry() -> (Arc<Registry>, mpsc::UnboundedReceiver<Command>) {
        let (commands, mut receiver) = mpsc::unbounded_channel();
        let registry = Arc::new(Registry::new(commands));
        assert_eq!(sent(&mut receiver), ["subscribe server-message"]);
        (registry, receiver)
    }

    /// The commands sent so far.
    fn sent(receiver: &mut mpsc::UnboundedReceiver<Command>) -> Vec<String> {
        std::iter::from_fn(|| receiver.try_recv().ok())
            .map(|command| match command {
                Command::Subscribe(channel) => format!("subscribe {channel}"),
                Command::Unsubscribe(channel) => format!("unsubscribe {channel}"),
                Command::Republish(channel) => format!("republish {channel}"),
                Command::Shutdown => "shutdown".to_owned(),
            })
            .collect()
    }

    fn memory(path: &str) -> Channel {
        Channel::Memory {
            user: "u1".to_owned(),
            shard: None,
            path: path.to_owned(),
        }
    }

    fn value(payload: Payload) -> Option<String> {
        match payload {
            Payload::Memory(value) => Some(value),
            _ => None,
        }
    }

    #[tokio::test]
    async fn watches_share_one_reference_counted_subscription_and_the_latest_value() {
        let (registry, mut commands) = registry();
        let mut first = watch(&registry, memory("a"), value);
        assert_eq!(sent(&mut commands), ["subscribe user:u1/memory/a"]);
        assert_eq!(first.latest(), None);

        registry.deliver(&memory("a"), Payload::Memory("1".to_owned()));
        assert_eq!(*first.changed().await.unwrap(), "1");
        // A second watch starts with the latest value and sends nothing.
        let mut second = watch(&registry, memory("a"), value);
        assert_eq!(second.latest().as_deref().map(String::as_str), Some("1"));
        assert_eq!(*second.changed().await.unwrap(), "1");
        let mut clone = first.clone();
        assert_eq!(sent(&mut commands), NOTHING);

        registry.deliver(&memory("a"), Payload::Memory("2".to_owned()));
        registry.deliver(&memory("a"), Payload::Memory("3".to_owned()));
        // A reader that fell behind skips to the latest value; the stream yields the same.
        assert_eq!(*first.changed().await.unwrap(), "3");
        assert_eq!(*StreamExt::next(&mut clone).await.unwrap(), "3");

        drop(first);
        drop(second);
        assert_eq!(sent(&mut commands), NOTHING);
        drop(clone);
        assert_eq!(sent(&mut commands), ["unsubscribe user:u1/memory/a"]);
        // The permanent `server-message` reference survives a handle's release.
        drop(watch(&registry, Channel::ServerMessage, |_| Some(())));
        assert_eq!(sent(&mut commands), NOTHING);
    }

    #[tokio::test]
    async fn feeds_skip_lost_items_with_a_notice_and_end_when_closed() {
        let (registry, mut commands) = registry();
        let mut notices = registry.notices();
        let mut items = feed(&registry, memory("log"), value);
        let other = items.clone();
        assert_eq!(sent(&mut commands), ["subscribe user:u1/memory/log"]);
        for index in 0..FEED_CAPACITY + 3 {
            registry.deliver(&memory("log"), Payload::Memory(index.to_string()));
        }
        assert_eq!(items.recv().await.unwrap(), "3");
        assert_eq!(
            notices.try_recv().unwrap(),
            Notice::Lagged {
                channel: memory("log"),
                skipped: 3
            }
        );
        assert_eq!(items.recv().await.unwrap(), "4");
        drop(other);

        // A clone receives what arrives after it was made; a cancelled wait loses nothing.
        let mut late = items.clone();
        let waiting = tokio::time::timeout(std::time::Duration::from_millis(10), late.recv());
        assert!(waiting.await.is_err());
        registry.deliver(&memory("log"), Payload::Memory("late".to_owned()));
        assert_eq!(late.recv().await.unwrap(), "late");
        drop(late);
        assert_eq!(sent(&mut commands), NOTHING);

        registry.close();
        let rest: Vec<_> = StreamExt::collect(&mut items).await;
        assert_eq!(rest.len(), FEED_CAPACITY - 1);
        assert_eq!(rest.last().map(String::as_str), Some("late"));
        assert_eq!(items.recv().await, Err(Closed));
        // Handles of a closed registry are closed at once and release nothing.
        let mut closed = watch(&registry, memory("b"), value);
        assert_eq!(closed.changed().await, Err(Closed));
        drop(closed);
        drop(items);
        assert_eq!(sent(&mut commands), NOTHING);
    }
}
