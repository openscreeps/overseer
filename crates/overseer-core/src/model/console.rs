//! Console: expressions sent to the player's runtime and the console output, results and
//! errors of the socket's console channel.
//!
//! The contracts differ only in the console event: screeps.com names the shard that produced
//! it, single-world servers (`OpenScreeps`, the Screeps server) do not. `POST /api/user/console`
//! answers differently too (screeps.com echoes the queued document, the others send `{ok: 1}`),
//! but the reply carries nothing Overseer reads: the output arrives on the socket.

use crate::socket::{community, official, openscreeps};

/// One `user:<user>/console` event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConsoleEvent {
    /// The console output of a run (sent every run, also with empty lists).
    Output {
        /// `console.log` output, as HTML.
        log: Vec<String>,
        /// Results of console expressions (`POST /api/user/console`), as HTML.
        results: Vec<String>,
        /// The shard that ran (screeps.com); `None` on single-world servers.
        shard: Option<String>,
    },
    /// The error that ended a run.
    Error {
        /// The error text, as HTML.
        error: String,
        /// The shard that ran (screeps.com); `None` on single-world servers.
        shard: Option<String>,
    },
}

/// `From` the official console event, which names the shard.
impl From<official::types::UserConsole> for ConsoleEvent {
    fn from(event: official::types::UserConsole) -> Self {
        match event {
            official::types::UserConsole::Output(output) => Self::Output {
                log: output.messages.log,
                results: output.messages.results,
                shard: Some(output.shard),
            },
            official::types::UserConsole::Error(error) => Self::Error {
                error: error.error,
                shard: error.shard,
            },
        }
    }
}

/// `From` the console event of the single-world contracts, which name no shard.
macro_rules! console_event_from {
    ($($contract:ident),+) => {$(
        impl From<$contract::types::UserConsole> for ConsoleEvent {
            fn from(event: $contract::types::UserConsole) -> Self {
                match event {
                    $contract::types::UserConsole::Output(output) => Self::Output {
                        log: output.messages.log,
                        results: output.messages.results,
                        shard: None,
                    },
                    $contract::types::UserConsole::Error(error) => Self::Error {
                        error: error.error,
                        shard: None,
                    },
                }
            }
        }
    )+};
}

console_event_from!(openscreeps, community);

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn output(log: &[&str], results: &[&str], shard: Option<&str>) -> ConsoleEvent {
        ConsoleEvent::Output {
            log: log.iter().map(|line| (*line).to_owned()).collect(),
            results: results.iter().map(|line| (*line).to_owned()).collect(),
            shard: shard.map(str::to_owned),
        }
    }

    #[test]
    fn official_events_name_the_shard() {
        let event: official::types::UserConsole = serde_json::from_value(json!({
            "messages": { "log": ["tick 5"], "results": ["42"] },
            "shard": "shard3",
        }))
        .unwrap();
        assert_eq!(
            ConsoleEvent::from(event),
            output(&["tick 5"], &["42"], Some("shard3"))
        );
        let event: official::types::UserConsole = serde_json::from_value(json!({
            "error": "Error: boom\n    at main:1:7",
            "shard": "shard3",
        }))
        .unwrap();
        assert_eq!(
            ConsoleEvent::from(event),
            ConsoleEvent::Error {
                error: "Error: boom\n    at main:1:7".to_owned(),
                shard: Some("shard3".to_owned()),
            }
        );
    }

    #[test]
    fn single_world_events_name_no_shard() {
        let reply = json!({ "messages": { "log": [], "results": ["undefined"] } });
        let event: openscreeps::types::UserConsole = serde_json::from_value(reply.clone()).unwrap();
        assert_eq!(ConsoleEvent::from(event), output(&[], &["undefined"], None));
        let event: community::types::UserConsole = serde_json::from_value(reply).unwrap();
        assert_eq!(ConsoleEvent::from(event), output(&[], &["undefined"], None));

        let error = json!({ "error": "Script execution timed out" });
        let event: openscreeps::types::UserConsole = serde_json::from_value(error.clone()).unwrap();
        assert_eq!(
            ConsoleEvent::from(event),
            ConsoleEvent::Error {
                error: "Script execution timed out".to_owned(),
                shard: None,
            }
        );
        let event: community::types::UserConsole = serde_json::from_value(error).unwrap();
        assert!(matches!(
            ConsoleEvent::from(event),
            ConsoleEvent::Error { shard: None, .. }
        ));
    }
}
