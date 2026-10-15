//! Room state built from `room:` events and `/room-history` ticks.
//!
//! [`apply_objects_diff`] applies an `objects` diff exactly like the official client (its
//! `U`/`N` functions with lodash 3.10 `_.merge` and a customizer that replaces an array merged
//! into an array), so a client following the server's diffs converges on the server's state:
//!
//! - `null` removes the object;
//! - an unknown id adds the value as a new object, with its top-level arrays converted to
//!   objects keyed by array index (`"0"`, `"1"`, …);
//! - for a known id, the top-level arrays of both the held object and the patch are converted
//!   to index-keyed objects, then the patch is deep-merged into the object: an array merged into
//!   an array replaces it, a `null` is written as a value (readers treat it like an absent
//!   property), an object merged into a non-object starts from `{}`, and an array merged into a
//!   string or into an object with an integer `length` starts from an index-wise copy of it.
//!
//! The client-side merge reproduces lodash 3.10.1 (`baseMerge`/`baseMergeDeep`),
//! Copyright 2012-2015 The Dojo Foundation, MIT license.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::hash::BuildHasher;
use std::mem;

use serde_json::{Map, Value};

use super::flag::{Flag, FlagError};
use super::types::{RoomEvent, RoomUser, RoomVisualItem};
use super::visual;
use crate::api::types::{GameRoomDecoration, GameRoomObject};

/// Top-level list properties of room objects (the members of `GameRoomObject` in the
/// reference). The client merge holds them as objects keyed by array index; typed access turns
/// them back into lists.
const LIST_PROPERTIES: [&str; 5] = ["body", "creepBody", "depositTypes", "effects", "population"];

/// Largest integer a JavaScript number holds exactly (`Number.MAX_SAFE_INTEGER`).
const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// The state of one room as the official client holds it, built from `room:` events
/// ([`RoomEvent`]s) or `/room-history` ticks.
///
/// Objects keep the client's representation: top-level array properties (such as `effects` or
/// `body`) are objects keyed by array index, and removed properties may be held as `null`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RoomState {
    objects: HashMap<String, Map<String, Value>>,
    users: BTreeMap<String, RoomUser>,
    flags: Option<String>,
    mode: Option<String>,
    game_time: Option<u64>,
    visual: String,
    decorations: Vec<GameRoomDecoration>,
}

impl RoomState {
    /// Applies one update: `objects` per [`apply_objects_diff`], `users` replaced per id; `flags`,
    /// `info`, `gameTime`, `visual` and `decorations` replaced when present and kept when absent
    /// (`flags: null` clears the flags).
    pub fn apply(&mut self, update: RoomEvent) {
        apply_objects_diff(&mut self.objects, update.objects);
        if let Some(users) = update.users {
            self.users.extend(users);
        }
        if let Some(flags) = update.flags {
            self.flags = flags;
        }
        if let Some(info) = update.info {
            self.mode = info.mode;
        }
        if let Some(game_time) = update.game_time {
            self.game_time = Some(game_time);
        }
        if let Some(visual) = update.visual {
            self.visual = visual;
        }
        if let Some(decorations) = update.decorations {
            self.decorations = decorations;
        }
    }

    /// Every object as `(id, object)`, in no particular order.
    pub fn objects(&self) -> impl Iterator<Item = (&str, &Map<String, Value>)> {
        self.objects
            .iter()
            .map(|(id, object)| (id.as_str(), object))
    }

    /// The object with this id.
    #[must_use]
    pub fn object(&self, id: &str) -> Option<&Map<String, Value>> {
        self.objects.get(id)
    }

    /// The object with this id as a typed [`GameRoomObject`]: the member its `type` selects.
    ///
    /// Copies the object, turning list properties that the merge holds index-keyed back into
    /// lists. The result is an error when the merged object does not match the reference
    /// schema of its type.
    #[must_use]
    pub fn typed_object(&self, id: &str) -> Option<Result<GameRoomObject, serde_json::Error>> {
        self.objects.get(id).map(typed)
    }

    /// Every object as `(id, typed object)`, in no particular order; see [`Self::typed_object`].
    pub fn typed_objects(
        &self,
    ) -> impl Iterator<Item = (&str, Result<GameRoomObject, serde_json::Error>)> {
        self.objects
            .iter()
            .map(|(id, object)| (id.as_str(), typed(object)))
    }

    /// Every user referenced by the room's objects as `(user id, user)`, ordered by id.
    pub fn users(&self) -> impl Iterator<Item = (&str, &RoomUser)> {
        self.users.iter().map(|(id, user)| (id.as_str(), user))
    }

    /// The user with this id.
    #[must_use]
    pub fn user(&self, id: &str) -> Option<&RoomUser> {
        self.users.get(id)
    }

    /// The subscribing user's flags in the room; empty without flags.
    ///
    /// # Errors
    ///
    /// Returns the first entry of [`Self::flags_raw`] that is not a valid flag.
    pub fn flags(&self) -> Result<Vec<Flag>, FlagError> {
        self.flags
            .as_deref()
            .map_or(Ok(Vec::new()), Flag::parse_all)
    }

    /// The subscribing user's flags as sent: entries joined by `|`, each
    /// `name~color~secondaryColor~x~y` (`|` and `~` inside a name written as `$VLINE$` and
    /// `$TILDE$`). `None` without flags.
    #[must_use]
    pub fn flags_raw(&self) -> Option<&str> {
        self.flags.as_deref()
    }

    /// Game tick of the latest update that carried one.
    #[must_use]
    pub const fn game_time(&self) -> Option<u64> {
        self.game_time
    }

    /// The subscribing user's room visuals of the previous tick, one item per line of
    /// [`Self::visual`]; a line that does not match the reference is an error item.
    pub fn visuals(&self) -> impl Iterator<Item = Result<RoomVisualItem, serde_json::Error>> {
        visual::items(&self.visual)
    }

    /// The subscribing user's room visuals of the previous tick as sent: newline-separated
    /// JSON items, empty when none.
    #[must_use]
    pub fn visual(&self) -> &str {
        &self.visual
    }

    /// Room mode (`world` for world rooms).
    #[must_use]
    pub fn mode(&self) -> Option<&str> {
        self.mode.as_deref()
    }

    /// Room decorations (official server only).
    #[must_use]
    pub fn decorations(&self) -> &[GameRoomDecoration] {
        &self.decorations
    }
}

/// A typed copy of a merged room object (see [`RoomState::typed_object`]).
fn typed(object: &Map<String, Value>) -> Result<GameRoomObject, serde_json::Error> {
    let mut object = object.clone();
    for key in LIST_PROPERTIES {
        if let Some(value) = object.get_mut(key)
            && let Value::Object(items) = value
        {
            let indexed: Option<Vec<(usize, &Value)>> = items
                .iter()
                .map(|(index, item)| index.parse().ok().map(|index| (index, item)))
                .collect();
            // Keys that are not all indices are left alone so the mismatch surfaces as an error.
            if let Some(mut indexed) = indexed {
                indexed.sort_unstable_by_key(|(index, _)| *index);
                let list = indexed.into_iter().map(|(_, item)| item.clone()).collect();
                *value = Value::Array(list);
            }
        }
    }
    serde_json::from_value(Value::Object(object))
}

/// Applies an `objects` diff to room objects keyed by id, exactly as the official client does
/// (see the [module documentation](self)).
///
/// Entries other than objects and `null` are not room objects: for a known id they are merged
/// like the client's `Object.entries` sees them (arrays by index, strings by UTF-16 code unit,
/// other values not at all); for an unknown id they are ignored, where the client would hold
/// the non-object value. Lone UTF-16 surrogates copied out of strings become U+FFFD.
pub fn apply_objects_diff<S: BuildHasher>(
    objects: &mut HashMap<String, Map<String, Value>, S>,
    diff: Map<String, Value>,
) {
    for (id, change) in diff {
        match objects.entry(id) {
            Entry::Vacant(entry) => {
                if let Value::Object(mut object) = change {
                    index_top_level_arrays(&mut object);
                    entry.insert(object);
                }
            }
            Entry::Occupied(entry) => {
                let mut source = match change {
                    Value::Null => {
                        entry.remove();
                        continue;
                    }
                    Value::Object(source) => source,
                    Value::Array(items) => index_keyed(items),
                    Value::String(text) => index_keyed(code_units(&text).collect()),
                    Value::Bool(_) | Value::Number(_) => continue,
                };
                let object = entry.into_mut();
                index_top_level_arrays(object);
                index_top_level_arrays(&mut source);
                merge_object(object, source);
            }
        }
    }
}

/// Client `N`: top-level arrays become objects keyed by array index.
fn index_top_level_arrays(object: &mut Map<String, Value>) {
    for value in object.values_mut() {
        if let Value::Array(items) = value {
            *value = Value::Object(index_keyed(mem::take(items)));
        }
    }
}

/// `{ ...array }`.
fn index_keyed(items: Vec<Value>) -> Map<String, Value> {
    items
        .into_iter()
        .enumerate()
        .map(|(index, item)| (index.to_string(), item))
        .collect()
}

/// The elements JavaScript indexes in a string: one-unit strings of its UTF-16 code units.
fn code_units(text: &str) -> impl Iterator<Item = Value> + '_ {
    text.encode_utf16().map(|unit| {
        let unit = char::from_u32(u32::from(unit)).unwrap_or(char::REPLACEMENT_CHARACTER);
        Value::String(unit.to_string())
    })
}

/// lodash `baseMerge` of a record source into a record.
fn merge_object(target: &mut Map<String, Value>, source: Map<String, Value>) {
    for (key, value) in source {
        let slot = target.entry(key).or_insert(Value::Null);
        let existing = mem::take(slot);
        *slot = merge_value(existing, value);
    }
}

/// lodash `baseMerge` of an array source into an array.
fn merge_array(target: &mut Vec<Value>, source: Vec<Value>) {
    for (index, value) in source.into_iter().enumerate() {
        if let Some(slot) = target.get_mut(index) {
            let existing = mem::take(slot);
            *slot = merge_value(existing, value);
        } else {
            // Indices are visited in order, so `index == target.len()` here.
            target.push(merge_value(Value::Null, value));
        }
    }
}

/// The value stored for a key after merging `source` into its `existing` value (`Null` when
/// absent): `baseMergeDeep` for containers, a plain write for everything else.
fn merge_value(existing: Value, source: Value) -> Value {
    match source {
        Value::Array(source) => {
            if existing.is_array() {
                // The client's customizer: an array merged into an array replaces it.
                return Value::Array(source);
            }
            let mut result = array_like_copy(existing);
            merge_array(&mut result, source);
            Value::Array(result)
        }
        Value::Object(source) => {
            let mut result = match existing {
                Value::Object(record) => record,
                _ => Map::new(),
            };
            merge_object(&mut result, source);
            Value::Object(result)
        }
        primitive => primitive,
    }
}

/// lodash `isArrayLike(value) ? arrayCopy(value) : []` for a non-array value: strings and
/// records with a valid integer `length` are copied index by index.
fn array_like_copy(value: Value) -> Vec<Value> {
    match value {
        Value::String(text) => code_units(&text).collect(),
        Value::Object(mut record) => {
            let Some(length) = record.get("length").and_then(safe_length) else {
                return Vec::new();
            };
            let mut items = Vec::new();
            for index in 0..length {
                items.push(record.remove(&index.to_string()).unwrap_or(Value::Null));
            }
            items
        }
        _ => Vec::new(),
    }
}

/// A JavaScript `length` that lodash `isLength` accepts: a safe non-negative integer.
fn safe_length(value: &Value) -> Option<u64> {
    let Value::Number(number) = value else {
        return None;
    };
    if let Some(length) = number.as_u64() {
        return (length <= MAX_SAFE_INTEGER).then_some(length);
    }
    let length = number.as_f64()?;
    #[expect(
        clippy::cast_precision_loss,
        reason = "MAX_SAFE_INTEGER converts to f64 exactly"
    )]
    let max = MAX_SAFE_INTEGER as f64;
    if length >= 0.0 && length.fract() == 0.0 && length <= max {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "checked to be a non-negative integer no larger than 2^53 - 1"
        )]
        Some(length as u64)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::{Map, Value, json};

    use super::{LIST_PROPERTIES, RoomState, apply_objects_diff};
    use crate::api::types::GameRoomObject;
    use crate::socket::types::{RoomEvent, RoomVisualItem};

    /// Applies each diff in turn to empty objects. Expected values below are the output of
    /// `../server/src/realtime/diff.ts` (`merge`/`indexTopLevelArrays`, the client's `U`) for the
    /// same diffs.
    fn replay(diffs: &[Value]) -> Value {
        let mut objects = HashMap::new();
        for diff in diffs {
            let Value::Object(diff) = diff.clone() else {
                panic!("diff must be an object")
            };
            apply_objects_diff(&mut objects, diff);
        }
        Value::Object(
            objects
                .into_iter()
                .map(|(id, object)| (id, Value::Object(object)))
                .collect(),
        )
    }

    #[test]
    fn top_level_arrays_are_index_keyed_and_merged_per_index() {
        let objects = replay(&[
            json!({"c1": {"_id": "c1", "body": [{"type": "move", "hits": 100}, {"type": "work", "hits": 100}],
                "effects": [{"effect": 1001, "ticksRemaining": 5}], "store": {"energy": 10, "H": 2}}}),
            json!({"c1": {"body": [{"hits": 50}], "store": {"energy": null},
                "effects": {"0": {"ticksRemaining": 4}}}}),
        ]);
        assert_eq!(
            objects,
            json!({"c1": {"_id": "c1", "body": {"0": {"type": "move", "hits": 50}, "1": {"type": "work", "hits": 100}},
                "effects": {"0": {"effect": 1001, "ticksRemaining": 4}}, "store": {"energy": null, "H": 2}}})
        );
    }

    #[test]
    fn nested_arrays_merged_into_arrays_are_replaced() {
        let objects = replay(&[
            json!({"s": {"_id": "s", "memory": {"path": [1, 2, 3], "tag": "a"}}}),
            json!({"s": {"memory": {"path": [4]}}}),
        ]);
        assert_eq!(
            objects,
            json!({"s": {"_id": "s", "memory": {"path": [4], "tag": "a"}}})
        );

        let objects = replay(&[
            json!({"o": {"_id": "o", "e": [{"p": [1, 2]}]}}),
            json!({"o": {"e": [{"p": [7]}], "x": []}}),
        ]);
        assert_eq!(
            objects,
            json!({"o": {"_id": "o", "e": {"0": {"p": [7]}}, "x": {}}})
        );
    }

    #[test]
    fn null_removes_known_objects_and_new_ids_are_inserted() {
        let objects = replay(&[
            json!({"a": {"_id": "a"}, "b": {"_id": "b"}}),
            json!({"a": null, "c": null, "d": {"_id": "d", "n": [5]}}),
        ]);
        assert_eq!(
            objects,
            json!({"b": {"_id": "b"}, "d": {"_id": "d", "n": {"0": 5}}})
        );
    }

    #[test]
    fn lodash_3_merge_of_mismatched_containers() {
        let objects = replay(&[
            json!({"o": {"_id": "o", "sign": "old", "nested": {"list": [1, 2], "text": "ab",
                "rec": {"length": 2, "0": "p", "1": "q", "extra": 1}, "bad": {"length": -1, "0": "p"}}}}),
            json!({"o": {"sign": {"text": "x"}, "nested": {"list": {"0": 9}, "text": ["X"],
                "rec": ["z"], "bad": ["y"], "fresh": [null, {"k": 1}]}}}),
        ]);
        assert_eq!(
            objects,
            json!({"o": {"_id": "o", "sign": {"text": "x"}, "nested": {"list": {"0": 9}, "text": ["X", "b"],
                "rec": ["z", "q"], "bad": ["y"], "fresh": [null, {"k": 1}]}}})
        );
    }

    #[test]
    fn arrays_merged_into_strings_copy_utf16_code_units() {
        let objects = replay(&[
            json!({"o": {"_id": "o", "n": {"s": "hé!"}}}),
            json!({"o": {"n": {"s": [{"a": 1}]}}}),
        ]);
        assert_eq!(
            objects,
            json!({"o": {"_id": "o", "n": {"s": [{"a": 1}, "é", "!"]}}})
        );
    }

    fn update(value: Value) -> RoomEvent {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn room_fields_replace_when_present_and_keep_when_absent() {
        let mut room = RoomState::default();
        room.apply(update(json!({
            "objects": {"a": {"_id": "a", "energy": 3000}},
            "users": {"2": {"_id": "2", "username": "Invader"}, "3": {"_id": "3", "username": "Source Keeper"}},
            "flags": "Flag1~1~2~10~20|Base$VLINE$2~5~5~25~30",
            "info": {"mode": "world"},
            "decorations": []
        })));
        room.apply(update(json!({
            "objects": {"a": {"energy": 2990}},
            "users": {"3": {"_id": "3", "username": "Keeper"}, "9": {"_id": "9", "username": "New"}},
            "gameTime": 71,
            "visual": "{\"t\":\"c\",\"x\":10,\"y\":12,\"s\":{\"radius\":0.5}}\n{\"t\":\"l\",\"x1\":1,\"y1\":2,\"x2\":3,\"y2\":4,\"s\":{}}\n"
        })));
        assert_eq!(
            room.object("a").and_then(|object| object.get("energy")),
            Some(&json!(2990))
        );
        assert_eq!(room.objects().count(), 1);
        assert_eq!(
            room.users().map(|(id, _)| id).collect::<Vec<_>>(),
            ["2", "3", "9"],
            "users extend per id"
        );
        assert_eq!(
            room.user("3").map(|user| user.username.as_str()),
            Some("Keeper")
        );
        assert_eq!(
            room.flags_raw(),
            Some("Flag1~1~2~10~20|Base$VLINE$2~5~5~25~30"),
            "absent flags are kept"
        );
        let flags = room.flags().unwrap();
        assert_eq!(
            flags
                .iter()
                .map(|flag| (
                    flag.name.as_str(),
                    flag.color,
                    flag.secondary_color,
                    flag.x,
                    flag.y
                ))
                .collect::<Vec<_>>(),
            [("Flag1", 1, 2, 10, 20), ("Base|2", 5, 5, 25, 30)]
        );
        assert_eq!(room.mode(), Some("world"));
        assert_eq!(room.game_time(), Some(71));
        let visuals: Vec<RoomVisualItem> = room.visuals().collect::<Result<_, _>>().unwrap();
        let [RoomVisualItem::Circle(circle), RoomVisualItem::Line(line)] = &visuals[..] else {
            panic!("not a circle and a line: {visuals:?}");
        };
        assert_eq!(
            (circle.x, circle.y, line.x1, line.y2),
            (10.0, 12.0, 1.0, 4.0)
        );
        assert_eq!(room.decorations(), []);

        room.apply(update(
            json!({"objects": {}, "flags": null, "gameTime": 72}),
        ));
        assert_eq!(room.flags_raw(), None, "null flags clear them");
        assert_eq!(room.flags().unwrap(), []);
        assert_eq!(room.visuals().count(), 2, "absent visuals are kept");
        assert_eq!(room.game_time(), Some(72));
    }

    #[test]
    fn malformed_visual_lines_and_flags_are_errors() {
        let mut room = RoomState::default();
        room.apply(update(json!({
            "objects": {},
            "flags": "ok~1~1~1~1|broken",
            "visual": "{\"t\":\"c\",\"x\":1,\"y\":1}\n{\"t\":\"?\"}"
        })));
        assert_eq!(room.flags().unwrap_err().entry, "broken");
        let visuals: Vec<_> = room.visuals().collect();
        assert!(visuals[0].is_ok());
        assert!(visuals[1].is_err());
    }

    #[test]
    fn non_object_entries_follow_the_client() {
        let mut objects = HashMap::new();
        let diff = |value: Value| -> Map<String, Value> { serde_json::from_value(value).unwrap() };
        apply_objects_diff(
            &mut objects,
            diff(json!({"a": {"_id": "a"}, "n": 5, "s": "new"})),
        );
        assert_eq!(
            objects.len(),
            1,
            "unknown ids with non-object values are not held"
        );
        apply_objects_diff(&mut objects, diff(json!({"a": "xy"})));
        apply_objects_diff(&mut objects, diff(json!({"a": [[1], 7]})));
        apply_objects_diff(&mut objects, diff(json!({"a": true})));
        assert_eq!(
            Value::Object(objects.remove("a").unwrap()),
            json!({"_id": "a", "0": {"0": 1}, "1": 7})
        );
    }

    /// The first event of a room: objects shaped like live `room:` events of each kind.
    fn objects() -> Value {
        json!({
            "c": {"_id": "c", "type": "creep", "room": "W1N1", "x": 1, "y": 2, "user": "u",
                  "name": "Harvester1", "hits": 200, "hitsMax": 200, "fatigue": 0,
                  "store": {"energy": 50}, "storeCapacity": 50, "ageTime": 1500,
                  "body": [{"type": "move", "hits": 100}, {"type": "work", "hits": 100, "boost": null}]},
            "s": {"_id": "s", "type": "spawn", "room": "W1N1", "x": 25, "y": 25, "user": "u",
                  "name": "Spawn1", "hits": 5000, "hitsMax": 5000, "notifyWhenAttacked": true,
                  "store": {"energy": 300}, "storeCapacityResource": {"energy": 300},
                  "spawning": null},
            "k": {"_id": "k", "type": "controller", "room": "W1N1", "x": 30, "y": 9, "user": "u",
                  "level": 3, "progress": 1200, "downgradeTime": 40000, "safeModeAvailable": 1,
                  "sign": {"user": "u", "text": "mine", "time": 70, "datetime": 1_700_000_000_000_u64}},
            "d": {"_id": "d", "type": "energy", "room": "W1N1", "x": 5, "y": 6,
                  "resourceType": "energy", "energy": 40},
            "o": {"_id": "o", "type": "scoreCollector", "room": "W1N1", "x": 7, "y": 8,
                  "strength": 3}
        })
    }

    #[test]
    fn typed_objects_select_the_member_by_type_from_merged_state() {
        let mut room = RoomState::default();
        room.apply(update(json!({"objects": objects()})));
        // The server sends list changes index-keyed; the merge holds the body index-keyed too.
        room.apply(update(json!({"objects": {
            "c": {"body": {"1": {"hits": 40}}, "fatigue": 2},
            "d": {"energy": 35}
        }})));
        let Some(Ok(GameRoomObject::Creep(creep))) = room.typed_object("c") else {
            panic!("not a creep: {:?}", room.typed_object("c"));
        };
        assert_eq!(creep.body.len(), 2);
        assert_eq!(
            (creep.body[1].r#type.as_str(), creep.body[1].hits),
            ("work", 40)
        );
        assert_eq!(creep.fatigue, Some(2));
        let Some(Ok(GameRoomObject::Spawn(spawn))) = room.typed_object("s") else {
            panic!("not a spawn");
        };
        assert_eq!(spawn.name.as_deref(), Some("Spawn1"));
        let Some(Ok(GameRoomObject::Controller(controller))) = room.typed_object("k") else {
            panic!("not a controller");
        };
        assert_eq!(controller.level, 3);
        let Some(Ok(GameRoomObject::Resource(resource))) = room.typed_object("d") else {
            panic!("not a dropped resource");
        };
        assert_eq!(resource.amount(), 35);
        let Some(Ok(GameRoomObject::Other(other))) = room.typed_object("o") else {
            panic!("unknown types are not the fallback");
        };
        assert_eq!(
            other.other["strength"],
            json!(3),
            "unknown properties are kept"
        );
        let mut kinds: Vec<(String, &str, u64, Option<String>)> = room
            .typed_objects()
            .map(|(id, object)| {
                let object = object.unwrap();
                assert_eq!(object.id(), id);
                assert_eq!(object.room(), "W1N1");
                (
                    object.kind().to_owned(),
                    id,
                    object.x(),
                    object.user().map(str::to_owned),
                )
            })
            .collect();
        kinds.sort_unstable();
        assert_eq!(
            kinds,
            [
                ("controller".to_owned(), "k", 30, Some("u".to_owned())),
                ("creep".to_owned(), "c", 1, Some("u".to_owned())),
                ("energy".to_owned(), "d", 5, None),
                ("scoreCollector".to_owned(), "o", 7, None),
                ("spawn".to_owned(), "s", 25, Some("u".to_owned())),
            ]
        );
        assert!(room.typed_object("missing").is_none());
    }

    #[test]
    fn typed_objects_report_merged_objects_that_break_their_schema() {
        let mut room = RoomState::default();
        room.apply(update(json!({"objects": objects()})));
        room.apply(update(json!({"objects": {"c": {"name": null}}})));
        assert!(
            room.typed_object("c").unwrap().is_err(),
            "a creep needs a name"
        );
        assert!(room.typed_object("s").unwrap().is_ok());
    }

    #[test]
    fn list_properties_match_the_reference() {
        let reference: Value =
            serde_json::from_str(include_str!("../../../../reference/openapi.json")).unwrap();
        let schemas = &reference["components"]["schemas"];
        let mut lists: Vec<&str> = Vec::new();
        for member in schemas["GameRoomObject"]["oneOf"].as_array().unwrap() {
            let mut properties = Vec::new();
            collect_properties(schemas, member, &mut properties);
            for (name, schema) in properties {
                if is_list(schemas, schema) && !lists.contains(&name) {
                    lists.push(name);
                }
            }
        }
        lists.sort_unstable();
        let mut expected = LIST_PROPERTIES.to_vec();
        expected.sort_unstable();
        assert_eq!(lists, expected);
    }

    /// Appends the properties a member schema declares, through `$ref` and `allOf`.
    fn collect_properties<'v>(
        schemas: &'v Value,
        schema: &'v Value,
        out: &mut Vec<(&'v str, &'v Value)>,
    ) {
        if let Some(reference) = schema["$ref"].as_str() {
            let name = reference.trim_start_matches("#/components/schemas/");
            return collect_properties(schemas, &schemas[name], out);
        }
        for part in schema["allOf"].as_array().into_iter().flatten() {
            collect_properties(schemas, part, out);
        }
        for (name, property) in schema["properties"].as_object().into_iter().flatten() {
            out.push((name.as_str(), property));
        }
    }

    /// Whether a reference schema admits a JSON array.
    fn is_list(schemas: &Value, schema: &Value) -> bool {
        if let Some(reference) = schema["$ref"].as_str() {
            let name = reference.trim_start_matches("#/components/schemas/");
            return is_list(schemas, &schemas[name]);
        }
        let types = &schema["type"];
        *types == "array"
            || types
                .as_array()
                .is_some_and(|types| types.iter().any(|kind| *kind == "array"))
            || ["anyOf", "oneOf"].iter().any(|key| {
                schema[key]
                    .as_array()
                    .is_some_and(|options| options.iter().any(|option| is_list(schemas, option)))
            })
    }
}
