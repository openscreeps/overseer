//! The pure logic of the Memory tab: paths, previews of values, the rows of the tree and their
//! pages, the change of the loaded value after a write, and the limits of segments.
//!
//! The server splits a path at its dots and puts each part in a quoted JavaScript string
//! (`Memory['creeps']['c1']`). Thus a key with a dot cannot be in a path, and a key with a
//! backslash or a line break does not survive the quotes. The tab shows the values under such
//! keys from the loaded parent, read-only.

use std::collections::{HashMap, HashSet};

use overseer::memory::{MAX_SEGMENT, MemoryValue};
use serde_json::Value;

/// The most children of an object or an array that show before "Show more"; each "Show more"
/// shows this many more.
pub const PAGE: usize = 200;
/// The most characters of a string in its preview.
const PREVIEW_CHARS: usize = 48;
/// The most UTF-16 units in a segment (the server's limit, 100 KiB).
pub const SEGMENT_LIMIT: usize = 100 * 1024;

/// The keys from the loaded value down to a value; empty for the loaded value itself.
pub type Keys = Vec<String>;

/// Whether `key` can be a part of a path: not empty, and without a dot, a backslash or a line
/// break.
pub fn addressable(key: &str) -> bool {
    !key.is_empty() && !key.contains(['.', '\\', '\n', '\r'])
}

/// The path of the value at `keys` under the value loaded from path `base` (empty for all of
/// `Memory`); `None` when a key cannot be a part of a path.
pub fn path_of(base: &str, keys: &[String]) -> Option<String> {
    let mut path = base.to_owned();
    for key in keys {
        if !addressable(key) {
            return None;
        }
        if !path.is_empty() {
            path.push('.');
        }
        path.push_str(key);
    }
    Some(path)
}

/// The path the user typed, without spaces around it and without a leading `Memory.`; empty for
/// all of `Memory`. The error tells why the path is not correct.
pub fn parse_path(text: &str) -> Result<String, &'static str> {
    let text = text.trim();
    let text = match text.strip_prefix("Memory") {
        Some("") => "",
        Some(rest) => rest.strip_prefix('.').unwrap_or(text),
        None => text,
    };
    if text.is_empty() {
        return Ok(String::new());
    }
    for part in text.split('.') {
        if part.is_empty() {
            return Err("A path has no empty parts: write creeps.c123, not creeps..c123.");
        }
        if !addressable(part) {
            return Err("A part of a path cannot have a backslash or a line break.");
        }
    }
    Ok(text.to_owned())
}

/// Why `key` cannot be a new key of `parent`; `None` when it can.
pub fn new_key_error(key: &str, parent: &Value) -> Option<&'static str> {
    if key.is_empty() {
        Some("Type a key.")
    } else if !addressable(key) {
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

/// The child of `value` at `key` (an index for an array).
fn child<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Object(map) => map.get(key),
        Value::Array(items) => items.get(key.parse::<usize>().ok()?),
        _ => None,
    }
}

/// The value at `keys` under `value`.
pub fn value_at<'a>(value: &'a Value, keys: &[String]) -> Option<&'a Value> {
    keys.iter().try_fold(value, |value, key| child(value, key))
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

/// Puts `new` at `keys` (not empty) under `value`, or deletes the value there for `None`, as the
/// player's code does: a deleted item of an array becomes `null` (JSON has no holes). Whether
/// the parent of `keys` exists and can take the change.
pub fn patch(value: &mut Value, keys: &[String], new: Option<Value>) -> bool {
    let Some((last, parents)) = keys.split_last() else {
        return false;
    };
    let mut parent = value;
    for key in parents {
        let next = match parent {
            Value::Object(map) => map.get_mut(key),
            Value::Array(items) => key
                .parse::<usize>()
                .ok()
                .and_then(|index| items.get_mut(index)),
            _ => None,
        };
        let Some(next) = next else {
            return false;
        };
        parent = next;
    }
    match (parent, new) {
        (Value::Object(map), Some(new)) => {
            map.insert(last.clone(), new);
            true
        }
        (Value::Object(map), None) => {
            map.remove(last);
            true
        }
        (Value::Array(items), new) => {
            let Ok(index) = last.parse::<usize>() else {
                return false;
            };
            if let Some(item) = items.get_mut(index) {
                *item = new.unwrap_or(Value::Null);
                true
            } else if let Some(new) = new
                && index == items.len()
            {
                items.push(new);
                true
            } else {
                false
            }
        }
        _ => false,
    }
}

/// Puts the applied write of `new` at `keys` into `loaded`, the value read from a path, or
/// deletes the value there for `None`: all of `loaded` for no keys ([`patch`] otherwise).
pub fn apply_write(loaded: &mut MemoryValue, keys: &[String], new: Option<Value>) {
    if keys.is_empty() {
        *loaded = new.map_or(MemoryValue::Undefined, MemoryValue::Json);
    } else if let MemoryValue::Json(value) = loaded {
        patch(value, keys, new);
    }
}

/// The keys from all of `Memory` down to the value at `path` (a path of [`parse_path`] or
/// [`path_of`]); none for all of `Memory`.
pub fn keys_of(path: &str) -> Keys {
    if path.is_empty() {
        Vec::new()
    } else {
        path.split('.').map(str::to_owned).collect()
    }
}

/// Whether `a` and `b` are the same JSON value. Numbers compare as JavaScript numbers do
/// (`1` and `1.0` are the same), because the player's code stores every number as a double.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => {
            a == b || a.as_f64().map(f64::to_bits) == b.as_f64().map(f64::to_bits)
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, a)| b.get(key).is_some_and(|b| same(a, b)))
        }
        (a, b) => a == b,
    }
}

/// Whether `read` (the value at a written path) shows the write of `expected`, or the deletion
/// for `None`.
pub fn shows_write(read: &MemoryValue, expected: Option<&Value>) -> bool {
    match (read, expected) {
        (MemoryValue::Json(read), Some(expected)) => same(read, expected),
        (MemoryValue::Undefined, None) => true,
        _ => false,
    }
}

/// The segment number in `text` (`0` to [`MAX_SEGMENT`]).
pub fn parse_segment(text: &str) -> Option<u8> {
    text.trim()
        .parse::<u8>()
        .ok()
        .filter(|id| *id <= MAX_SEGMENT)
}

/// The length of `text` as the server counts it (UTF-16 units), for [`SEGMENT_LIMIT`].
pub fn segment_length(text: &str) -> usize {
    text.encode_utf16().count()
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use overseer::memory::MemoryValue;
    use serde_json::{Value, json};

    use super::{
        PAGE, Row, RowId, RowKind, apply_write, keys_of, new_key_error, parse_path, parse_segment,
        patch, path_of, preview, rows, same, segment_length, shows_write, value_at,
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
    fn joins_keys_into_paths() {
        assert_eq!(
            path_of("", &keys(&["creeps", "c1"])),
            Some("creeps.c1".into())
        );
        assert_eq!(
            path_of("creeps", &keys(&["c1", "0"])),
            Some("creeps.c1.0".into())
        );
        assert_eq!(path_of("creeps", &[]), Some("creeps".into()));
        assert_eq!(path_of("", &[]), Some(String::new()));
    }

    #[test]
    fn keys_that_a_path_cannot_hold_make_no_path() {
        for key in ["a.b", "a\\b", "a\nb", "a\rb", ""] {
            assert_eq!(path_of("", &keys(&["rooms", key, "x"])), None, "{key:?}");
        }
        assert_eq!(
            path_of("", &keys(&["W1N1", "my-key", "é 1"])),
            Some("W1N1.my-key.é 1".into())
        );
    }

    #[test]
    fn parses_typed_paths() {
        assert_eq!(parse_path("  creeps.c1 "), Ok("creeps.c1".into()));
        assert_eq!(parse_path("Memory.creeps"), Ok("creeps".into()));
        assert_eq!(parse_path("Memory"), Ok(String::new()));
        assert_eq!(parse_path(""), Ok(String::new()));
        assert_eq!(parse_path("Memoryless"), Ok("Memoryless".into()));
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
    fn finds_values_by_keys() {
        let value = json!({"a": [{"b": 2}]});
        assert_eq!(value_at(&value, &keys(&["a", "0", "b"])), Some(&json!(2)));
        assert_eq!(value_at(&value, &[]), Some(&value));
        assert_eq!(value_at(&value, &keys(&["a", "1"])), None);
        assert_eq!(value_at(&value, &keys(&["a", "x"])), None);
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
    fn patches_the_loaded_value() {
        let mut value = json!({"a": {"b": 1}, "list": [1, 2]});
        assert!(patch(
            &mut value,
            &keys(&["a", "c"]),
            Some(json!({"x": true}))
        ));
        assert!(patch(&mut value, &keys(&["a", "b"]), None));
        assert!(patch(&mut value, &keys(&["list", "0"]), Some(json!(9))));
        assert!(patch(&mut value, &keys(&["list", "2"]), Some(json!(3))));
        assert!(patch(&mut value, &keys(&["list", "1"]), None));
        assert_eq!(
            value,
            json!({"a": {"c": {"x": true}}, "list": [9, null, 3]})
        );
        // A missing parent, or a parent that cannot hold keys, takes nothing.
        assert!(!patch(&mut value, &keys(&["gone", "x"]), Some(json!(1))));
        assert!(!patch(&mut value, &keys(&["list", "5"]), Some(json!(1))));
        assert!(!patch(
            &mut value,
            &keys(&["a", "c", "x", "y"]),
            Some(json!(1))
        ));
        assert!(!patch(&mut value, &[], Some(json!(1))));
        assert_eq!(
            value,
            json!({"a": {"c": {"x": true}}, "list": [9, null, 3]})
        );
    }

    #[test]
    fn applies_a_write_to_a_read_value() {
        let mut loaded = MemoryValue::Json(json!({"a": 1}));
        apply_write(&mut loaded, &keys(&["b"]), Some(json!(2)));
        assert_eq!(loaded, MemoryValue::Json(json!({"a": 1, "b": 2})));
        apply_write(&mut loaded, &keys(&["a"]), None);
        assert_eq!(loaded, MemoryValue::Json(json!({"b": 2})));
        // No keys: the write replaces the value, or deletes it.
        apply_write(&mut loaded, &[], Some(json!(3)));
        assert_eq!(loaded, MemoryValue::Json(json!(3)));
        apply_write(&mut loaded, &[], None);
        assert_eq!(loaded, MemoryValue::Undefined);
        // Nothing was read: a key has no parent.
        apply_write(&mut loaded, &keys(&["a"]), Some(json!(1)));
        assert_eq!(loaded, MemoryValue::Undefined);
    }

    #[test]
    fn splits_a_path_into_keys() {
        assert_eq!(keys_of(""), keys(&[]));
        assert_eq!(keys_of("creeps"), keys(&["creeps"]));
        let path = path_of("creeps.c1", &keys(&["memory", "role"])).unwrap();
        assert_eq!(keys_of(&path), keys(&["creeps", "c1", "memory", "role"]));
    }

    #[test]
    fn compares_numbers_as_javascript_does() {
        let written: Value = serde_json::from_str(r#"{"a": 1.0, "b": [2.50]}"#).unwrap();
        let read: Value = serde_json::from_str(r#"{"b": [2.5], "a": 1}"#).unwrap();
        assert!(same(&written, &read));
        assert!(!same(&json!({"a": 1}), &json!({"a": 2})));
        assert!(!same(&json!({"a": 1}), &json!({"a": 1, "b": 1})));
        assert!(!same(&json!([1]), &json!([1, 1])));
        assert!(!same(&json!("1"), &json!(1)));
    }

    #[test]
    fn sees_when_a_write_applied() {
        let written = json!({"a": 1.0});
        assert!(shows_write(
            &MemoryValue::Json(json!({"a": 1})),
            Some(&written)
        ));
        assert!(!shows_write(
            &MemoryValue::Json(json!({"a": 2})),
            Some(&written)
        ));
        assert!(!shows_write(&MemoryValue::Undefined, Some(&written)));
        // A deletion shows as nothing at the path; a `null` there is a value.
        assert!(shows_write(&MemoryValue::Undefined, None));
        assert!(!shows_write(&MemoryValue::Json(Value::Null), None));
        assert!(!shows_write(&MemoryValue::IncorrectPath, None));
    }

    #[test]
    fn parses_segment_numbers() {
        assert_eq!(parse_segment(" 0 "), Some(0));
        assert_eq!(parse_segment("99"), Some(99));
        assert_eq!(parse_segment("100"), None);
        assert_eq!(parse_segment("-1"), None);
        assert_eq!(parse_segment("x"), None);
    }

    #[test]
    fn counts_segment_length_as_the_server_does() {
        assert_eq!(segment_length("ab"), 2);
        assert_eq!(segment_length("é"), 1);
        assert_eq!(segment_length("😀"), 2);
    }
}
