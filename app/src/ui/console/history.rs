//! The console input's command history: ↑/↓ navigation ([`CommandHistory`]) and the commands
//! remembered per server across restarts ([`SavedCommands`]).
//!
//! Persisted as one JSON document (`localStorage` key [`STORAGE_KEY`], written with
//! [`VERSION`]) by every window that sends a command (it reads the document again before each
//! write, so windows do not drop each other's commands). Parsing never fails: an invalid
//! document is empty, entries that are not strings are dropped.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

/// `localStorage` key of the document.
pub const STORAGE_KEY: &str = "overseer.console";
/// Version written into the document.
pub const VERSION: u32 = 1;
/// Commands kept per server (as many as the library's `Console::recent_commands`).
pub const LIMIT: usize = overseer::console::RECENT_COMMANDS;

/// The commands sent on each server, oldest first, each once (at its latest use).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SavedCommands {
    /// By server base URL.
    pub servers: BTreeMap<String, Vec<String>>,
}

impl SavedCommands {
    /// Reads the document leniently (see the module docs).
    pub fn parse(json: Option<&str>) -> Self {
        let Some(Value::Object(document)) = json.and_then(|json| serde_json::from_str(json).ok())
        else {
            return Self::default();
        };
        let servers = document
            .get("servers")
            .and_then(Value::as_object)
            .map(|servers| {
                servers
                    .iter()
                    .filter_map(|(url, commands)| {
                        let mut kept: Vec<String> = Vec::new();
                        for command in commands.as_array()?.iter().filter_map(Value::as_str) {
                            push_unique(&mut kept, command);
                        }
                        Some((url.clone(), kept))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self { servers }
    }

    /// The document to save.
    pub fn to_json(&self) -> String {
        let servers: Map<String, Value> = self
            .servers
            .iter()
            .map(|(url, commands)| (url.clone(), Value::from(commands.clone())))
            .collect();
        let mut document = Map::new();
        document.insert("version".to_owned(), Value::from(VERSION));
        document.insert("servers".to_owned(), Value::Object(servers));
        Value::Object(document).to_string()
    }

    /// The commands of the server with base URL `url`, oldest first.
    pub fn commands(&self, url: &str) -> &[String] {
        self.servers.get(url).map_or(&[], Vec::as_slice)
    }

    /// Remembers `command` as the latest of the server with base URL `url`.
    pub fn record(&mut self, url: &str, command: &str) {
        push_unique(self.servers.entry(url.to_owned()).or_default(), command);
    }
}

/// Appends `command` (moving an earlier copy to the end), keeping the latest [`LIMIT`].
fn push_unique(commands: &mut Vec<String>, command: &str) {
    if command.trim().is_empty() {
        return;
    }
    commands.retain(|known| known != command);
    commands.push(command.to_owned());
    if commands.len() > LIMIT {
        commands.drain(..commands.len() - LIMIT);
    }
}

/// The input's history: ↑ goes back from the newest command, ↓ forward and finally to what
/// was being typed before ↑.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandHistory {
    /// Oldest first.
    entries: Vec<String>,
    /// The entry shown, while navigating.
    cursor: Option<usize>,
    /// What the input held when navigation began.
    draft: String,
}

impl CommandHistory {
    /// A history of `entries`, oldest first.
    pub fn new(entries: Vec<String>) -> Self {
        let mut history = Self::default();
        for entry in entries {
            push_unique(&mut history.entries, &entry);
        }
        history
    }

    /// Records a sent command and ends navigation.
    pub fn record(&mut self, command: &str) {
        push_unique(&mut self.entries, command);
        self.reset();
    }

    /// Ends navigation (the user edited the input).
    pub fn reset(&mut self) {
        self.cursor = None;
        self.draft.clear();
    }

    /// The command before the one shown (the newest when not navigating; `current` is kept
    /// as the draft); `None` at the oldest or without commands.
    pub fn previous(&mut self, current: &str) -> Option<String> {
        let cursor = match self.cursor {
            None if self.entries.is_empty() => return None,
            None => {
                current.clone_into(&mut self.draft);
                self.entries.len() - 1
            }
            Some(0) => return None,
            Some(cursor) => cursor - 1,
        };
        self.cursor = Some(cursor);
        self.entries.get(cursor).cloned()
    }

    /// The command after the one shown, or the draft after the newest; `None` when not
    /// navigating.
    pub fn next(&mut self) -> Option<String> {
        let cursor = self.cursor?;
        if cursor + 1 < self.entries.len() {
            self.cursor = Some(cursor + 1);
            return self.entries.get(cursor + 1).cloned();
        }
        self.cursor = None;
        Some(std::mem::take(&mut self.draft))
    }
}

#[cfg(test)]
mod tests {
    use super::{CommandHistory, LIMIT, SavedCommands};

    fn of(entries: &[&str]) -> CommandHistory {
        CommandHistory::new(entries.iter().map(|entry| (*entry).to_owned()).collect())
    }

    #[test]
    fn navigates_back_and_forth_to_the_draft() {
        let mut history = of(&["a", "b", "c"]);
        assert_eq!(history.next(), None, "↓ does nothing before ↑");
        assert_eq!(history.previous("typing").as_deref(), Some("c"));
        assert_eq!(history.previous("c").as_deref(), Some("b"));
        assert_eq!(history.previous("b").as_deref(), Some("a"));
        assert_eq!(history.previous("a"), None, "stops at the oldest");
        assert_eq!(history.next().as_deref(), Some("b"));
        assert_eq!(history.next().as_deref(), Some("c"));
        assert_eq!(
            history.next().as_deref(),
            Some("typing"),
            "back to the draft"
        );
        assert_eq!(history.next(), None);
        assert_eq!(
            history.previous("").as_deref(),
            Some("c"),
            "navigation starts over"
        );
    }

    #[test]
    fn empty_history_has_nothing() {
        let mut history = of(&[]);
        assert_eq!(history.previous("x"), None);
        assert_eq!(history.next(), None);
    }

    #[test]
    fn recording_moves_repeats_to_the_end_and_resets() {
        let mut history = of(&["a", "b", "a"]);
        assert_eq!(
            history.entries,
            ["b", "a"],
            "duplicates once, at the latest"
        );
        history.previous("");
        history.record("b");
        assert_eq!(history.entries, ["a", "b"]);
        assert_eq!(history.previous("").as_deref(), Some("b"));
        history.record("   ");
        assert_eq!(history.entries, ["a", "b"], "blank commands are not kept");
        let mut long = of(&[]);
        for index in 0..LIMIT + 5 {
            long.record(&index.to_string());
        }
        assert_eq!(long.entries.len(), LIMIT);
        assert_eq!(long.entries[0], "5", "the oldest go first");
    }

    #[test]
    fn multi_line_commands_are_one_entry() {
        let mut history = of(&["for (const c in Game.creeps) {\n  console.log(c);\n}"]);
        assert_eq!(
            history.previous("").as_deref(),
            Some("for (const c in Game.creeps) {\n  console.log(c);\n}")
        );
    }

    #[test]
    fn saved_commands_round_trip_per_server() {
        let mut saved = SavedCommands::default();
        saved.record("https://screeps.com/", "Game.time");
        saved.record("http://localhost:21025/", "Memory");
        saved.record("https://screeps.com/", "Game.cpu\n.bucket");
        saved.record("https://screeps.com/", "Game.time");
        assert_eq!(
            saved.commands("https://screeps.com/"),
            ["Game.cpu\n.bucket", "Game.time"]
        );
        assert_eq!(SavedCommands::parse(Some(&saved.to_json())), saved);
        assert_eq!(saved.commands("http://other/"), [] as [&str; 0]);
    }

    #[test]
    fn parsing_is_lenient() {
        for json in [
            None,
            Some(""),
            Some("{"),
            Some("[]"),
            Some("null"),
            Some("{}"),
        ] {
            assert_eq!(
                SavedCommands::parse(json),
                SavedCommands::default(),
                "{json:?}"
            );
        }
        let saved = SavedCommands::parse(Some(
            r#"{"version": 9, "servers": {"a": ["x", 1, null, "y", "x", ""], "b": "nope", "c": []}}"#,
        ));
        assert_eq!(saved.commands("a"), ["y", "x"]);
        assert_eq!(saved.commands("b"), [] as [&str; 0]);
        assert!(saved.servers.contains_key("c"));
        let many: Vec<String> = (0..LIMIT + 3).map(|index| index.to_string()).collect();
        let json = serde_json::json!({ "servers": { "a": many } }).to_string();
        let saved = SavedCommands::parse(Some(&json));
        assert_eq!(saved.commands("a").len(), LIMIT);
        assert_eq!(saved.commands("a")[0], "3");
    }
}
