//! The presentation logic of the Memory tab: the path the user types, previews of values,
//! and the rows of the tree and their pages.
//!
//! The Memory rules themselves are the library's ([`overseer::memory`]): which keys a path
//! can hold ([`MemoryPath`]), how a write changes a read value, and the segment limits. A key
//! that a path cannot hold is shown from the loaded parent, read-only.

use std::collections::{HashMap, HashSet};

use overseer::memory::{MAX_SEGMENT, MemoryPath, MemoryPathError};
use serde_json::Value;

/// The most children of an object or an array that show before "Show more"; each "Show more"
/// shows this many more.
pub const PAGE: usize = 200;
/// The most characters of a string in its preview.
const PREVIEW_CHARS: usize = 48;

/// The keys from the loaded value down to a value; empty for the loaded value itself.
pub type Keys = Vec<String>;

/// The path the user typed, without spaces around it and without a leading `Memory.`; the
/// root path for all of `Memory`. The error tells why the path is not correct.
pub fn parse_path(text: &str) -> Result<MemoryPath, &'static str> {
    let text = text.trim();
    let text = match text.strip_prefix("Memory") {
        Some("") => "",
        Some(rest) => rest.strip_prefix('.').unwrap_or(text),
        None => text,
    };
    MemoryPath::parse(text).map_err(|error| match error {
        MemoryPathError::EmptyKey => {
            "A path has no empty parts: write creeps.c123, not creeps..c123."
        }
        MemoryPathError::Unaddressable(_) => {
            "A part of a path cannot have a backslash or a line break."
        }
    })
}

/// Why `key` cannot be a new key of `parent`; `None` when it can.
pub fn new_key_error(key: &str, parent: &Value) -> Option<&'static str> {
    if key.is_empty() {
        Some("Type a key.")
    } else if !MemoryPath::is_key(key) {
        Some("A key cannot have a dot, a backslash or a line break.")
    } else if parent.get(key).is_some() {
        Some("The key exists: select it in the tree to edit its value.")
    } else {
        None
    }
}

/// A short preview of `value`: the number of keys or items of an object or an array, a string
/// in quotes (cut after [`PREVIEW_CHARS`] characters), other values as JSON.
pub fn preview(value: &Value) -> String {
    match value {
        Value::Object(map) => match map.len() {
            1 => "{1 key}".to_owned(),
            count => format!("{{{count} keys}}"),
        },
        Value::Array(items) => match items.len() {
            1 => "[1 item]".to_owned(),
            count => format!("[{count} items]"),
        },
        Value::String(text) => {
            let mut short = text.chars();
            let start: String = short.by_ref().take(PREVIEW_CHARS).collect();
            let quoted = Value::String(start).to_string();
            if short.next().is_some() {
                format!("{quoted}…")
            } else {
                quoted
            }
        }
        other => other.to_string(),
    }
}

/// Whether `value` is an object or an array.
pub fn is_container(value: &Value) -> bool {
    matches!(value, Value::Object(_) | Value::Array(_))
}

/// The number of children of `value` (keys of an object, items of an array).
fn child_count(value: &Value) -> usize {
    match value {
        Value::Object(map) => map.len(),
        Value::Array(items) => items.len(),
        _ => 0,
    }
}

/// The first `limit` children of `value` with their keys (indexes for an array).
fn children(value: &Value, limit: usize) -> Vec<(String, &Value)> {
    match value {
        Value::Object(map) => map
            .iter()
            .take(limit)
            .map(|(key, child)| (key.clone(), child))
            .collect(),
        Value::Array(items) => items
            .iter()
            .take(limit)
            .enumerate()
            .map(|(index, child)| (index.to_string(), child))
            .collect(),
        _ => Vec::new(),
    }
}

/// The identity of a row of the tree.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RowId {
    /// The value at these keys.
    Entry(Keys),
    /// "Show more" of the object or array at these keys.
    More(Keys),
}

/// A row of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// Its identity.
    pub id: RowId,
    /// Its depth: 0 for the children of the loaded value.
    pub depth: usize,
    /// What it shows.
    pub kind: RowKind,
}

/// What a row of the tree shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// A value: its key and [`preview`]; for an object or an array, whether it is expanded.
    Entry {
        /// The key (an index in an array).
        key: String,
        /// The preview of the value.
        preview: String,
        /// `Some(expanded)` for an object or an array, `None` for other values.
        expanded: Option<bool>,
    },
    /// "Show more": the children not shown yet.
    More {
        /// How many children do not show.
        hidden: usize,
    },
}

/// The rows of the tree of `value`: its children, and the children of each expanded object or
/// array below them (`expanded` holds their keys), in order. Each object or array shows its
/// first `shown[keys]` children (default [`PAGE`]), then a "Show more" row.
pub fn rows(value: &Value, expanded: &HashSet<Keys>, shown: &HashMap<Keys, usize>) -> Vec<Row> {
    let mut rows = Vec::new();
    push_rows(value, &mut Vec::new(), expanded, shown, &mut rows);
    rows
}

/// Adds the rows of the children of `value`, at `keys`, to `rows`.
fn push_rows(
    value: &Value,
    keys: &mut Keys,
    expanded: &HashSet<Keys>,
    shown: &HashMap<Keys, usize>,
    rows: &mut Vec<Row>,
) {
    let depth = keys.len();
    let limit = shown.get(keys).copied().unwrap_or(PAGE);
    for (key, child) in children(value, limit) {
        keys.push(key.clone());
        let open = is_container(child).then(|| expanded.contains(keys));
        rows.push(Row {
            id: RowId::Entry(keys.clone()),
            depth,
            kind: RowKind::Entry {
                key,
                preview: preview(child),
                expanded: open,
            },
        });
        if open == Some(true) {
            push_rows(child, keys, expanded, shown, rows);
        }
        keys.pop();
    }
    let total = child_count(value);
    if total > limit {
        rows.push(Row {
            id: RowId::More(keys.clone()),
            depth,
            kind: RowKind::More {
                hidden: total - limit,
            },
        });
    }
}

/// The segment number in `text` (`0` to [`MAX_SEGMENT`]).
pub fn parse_segment(text: &str) -> Option<u8> {
    text.trim()
        .parse::<u8>()
        .ok()
        .filter(|id| *id <= MAX_SEGMENT)
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use overseer::memory::MemoryPath;
    use serde_json::json;

    use super::{
        PAGE, Row, RowId, RowKind, new_key_error, parse_path, parse_segment, preview, rows,
    };

    fn keys(keys: &[&str]) -> Vec<String> {
        keys.iter().map(|key| (*key).to_owned()).collect()
    }

    /// The rows as `depth:key` and `depth:+hidden` for "Show more".
    fn outline(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| match &row.kind {
                RowKind::Entry { key, .. } => format!("{}:{key}", row.depth),
                RowKind::More { hidden } => format!("{}:+{hidden}", row.depth),
            })
            .collect()
    }

    #[test]
    fn parses_typed_paths() {
        let path = |text: &str| Ok(MemoryPath::parse(text).unwrap());
        assert_eq!(parse_path("  creeps.c1 "), path("creeps.c1"));
        assert_eq!(parse_path("Memory.creeps"), path("creeps"));
        assert_eq!(parse_path("Memory"), Ok(MemoryPath::root()));
        assert_eq!(parse_path(""), Ok(MemoryPath::root()));
        assert_eq!(parse_path("Memoryless"), path("Memoryless"));
        assert!(parse_path("creeps..c1").is_err());
        assert!(parse_path("creeps.").is_err());
        assert!(parse_path(".creeps").is_err());
        assert!(parse_path("a\\b").is_err());
    }

    #[test]
    fn checks_new_keys() {
        let parent = json!({"taken": 1});
        assert_eq!(new_key_error("e2e", &parent), None);
        assert!(new_key_error("", &parent).is_some());
        assert!(new_key_error("a.b", &parent).is_some());
        assert!(new_key_error("taken", &parent).is_some());
    }

    #[test]
    fn previews_values() {
        assert_eq!(preview(&json!({})), "{0 keys}");
        assert_eq!(preview(&json!({"a": 1})), "{1 key}");
        assert_eq!(preview(&json!([1, 2])), "[2 items]");
        assert_eq!(preview(&json!([1])), "[1 item]");
        assert_eq!(preview(&json!("say \"hi\"\n")), r#""say \"hi\"\n""#);
        assert_eq!(preview(&json!(12.5)), "12.5");
        assert_eq!(preview(&json!(null)), "null");
        assert_eq!(preview(&json!(true)), "true");
        let long = "é".repeat(60);
        assert_eq!(preview(&json!(long)), format!("\"{}\"…", "é".repeat(48)));
        assert_eq!(
            preview(&json!("é".repeat(48))),
            format!("\"{}\"", "é".repeat(48))
        );
    }

    #[test]
    fn shows_expanded_children_only() {
        let value = json!({"creeps": {"c1": {"role": "h"}, "c2": {}}, "ticks": 4, "list": [1]});
        let expanded = HashSet::from([keys(&["creeps"]), keys(&["creeps", "c1"])]);
        let rows = rows(&value, &expanded, &HashMap::new());
        assert_eq!(
            outline(&rows),
            ["0:creeps", "1:c1", "2:role", "1:c2", "0:list", "0:ticks"]
        );
        assert_eq!(rows[0].id, RowId::Entry(keys(&["creeps"])));
        assert_eq!(
            rows[0].kind,
            RowKind::Entry {
                key: "creeps".into(),
                preview: "{2 keys}".into(),
                expanded: Some(true),
            }
        );
        // An object or array that is not expanded says so; other values cannot expand.
        assert!(matches!(
            rows[3].kind,
            RowKind::Entry {
                expanded: Some(false),
                ..
            }
        ));
        assert!(matches!(
            rows[5].kind,
            RowKind::Entry { expanded: None, .. }
        ));
    }

    #[test]
    fn pages_large_objects_and_arrays() {
        let big: Vec<u32> = (0..450).collect();
        let value = json!({ "big": big });
        let expanded = HashSet::from([keys(&["big"])]);
        let first = rows(&value, &expanded, &HashMap::new());
        assert_eq!(first.len(), 1 + PAGE + 1);
        let more = first.last().unwrap();
        assert_eq!(more.id, RowId::More(keys(&["big"])));
        assert_eq!(more.depth, 1);
        assert_eq!(more.kind, RowKind::More { hidden: 450 - PAGE });

        let shown = HashMap::from([(keys(&["big"]), 2 * PAGE)]);
        let second = rows(&value, &expanded, &shown);
        assert_eq!(
            second.last().unwrap().kind,
            RowKind::More {
                hidden: 450 - 2 * PAGE
            }
        );
        let all = HashMap::from([(keys(&["big"]), 3 * PAGE)]);
        let last = rows(&value, &expanded, &all);
        assert_eq!(last.len(), 1 + 450);
        assert_eq!(outline(&last).last().unwrap(), "1:449");
    }

    #[test]
    fn parses_segment_numbers() {
        assert_eq!(parse_segment(" 0 "), Some(0));
        assert_eq!(parse_segment("99"), Some(99));
        assert_eq!(parse_segment("100"), None);
        assert_eq!(parse_segment("-1"), None);
        assert_eq!(parse_segment("x"), None);
    }
}
