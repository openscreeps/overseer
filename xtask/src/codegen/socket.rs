//! Choices for the socket document's types: which schemas are the HTTP types, which stay raw.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::merge::{Components, ref_name, shape};
use super::model::Options;
use crate::Error;

/// Socket schemas generated as raw JSON objects. A room object diff is applied with the
/// official client's merge, which needs the patches exactly as sent: explicit `null`s, unknown
/// keys and index-keyed lists would not survive a typed round trip.
const RAW: &[&str] = &["RoomObjectsDiff"];

/// The prefix of socket room object schemas, which the HTTP reference names `Game` + name.
const ROOM_OBJECT: &str = "RoomObject";

/// The options of the socket model: `null` differs from absent (events are changes), the raw
/// schemas, and every socket schema the HTTP types already define (see [`shared`]).
pub fn options(socket: &Components, http: &Components) -> Result<Options, Error> {
    Ok(Options {
        absent_vs_null: true,
        raw: RAW.iter().map(|name| (*name).to_owned()).collect(),
        external: shared(socket, http)?
            .into_iter()
            .map(|(name, counterpart)| (name, format!("api::{counterpart}")))
            .collect(),
    })
}

/// The HTTP schema a socket schema may duplicate: `GameRoomObject…` for `RoomObject…`, the
/// same name otherwise.
fn counterpart(name: &str) -> String {
    if name.starts_with(ROOM_OBJECT) {
        format!("Game{name}")
    } else {
        name.to_owned()
    }
}

/// Socket schema names mapped to the HTTP schema with the same Rust type: the counterpart is
/// identical (ignoring documentation, the form of `discriminator`, and with references mapped
/// the same way) and every schema it references is mapped too. A `RoomObject…` schema whose
/// counterpart exists but differs is an error, so the two references cannot drift apart.
fn shared(socket: &Components, http: &Components) -> Result<BTreeMap<String, String>, Error> {
    let mut mapped: BTreeSet<&String> = socket
        .iter()
        .filter(|(name, _)| !RAW.contains(&name.as_str()))
        .filter(|(name, schema)| {
            http.get(&counterpart(name))
                .is_some_and(|other| comparable(&renamed(schema)) == comparable(other))
        })
        .map(|(name, _)| name)
        .collect();
    loop {
        let unmapped: Vec<&String> = mapped
            .iter()
            .filter(|name| {
                let mut targets = Vec::new();
                references(&socket[name.as_str()], &mut targets);
                targets
                    .iter()
                    .any(|target| !mapped.iter().any(|known| known.as_str() == *target))
            })
            .copied()
            .collect();
        if unmapped.is_empty() {
            break;
        }
        for name in unmapped {
            mapped.remove(name);
        }
    }
    let diverged: Vec<&str> = socket
        .keys()
        .filter(|name| name.starts_with(ROOM_OBJECT) && http.contains_key(&counterpart(name)))
        .filter(|name| !mapped.contains(name))
        .map(String::as_str)
        .collect();
    if !diverged.is_empty() {
        return Err(format!(
            "socket schemas differ from their HTTP counterparts (`Game` + name): {}",
            diverged.join(", ")
        )
        .into());
    }
    Ok(mapped
        .into_iter()
        .map(|name| (name.clone(), counterpart(name)))
        .collect())
}

/// `schema` with every reference pointing at the counterpart of its target.
fn renamed(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let value = match (key.as_str(), value.as_str().and_then(ref_name)) {
                        ("$ref", Some(target)) => {
                            Value::from(format!("#/components/schemas/{}", counterpart(target)))
                        }
                        _ => renamed(value),
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(renamed).collect()),
        other => other.clone(),
    }
}

/// The shape of a schema with `discriminator` reduced to its property name: `OpenAPI` writes
/// `{propertyName, mapping}` (the mapping repeats the members' values), `AsyncAPI` 3 the name.
fn comparable(schema: &Value) -> Value {
    match shape(schema) {
        Value::Object(mut map) => {
            if let Some(discriminator) = map.get_mut("discriminator")
                && let Some(property) = discriminator.get("propertyName").cloned()
            {
                *discriminator = property;
            }
            Value::Object(
                map.into_iter()
                    .map(|(key, value)| (key, comparable(&value)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(comparable).collect()),
        other => other,
    }
}

/// Appends the component names `schema` references.
fn references<'v>(schema: &'v Value, out: &mut Vec<&'v str>) {
    match schema {
        Value::Object(map) => {
            for (key, value) in map {
                match (key.as_str(), value.as_str().and_then(ref_name)) {
                    ("$ref", Some(target)) => out.push(target),
                    _ => references(value, out),
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                references(item, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value, json};

    use super::shared;

    fn components(value: Value) -> Map<String, Value> {
        let Value::Object(map) = value else {
            panic!("components must be an object")
        };
        map
    }

    #[test]
    fn identical_room_objects_map_and_diverging_ones_fail() {
        let http = components(json!({
            "GameRoomObject": {"oneOf": [{"$ref": "#/components/schemas/GameRoomObjectStore"}],
                "discriminator": {"propertyName": "type", "mapping": {"x": "#/components/schemas/GameRoomObjectStore"}}},
            "GameRoomObjectStore": {"type": "object", "description": "HTTP.", "additionalProperties": {"type": "integer"}},
            "UserMessage": {"type": "object", "properties": {"_id": {"type": "string"}}, "required": ["_id"]}
        }));
        let socket = components(json!({
            "RoomObject": {"oneOf": [{"$ref": "#/components/schemas/RoomObjectStore"}], "discriminator": "type"},
            "RoomObjectStore": {"type": "object", "description": "Socket.", "additionalProperties": {"type": "integer"}},
            "RoomObjectPatch": {"type": "object", "properties": {"store": {"$ref": "#/components/schemas/RoomObjectStore"}}},
            "UserMessage": {"type": "object", "properties": {"_id": {"type": "string"}}},
            "RoomObjectsDiff": {"type": "object", "additionalProperties": {"$ref": "#/components/schemas/RoomObject"}}
        }));
        let mapped = shared(&socket, &http).unwrap();
        assert_eq!(
            mapped.into_iter().collect::<Vec<_>>(),
            [
                ("RoomObject".to_owned(), "GameRoomObject".to_owned()),
                (
                    "RoomObjectStore".to_owned(),
                    "GameRoomObjectStore".to_owned()
                ),
            ]
        );
        let mut diverging = socket;
        diverging["RoomObjectStore"]["additionalProperties"] = json!({"type": "number"});
        let error = shared(&diverging, &http).unwrap_err().to_string();
        assert!(error.contains("RoomObject, RoomObjectStore"), "{error}");
    }
}
