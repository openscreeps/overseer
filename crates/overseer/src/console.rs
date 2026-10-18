//! The live console: output of the signed-in user's runtime and console commands, and the
//! names of the user's game for completion ([`Console::names`]).

mod names;

pub(crate) use names::NamesCache;
pub use names::{COMPLETION_FRESHNESS, GameNames, NAMES_MAX_ROOMS, NAMES_PARALLEL};

use std::collections::VecDeque;

use overseer_net::api::ConsoleApi as _;
use overseer_net::core::socket::{Channel, Payload};
use overseer_net::model::console::ConsoleEvent;
use overseer_net::rt::SystemTime;
use tokio::sync::broadcast;

use crate::live::{FEED_CAPACITY, Feed, Sink};
use crate::{Error, Overseer};

/// Entries [`Console::history`] keeps.
pub const HISTORY_CAPACITY: usize = 1000;

/// Commands [`Console::recent_commands`] keeps.
pub const RECENT_COMMANDS: usize = 100;

/// What a console line is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConsoleKind {
    /// `console.log` output of a run.
    Log,
    /// The result of a console command.
    Result,
    /// The error that ended a run.
    Error,
    /// A command sent with [`Console::execute`].
    Command,
}

/// One console line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsoleEntry {
    /// What the line is.
    pub kind: ConsoleKind,
    /// The shard that produced it (official server); `None` on single-world servers.
    pub shard: Option<String>,
    /// The text as sent: console output is HTML (see [`Self::plain_text`]); commands are the
    /// expression.
    pub text: String,
    /// When the line was received (or the command sent): [`std::time::SystemTime`] natively,
    /// the browser clock's equivalent in WebAssembly ([`overseer_net::rt::SystemTime`]).
    pub received: SystemTime,
}

impl ConsoleEntry {
    /// The text without HTML tags and with character references decoded: the named
    /// `&amp; &lt; &gt; &quot; &apos; &nbsp;` and numeric `&#NN;` / `&#xNN;` forms the server's
    /// HTML escaping produces. Unknown references are kept as written.
    #[must_use]
    pub fn plain_text(&self) -> String {
        if self.kind == ConsoleKind::Command {
            return self.text.clone();
        }
        decode_entities(&strip_tags(&self.text))
    }
}

/// The live console of the signed-in user; created by [`Overseer::console`].
#[derive(Clone, Debug)]
pub struct Console {
    overseer: Overseer,
}

impl Console {
    pub(crate) fn new(overseer: Overseer) -> Self {
        Self { overseer }
    }

    /// Every console line from now on: output and run errors of all shards and the commands
    /// sent with [`Self::execute`]. While any console feed exists the latest
    /// [`HISTORY_CAPACITY`] lines are kept for [`Self::history`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`] without a signed-in user and [`Error::Closed`] after
    /// [`Overseer::shutdown`].
    pub fn feed(&self) -> Result<Feed<ConsoleEntry>, Error> {
        let channel = self.channel()?;
        let (registration, attached) = self.overseer.registry().acquire(channel, |entry| {
            if let Some(sink) = entry.sink::<ConsoleSink>() {
                (sink.sender.subscribe(), sink.sender.downgrade())
            } else {
                let (sender, receiver) = broadcast::channel(FEED_CAPACITY);
                let weak = sender.downgrade();
                entry.sinks.push(Box::new(ConsoleSink {
                    sender,
                    history: VecDeque::new(),
                }));
                (receiver, weak)
            }
        });
        let (receiver, sender) = attached.ok_or(Error::Closed)?;
        Ok(Feed::new(receiver, sender, registration))
    }

    /// The latest [`HISTORY_CAPACITY`] lines, oldest first; kept while any console
    /// [feed](Self::feed) exists, empty otherwise.
    #[must_use]
    pub fn history(&self) -> Vec<ConsoleEntry> {
        let Ok(channel) = self.channel() else {
            return Vec::new();
        };
        self.overseer
            .registry()
            .with_entry(&channel, |entry| {
                entry
                    .sink::<ConsoleSink>()
                    .map(|sink| sink.history.iter().cloned().collect())
            })
            .flatten()
            .unwrap_or_default()
    }

    /// Runs `expression` in the signed-in user's runtime on `shard` (`None`: the default
    /// shard) via `POST /api/user/console`. Records it as a [`ConsoleKind::Command`] line and
    /// in [`Self::recent_commands`] first; its output and result arrive on the
    /// [feed](Self::feed).
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] for a blank expression, [`Error::Anonymous`] without a
    /// signed-in user, [`Error::Closed`] after [`Overseer::shutdown`] and [`Error::Net`] when
    /// the request fails.
    pub async fn execute(&self, expression: &str, shard: Option<&str>) -> Result<(), Error> {
        if expression.trim().is_empty() {
            return Err(Error::InvalidInput("a blank console expression".to_owned()));
        }
        let channel = self.channel()?;
        let shard = self.overseer.resolve_shard(shard);
        {
            let mut commands = self.overseer.console_commands();
            commands.retain(|command| command != expression);
            if commands.len() == RECENT_COMMANDS {
                commands.pop_front();
            }
            commands.push_back(expression.to_owned());
        }
        let entry = ConsoleEntry {
            kind: ConsoleKind::Command,
            shard: shard.clone(),
            text: expression.to_owned(),
            received: SystemTime::now(),
        };
        self.overseer.registry().with_entry(&channel, |subscribed| {
            if let Some(sink) = subscribed.sink::<ConsoleSink>() {
                sink.push(entry);
            }
        });
        self.overseer
            .api()
            .console(expression, shard.as_deref())
            .await?;
        Ok(())
    }

    /// The commands sent with [`Self::execute`] in this session, newest last, each once (at
    /// its latest use); at most [`RECENT_COMMANDS`].
    #[must_use]
    pub fn recent_commands(&self) -> Vec<String> {
        self.overseer.console_commands().iter().cloned().collect()
    }

    /// The signed-in user's console channel.
    fn channel(&self) -> Result<Channel, Error> {
        self.overseer.ensure_open()?;
        let user = self.overseer.require_user()?.to_owned();
        Ok(Channel::Console { user })
    }
}

/// The console channel's one sink: the shared sender of every console feed and the history.
struct ConsoleSink {
    sender: broadcast::Sender<ConsoleEntry>,
    history: VecDeque<ConsoleEntry>,
}

impl ConsoleSink {
    fn push(&mut self, entry: ConsoleEntry) {
        if self.history.len() == HISTORY_CAPACITY {
            self.history.pop_front();
        }
        self.history.push_back(entry.clone());
        // Fails only without receivers.
        let _ = self.sender.send(entry);
    }
}

impl Sink for ConsoleSink {
    fn deliver(&mut self, payload: &Payload) {
        let Payload::Console(console) = payload else {
            return;
        };
        let received = SystemTime::now();
        let entry = |kind, shard: &Option<String>, text: &String| ConsoleEntry {
            kind,
            shard: shard.clone(),
            text: text.clone(),
            received,
        };
        match console {
            ConsoleEvent::Output {
                log,
                results,
                shard,
            } => {
                for line in log {
                    self.push(entry(ConsoleKind::Log, shard, line));
                }
                for line in results {
                    self.push(entry(ConsoleKind::Result, shard, line));
                }
            }
            ConsoleEvent::Error { error, shard } => {
                self.push(entry(ConsoleKind::Error, shard, error));
            }
        }
    }

    fn is_unused(&self) -> bool {
        self.sender.receiver_count() == 0
    }
}

/// Removes HTML tags: a `<` followed by a letter, `/` or `!` up to the next `>`. Other `<`
/// characters are kept.
fn strip_tags(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        text.push_str(&rest[..start]);
        let tag = &rest[start..];
        let opens_tag = tag[1..]
            .chars()
            .next()
            .is_some_and(|next| next.is_ascii_alphabetic() || next == '/' || next == '!');
        match tag.find('>') {
            Some(end) if opens_tag => rest = &tag[end + 1..],
            _ => {
                text.push('<');
                rest = &tag[1..];
            }
        }
    }
    text.push_str(rest);
    text
}

/// Decodes named and numeric character references.
fn decode_entities(text: &str) -> String {
    let mut decoded = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        decoded.push_str(&rest[..start]);
        let reference = &rest[start..];
        if let Some((character, end)) = reference
            .find(';')
            .and_then(|end| Some((decode_reference(&reference[1..end])?, end)))
        {
            decoded.push(character);
            rest = &reference[end + 1..];
        } else {
            decoded.push('&');
            rest = &reference[1..];
        }
    }
    decoded.push_str(rest);
    decoded
}

/// The character of a reference between `&` and `;`.
fn decode_reference(name: &str) -> Option<char> {
    let code = if let Some(hex) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
        u32::from_str_radix(hex, 16).ok()?
    } else if let Some(decimal) = name.strip_prefix('#') {
        decimal.parse().ok()?
    } else {
        return match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ => None,
        };
    };
    char::from_u32(code)
}

#[cfg(test)]
mod tests {
    use overseer_net::core::socket::Payload;
    use overseer_net::model::console::ConsoleEvent;
    use overseer_net::rt::SystemTime;
    use tokio::sync::broadcast;

    use super::{ConsoleEntry, ConsoleKind, ConsoleSink, HISTORY_CAPACITY};
    use crate::live::Sink;

    fn plain(kind: ConsoleKind, text: &str) -> String {
        ConsoleEntry {
            kind,
            shard: None,
            text: text.to_owned(),
            received: SystemTime::UNIX_EPOCH,
        }
        .plain_text()
    }

    #[test]
    fn plain_text_strips_tags_and_decodes_references() {
        assert_eq!(
            plain(
                ConsoleKind::Log,
                r#"<span style="color:red">Fish &amp; chips &lt;3 &#x27;x&#x27; &#39;y&#39; &quot;z&quot;</span>"#
            ),
            r#"Fish & chips <3 'x' 'y' "z""#
        );
        assert_eq!(
            plain(
                ConsoleKind::Result,
                "a < b<br/>c &gt; d &unknown; &#xZZ; & e"
            ),
            "a < bc > d &unknown; &#xZZ; & e"
        );
        assert_eq!(
            plain(ConsoleKind::Log, "&#x1F600; &nbsp;"),
            "\u{1f600} \u{a0}"
        );
        assert_eq!(plain(ConsoleKind::Log, "<b>unclosed <i"), "unclosed <i");
        // Commands are what the user typed, not HTML.
        assert_eq!(plain(ConsoleKind::Command, "'<b>' &amp;"), "'<b>' &amp;");
    }

    #[test]
    fn output_becomes_lines_and_the_history_is_a_ring() {
        let (sender, mut receiver) = broadcast::channel(16);
        let mut sink = ConsoleSink {
            sender,
            history: std::collections::VecDeque::new(),
        };
        sink.deliver(&Payload::Console(ConsoleEvent::Output {
            log: vec!["one".to_owned(), "two".to_owned()],
            results: vec!["3".to_owned()],
            shard: Some("shard3".to_owned()),
        }));
        sink.deliver(&Payload::Console(ConsoleEvent::Error {
            error: "Error: boom".to_owned(),
            shard: None,
        }));
        let lines: Vec<_> = std::iter::from_fn(|| receiver.try_recv().ok())
            .map(|entry| (entry.kind, entry.shard, entry.text))
            .collect();
        assert_eq!(
            lines,
            [
                (
                    ConsoleKind::Log,
                    Some("shard3".to_owned()),
                    "one".to_owned()
                ),
                (
                    ConsoleKind::Log,
                    Some("shard3".to_owned()),
                    "two".to_owned()
                ),
                (
                    ConsoleKind::Result,
                    Some("shard3".to_owned()),
                    "3".to_owned()
                ),
                (ConsoleKind::Error, None, "Error: boom".to_owned()),
            ]
        );

        for index in 0..HISTORY_CAPACITY + 5 {
            sink.push(ConsoleEntry {
                kind: ConsoleKind::Log,
                shard: None,
                text: index.to_string(),
                received: SystemTime::UNIX_EPOCH,
            });
        }
        assert_eq!(sink.history.len(), HISTORY_CAPACITY);
        assert_eq!(sink.history.front().unwrap().text, "5");
        assert_eq!(
            sink.history.back().unwrap().text,
            (HISTORY_CAPACITY + 4).to_string()
        );
    }
}
