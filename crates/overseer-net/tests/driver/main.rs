//! Driver behaviour against an in-process server: discovery, reconnects, backoff, keep-alive,
//! token sharing, sign-in, rate limits, client flows, the event stream and upgrade retries.

mod account;
mod discovery;
mod flows;
mod server;
mod signin;
mod socket;
mod upgrade;
mod world;

use std::time::Duration;

use overseer_net::{DisconnectReason, ReconnectPolicy, Socket, SocketEvent};
use tokio::time::Instant;

/// `GET /api/version` of a single-world `OpenScreeps` server (the `openscreeps` feature).
const VERSION: &str = r#"{"ok":1,"protocol":14,"users":1,"serverData":{"customObjectTypes":{},"historyChunkSize":20,"renderer":{"resources":{},"metadata":{}},"features":[{"name":"market","version":1},{"name":"auth","version":1},{"name":"openscreeps","version":1}]}}"#;

/// `GET /api/auth/me` of user `u1`.
const ME: &str = r#"{"ok":1,"_id":"u1","money":0,"powerExperimentations":0}"#;

/// Upper bound of every wait in these tests.
const PATIENCE: Duration = Duration::from_secs(10);

/// Reconnects after 20 ms, doubling up to 50 ms.
fn fast_reconnect(max_attempts: Option<u32>) -> ReconnectPolicy {
    ReconnectPolicy {
        initial_delay: Duration::from_millis(20),
        max_delay: Duration::from_millis(50),
        max_attempts,
    }
}

/// A comparable description of an event.
fn describe(event: &SocketEvent) -> String {
    match event {
        SocketEvent::Connected => "connected".to_owned(),
        SocketEvent::Disconnected(reason) => match reason {
            DisconnectReason::Closed { .. } => "closed",
            DisconnectReason::Ended => "ended",
            DisconnectReason::Transport(_) => "transport failed",
            DisconnectReason::KeepAliveTimeout => "keep-alive timeout",
            DisconnectReason::ConnectFailed(_) => "connect failed",
        }
        .to_owned(),
        SocketEvent::Reconnecting { attempt, delay } => {
            format!("reconnecting {attempt} after {}ms", delay.as_millis())
        }
        SocketEvent::Event(event) => format!("{event:?}"),
    }
}

/// Events with their arrival times, up to and including the first one `last` accepts, or up to
/// the end of the socket.
async fn events_until(
    socket: &mut Socket,
    mut last: impl FnMut(&SocketEvent) -> bool,
) -> Vec<(Instant, String)> {
    let mut events = Vec::new();
    loop {
        let next = tokio::time::timeout(PATIENCE, socket.next())
            .await
            .expect("an event in time")
            .unwrap();
        let Some(event) = next else {
            return events;
        };
        events.push((Instant::now(), describe(&event)));
        if last(&event) {
            return events;
        }
    }
}

/// The descriptions of [`events_until`].
async fn descriptions_until(
    socket: &mut Socket,
    last: impl FnMut(&SocketEvent) -> bool,
) -> Vec<String> {
    events_until(socket, last)
        .await
        .into_iter()
        .map(|(_, event)| event)
        .collect()
}
