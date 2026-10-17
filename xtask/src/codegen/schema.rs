//! Reading JSON schemas: component maps, references, shapes for comparison and the properties of
//! object schemas.

use serde_json::{Map, Value};

/// Component schemas of a document, by name.
pub type Components = Map<String, Value>;

/// The schema `value` with documentation stripped, for comparing shapes.
pub fn shape(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| !is_annotation(key))
                .map(|(key, value)| (key.clone(), shape(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(shape).collect()),
        other => other.clone(),
    }
}

/// Keys that document a schema without constraining it.
fn is_annotation(key: &str) -> bool {
    matches!(
        key,
        "description" | "summary" | "title" | "examples" | "example"
    ) || key.starts_with("x-")
}

/// The component name a `$ref` points to.
pub fn ref_name(reference: &str) -> Option<&str> {
    reference.strip_prefix("#/components/schemas/")
}

/// The properties of an object schema, collected through `$ref` and `allOf` (ignoring the
/// `if`/`then` members of an `allOf`).
#[derive(Debug, Default)]
pub struct ObjectShape {
    /// Properties, in document order of first appearance.
    pub properties: Map<String, Value>,
    /// Required property names.
    pub required: Vec<String>,
    /// The `additionalProperties` schema, when one is given.
    pub additional: Option<Value>,
}

impl ObjectShape {
    /// The object shape of `schema`, or `None` when it is not an object with properties.
    pub fn of(schema: &Value, components: &Components) -> Option<Self> {
        let mut shape = Self::default();
        shape.collect(schema, components, 0).then_some(shape)
    }

    /// Adds the properties of `schema`; returns whether it is object-like.
    fn collect(&mut self, schema: &Value, components: &Components, depth: usize) -> bool {
        if depth > 32 {
            return false;
        }
        if let Some(target) = schema
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(ref_name)
        {
            return components
                .get(target)
                .is_some_and(|target| self.collect(target, components, depth + 1));
        }
        if schema.get("anyOf").is_some() || schema.get("oneOf").is_some() {
            return false;
        }
        let mut object_like = false;
        if let Some(members) = schema.get("allOf").and_then(Value::as_array) {
            for member in members.iter().filter(|member| member.get("if").is_none()) {
                if !self.collect(member, components, depth + 1) {
                    return false;
                }
                object_like = true;
            }
        }
        let typed_object = schema.get("type").and_then(Value::as_str) == Some("object");
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for (name, property) in properties {
                self.properties
                    .entry(name.clone())
                    .or_insert_with(|| property.clone());
            }
            object_like = true;
        } else if typed_object
            && schema
                .get("additionalProperties")
                .is_none_or(Value::is_boolean)
        {
            // `{type: object}` with optional extra fields: an empty object.
            object_like = true;
        }
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for name in required.iter().filter_map(Value::as_str) {
                if !self.required.iter().any(|known| known == name) {
                    self.required.push(name.to_owned());
                }
            }
        }
        if let Some(additional) = schema
            .get("additionalProperties")
            .filter(|value| value.is_object())
        {
            if !object_like {
                // A map, not an object with properties.
                return false;
            }
            self.additional = Some(additional.clone());
        }
        object_like
    }
}
