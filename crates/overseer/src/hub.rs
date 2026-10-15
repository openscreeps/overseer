//! The background task: owns the socket, applies subscription commands and delivers events.

use overseer_net::core::socket::{Channel, Event, Payload};
use overseer_net::rt::{self, JoinHandle};
use overseer_net::{Socket, SocketEvent};
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

use crate::live::{Command, Registry};
use crate::rooms::{RoomSink, RoomSnapshot};
use crate::{ConnectionStatus, Notice};

/// Spawns the task driving `socket` until shutdown or until the socket gives up reconnecting:
/// a tokio task natively, a task on the browser's event loop in WebAssembly.
pub(crate) fn spawn(
    socket: Socket,
    registry: Arc<Registry>,
    commands: mpsc::UnboundedReceiver<Command>,
    status: watch::Sender<ConnectionStatus>,
) -> JoinHandle<()> {
    rt::spawn(
        Hub {
            socket,
            registry,
            commands,
            status,
        }
        .run(),
    )
}

struct Hub {
    socket: Socket,
    registry: Arc<Registry>,
    commands: mpsc::UnboundedReceiver<Command>,
    status: watch::Sender<ConnectionStatus>,
}

impl Hub {
    async fn run(mut self) {
        let shutdown = loop {
            tokio::select! {
                // Subscriptions go out before more events are read.
                biased;
                command = self.commands.recv() => {
                    if !self.apply(command) {
                        break true;
                    }
                    // A send failure resurfaces as a disconnection from `next`.
                    let _ = self.socket.flush().await;
                }
                event = self.socket.next() => match event {
                    Ok(Some(event)) => self.handle(event),
                    Ok(None) => break false,
                    Err(overseer_net::Error::Socket(error)) => {
                        self.registry.notify(Notice::Undecodable(describe(&error)));
                    }
                    // The only other errors are failed sign-ins after `auth failed`.
                    Err(_) => self.registry.notify(Notice::AuthFailed),
                },
            }
        };
        self.registry.close();
        self.status.send_replace(ConnectionStatus::Closed);
        if shutdown {
            // The session is going away; a failed close handshake changes nothing.
            let _ = self.socket.close().await;
        }
    }

    /// Applies `command` and every command queued behind it; `false` on shutdown.
    fn apply(&mut self, mut command: Option<Command>) -> bool {
        loop {
            match command {
                Some(Command::Subscribe(channel)) => {
                    self.socket.subscribe(channel);
                }
                Some(Command::Unsubscribe(channel)) => self.socket.unsubscribe(&channel),
                Some(Command::Shutdown) | None => return false,
            }
            match self.commands.try_recv() {
                Ok(next) => command = Some(next),
                Err(mpsc::error::TryRecvError::Empty) => return true,
                Err(mpsc::error::TryRecvError::Disconnected) => return false,
            }
        }
    }

    fn handle(&self, event: SocketEvent) {
        match event {
            SocketEvent::Connected => {
                self.status.send_replace(ConnectionStatus::Connected);
            }
            SocketEvent::Reconnecting { attempt, delay } => {
                self.status
                    .send_replace(ConnectionStatus::Reconnecting { attempt, delay });
            }
            // `Reconnecting` (or the end of the socket) follows at once.
            SocketEvent::Disconnected(_) => {}
            SocketEvent::Event(event) => self.handle_event(event),
        }
    }

    fn handle_event(&self, event: Event) {
        match event {
            Event::Channel {
                channel,
                payload: Payload::Room,
            } => self.publish_room(&channel),
            Event::Channel { channel, payload } => {
                if let Payload::ServerMessage(message) = &payload {
                    self.registry.notify(Notice::ServerMessage(message.clone()));
                }
                self.registry.deliver(&channel, payload);
            }
            Event::ChannelError { channel, message } => {
                if matches!(channel, Channel::Room { .. }) {
                    self.registry.with_entry(&channel, |entry| {
                        if let Some(sink) = entry.sink::<RoomSink>() {
                            sink.refuse(&channel, message.clone());
                        }
                    });
                }
                self.registry
                    .notify(Notice::ChannelError { channel, message });
            }
            Event::ServerDown => self.registry.notify(Notice::ServerDown),
            Event::AuthFailed => self.registry.notify(Notice::AuthFailed),
            Event::CannotSubscribe(line) => self.registry.notify(Notice::ServerMessage(line)),
            Event::Time(_)
            | Event::Protocol(_)
            | Event::Package(_)
            | Event::Authenticated
            | Event::Unsolicited { .. } => {}
        }
    }

    /// Builds the snapshot of an updated room once and publishes it to the room's watches.
    fn publish_room(&self, channel: &Channel) {
        let Channel::Room { shard, room } = channel else {
            return;
        };
        let Some(state) = self.socket.session().room(channel) else {
            return;
        };
        let Some(terrain) = self
            .registry
            .with_entry(channel, |entry| {
                entry.sink::<RoomSink>().map(|sink| sink.terrain())
            })
            .flatten()
        else {
            return;
        };
        let (snapshot, flag_error) = RoomSnapshot::build(room, shard.as_deref(), terrain, state);
        if let Some(error) = flag_error {
            self.registry.notify(Notice::Undecodable(format!(
                "flags of room {room}: {}",
                describe(&error)
            )));
        }
        self.registry.with_entry(channel, |entry| {
            if let Some(sink) = entry.sink::<RoomSink>() {
                sink.publish(snapshot);
            }
        });
    }
}

/// An error and its sources, `: `-separated.
fn describe(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}
