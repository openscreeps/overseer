//! Queued changes of Memory, and how a value read later shows them.

use serde_json::Value;

use super::MemoryValue;

/// A change of the value at a Memory path, as `POST /api/user/memory` queues it: the player's
/// runtime applies it on a later tick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemoryChange {
    /// Assign this value.
    Set(Value),
    /// Delete the value.
    Delete,
}

impl MemoryChange {
    /// Whether `read`, the value read at the changed path, shows the change: the same value
    /// (numbers compare as JavaScript numbers: `1` and `1.0` are the same), or nothing for a
    /// deletion. This tells only that Memory holds the result, not that this write made it.
    #[must_use]
    pub fn is_shown_by(&self, read: &MemoryValue) -> bool {
        match (self, read) {
            (Self::Set(expected), MemoryValue::Json(read)) => same(read, expected),
            (Self::Delete, MemoryValue::Undefined) => true,
            _ => false,
        }
    }

    /// Applies the change at `keys` under `value`, which was read from a path above the changed
    /// one (no keys: from the changed path itself), as the player's code does: a deleted item
    /// of an array becomes `null` (JSON has no holes). Whether `value` changed: `false` when
    /// the parent of `keys` is not in `value` or cannot take the change.
    pub fn apply(&self, value: &mut MemoryValue, keys: &[&str]) -> bool {
        let new = match self {
            Self::Set(new) => Some(new.clone()),
            Self::Delete => None,
        };
        if keys.is_empty() {
            *value = new.map_or(MemoryValue::Undefined, MemoryValue::Json);
            true
        } else if let MemoryValue::Json(value) = value {
            patch(value, keys, new)
        } else {
            false
        }
    }

    /// The value at `keys` below the changed path after the change, when the change tells it:
    /// for an assignment, the value at `keys` inside it. `None` when unknown.
    #[must_use]
    pub fn below(&self, keys: &[&str]) -> Option<MemoryValue> {
        match self {
            Self::Set(new) => json_at(new, keys).cloned().map(MemoryValue::Json),
            Self::Delete => None,
        }
    }
}

/// The value at `keys` under `value`: object keys, and indexes for arrays.
#[must_use]
pub fn json_at<'a, K: AsRef<str>>(value: &'a Value, keys: &[K]) -> Option<&'a Value> {
    keys.iter().try_fold(value, |value, key| {
        let key = key.as_ref();
        match value {
            Value::Object(map) => map.get(key),
            Value::Array(items) => items.get(key.parse::<usize>().ok()?),
            _ => None,
        }
    })
}

/// Puts `new` at `keys` (not empty) under `value`, or deletes the value there for `None`.
/// Whether the parent of `keys` exists and can take the change.
fn patch(value: &mut Value, keys: &[&str], new: Option<Value>) -> bool {
    let Some((last, parents)) = keys.split_last() else {
        return false;
    };
    let mut parent = value;
    for key in parents {
        let next = match parent {
            Value::Object(map) => map.get_mut(*key),
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
            map.insert((*last).to_owned(), new);
            true
        }
        (Value::Object(map), None) => map.remove(*last).is_some(),
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

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{MemoryChange, MemoryValue, json_at, patch, same};

    #[test]
    fn finds_values_by_keys() {
        let value = json!({"a": [{"b": 2}]});
        assert_eq!(json_at(&value, &["a", "0", "b"]), Some(&json!(2)));
        assert_eq!(json_at::<&str>(&value, &[]), Some(&value));
        assert_eq!(json_at(&value, &["a", "1"]), None);
        assert_eq!(json_at(&value, &["a", "x"]), None);
    }

    #[test]
    fn patches_a_value() {
        let mut value = json!({"a": {"b": 1}, "list": [1, 2]});
        assert!(patch(&mut value, &["a", "c"], Some(json!({"x": true}))));
        assert!(patch(&mut value, &["a", "b"], None));
        assert!(patch(&mut value, &["list", "0"], Some(json!(9))));
        assert!(patch(&mut value, &["list", "2"], Some(json!(3))));
        assert!(patch(&mut value, &["list", "1"], None));
        assert_eq!(
            value,
            json!({"a": {"c": {"x": true}}, "list": [9, null, 3]})
        );
        // A missing parent, or a parent that cannot hold keys, takes nothing.
        assert!(!patch(&mut value, &["gone", "x"], Some(json!(1))));
        assert!(!patch(&mut value, &["list", "5"], Some(json!(1))));
        assert!(!patch(&mut value, &["a", "c", "x", "y"], Some(json!(1))));
        assert!(!patch(&mut value, &["a", "gone"], None));
        assert!(!patch(&mut value, &[], Some(json!(1))));
        assert_eq!(
            value,
            json!({"a": {"c": {"x": true}}, "list": [9, null, 3]})
        );
    }

    #[test]
    fn applies_a_change_to_a_read_value() {
        let set = |value: Value| MemoryChange::Set(value);
        let mut loaded = MemoryValue::Json(json!({"a": 1}));
        assert!(set(json!(2)).apply(&mut loaded, &["b"]));
        assert_eq!(loaded, MemoryValue::Json(json!({"a": 1, "b": 2})));
        assert!(MemoryChange::Delete.apply(&mut loaded, &["a"]));
        assert_eq!(loaded, MemoryValue::Json(json!({"b": 2})));
        // No keys: the change replaces the value, or deletes it.
        assert!(set(json!(3)).apply(&mut loaded, &[]));
        assert_eq!(loaded, MemoryValue::Json(json!(3)));
        assert!(MemoryChange::Delete.apply(&mut loaded, &[]));
        assert_eq!(loaded, MemoryValue::Undefined);
        // Nothing was read: a key has no parent.
        assert!(!set(json!(1)).apply(&mut loaded, &["a"]));
        assert_eq!(loaded, MemoryValue::Undefined);
    }

    #[test]
    fn tells_values_below_a_change() {
        let set = MemoryChange::Set(json!({"role": {"name": "h"}}));
        assert_eq!(
            set.below(&["role", "name"]),
            Some(MemoryValue::Json(json!("h")))
        );
        assert_eq!(set.below(&["gone"]), None);
        assert_eq!(MemoryChange::Delete.below(&["role"]), None);
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
    fn sees_when_a_change_applied() {
        let written = MemoryChange::Set(json!({"a": 1.0}));
        assert!(written.is_shown_by(&MemoryValue::Json(json!({"a": 1}))));
        assert!(!written.is_shown_by(&MemoryValue::Json(json!({"a": 2}))));
        assert!(!written.is_shown_by(&MemoryValue::Undefined));
        // A deletion shows as nothing at the path; a `null` there is a value.
        assert!(MemoryChange::Delete.is_shown_by(&MemoryValue::Undefined));
        assert!(!MemoryChange::Delete.is_shown_by(&MemoryValue::Json(Value::Null)));
        assert!(!MemoryChange::Delete.is_shown_by(&MemoryValue::IncorrectPath));
    }
}
