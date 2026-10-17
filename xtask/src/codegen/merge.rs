//! Merging the official and the standalone document into schemas both servers satisfy.
//!
//! The standalone profile reuses official schemas but documents some differences: a property
//! only one side sends, a property of another type, or a response of another shape. Generated
//! types must deserialize replies from both kinds of server, so where the documents differ the
//! merged schema accepts either: objects merge into one object whose properties are the union
//! (required only when both sides require them), arrays merge into one array of the merged
//! items, schemas of nothing but a `type` merge into the union of their types, unions of
//! references merge into the union of the references, unions of as many members merge member
//! by member, and anything else becomes an `anyOf` of both.

use serde_json::{Map, Value, json};

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

/// The union of both documents' component schemas, merged by name where they differ.
pub fn components(official: &Components, standalone: &Components) -> Components {
    let mut merged = official.clone();
    for (name, schema) in standalone {
        let entry = match official.get(name) {
            Some(existing) => merge(existing, schema, official, standalone),
            None => schema.clone(),
        };
        merged.insert(name.clone(), entry);
    }
    merged
}

/// A schema accepting every value `a` (from the official document, resolved against `ca`) or
/// `b` (from the standalone document, resolved against `cb`) accepts.
pub fn merge(a: &Value, b: &Value, ca: &Components, cb: &Components) -> Value {
    if shape(a) == shape(b) {
        return a.clone();
    }
    if let (Some(left), Some(right)) = (ObjectShape::of(a, ca), ObjectShape::of(b, cb)) {
        let mut merged = left.merge(right, ca, cb);
        // A title names the Rust type or variant of the schema; keep it.
        if let Some(title) = a.get("title").or_else(|| b.get("title")) {
            merged["title"] = title.clone();
        }
        return merged;
    }
    if let Some(merged) = merge_arrays(a, b, ca, cb) {
        return merged;
    }
    if let Some(merged) = merge_types(a, b) {
        return merged;
    }
    if let Some(merged) = merge_unions(a, b, ca, cb) {
        return merged;
    }
    let mut variants = Vec::new();
    for side in [a, b] {
        let flattened = match side.get("anyOf").and_then(Value::as_array) {
            Some(items) if side.get("type").is_none() => items.clone(),
            _ => vec![side.clone()],
        };
        for variant in flattened {
            if !variants.iter().any(|known| shape(known) == shape(&variant)) {
                variants.push(variant);
            }
        }
    }
    let mut merged = Map::new();
    if let Some(description) = a.get("description").or_else(|| b.get("description")) {
        merged.insert("description".to_owned(), description.clone());
    }
    merged.insert("anyOf".to_owned(), Value::Array(variants));
    Value::Object(merged)
}

/// Two arrays that differ only in their items merged into one array whose items accept both
/// sides' items, documented by `a`, else by `b`. `None` for other pairs.
fn merge_arrays(a: &Value, b: &Value, ca: &Components, cb: &Components) -> Option<Value> {
    let is_array = |schema: &Value| schema.get("type").and_then(Value::as_str) == Some("array");
    if !is_array(a) || !is_array(b) {
        return None;
    }
    let (left, right) = (a.get("items")?, b.get("items")?);
    let without_items = |schema: &Value| {
        let mut rest = shape(schema);
        rest.as_object_mut()?.remove("items");
        Some(rest)
    };
    if without_items(a)? != without_items(b)? {
        return None;
    }
    let mut merged = a.clone();
    merged["items"] = merge(left, right, ca, cb);
    if a.get("description").is_none()
        && let Some(description) = b.get("description")
    {
        merged["description"] = description.clone();
    }
    Some(merged)
}

/// Two schemas constraining nothing but `type` (a name or a list of names) merged into one
/// whose `type` lists both sides' types, documented by `a`, else by `b`. `None` for other pairs.
fn merge_types(a: &Value, b: &Value) -> Option<Value> {
    let types = |schema: &Value| -> Option<Vec<Value>> {
        let shape = shape(schema);
        let map = shape.as_object()?;
        if map.len() != 1 {
            return None;
        }
        match map.get("type")? {
            name @ Value::String(_) => Some(vec![name.clone()]),
            Value::Array(names) if names.iter().all(Value::is_string) => Some(names.clone()),
            _ => None,
        }
    };
    let mut union = types(a)?;
    for name in types(b)? {
        if !union.contains(&name) {
            union.push(name);
        }
    }
    let mut merged = a.clone();
    merged["type"] = Value::Array(union);
    if a.get("description").is_none()
        && let Some(description) = b.get("description")
    {
        merged["description"] = description.clone();
    }
    Some(merged)
}

/// The `oneOf`/`anyOf` keyword and members of a union schema.
fn union_members(schema: &Value) -> Option<(&'static str, &Vec<Value>)> {
    if schema.get("type").is_some() {
        return None;
    }
    ["oneOf", "anyOf"].into_iter().find_map(|keyword| {
        schema
            .get(keyword)
            .and_then(Value::as_array)
            .map(|members| (keyword, members))
    })
}

/// Two unions merged into one with `a`'s keyword and annotations: references to the same
/// component name the same merged component, so unions of references keep every reference;
/// other unions of as many members merge member by member. `None` for other pairs.
fn merge_unions(a: &Value, b: &Value, ca: &Components, cb: &Components) -> Option<Value> {
    let (keyword, left) = union_members(a)?;
    let (_, right) = union_members(b)?;
    let references = left
        .iter()
        .chain(right)
        .all(|member| member.get("$ref").is_some());
    let members: Vec<Value> = if references {
        let mut members = left.clone();
        for member in right {
            if !members.contains(member) {
                members.push(member.clone());
            }
        }
        members
    } else if left.len() == right.len() {
        left.iter()
            .zip(right)
            .map(|(left, right)| merge(left, right, ca, cb))
            .collect()
    } else {
        return None;
    };
    let mut merged = a.as_object()?.clone();
    merged.insert(keyword.to_owned(), Value::Array(members));
    if let (Some(left), Some(right)) = (
        a.pointer("/discriminator/mapping")
            .and_then(Value::as_object),
        b.pointer("/discriminator/mapping")
            .and_then(Value::as_object),
    ) {
        let mut mapping = left.clone();
        for (value, reference) in right {
            mapping
                .entry(value.clone())
                .or_insert_with(|| reference.clone());
        }
        merged["discriminator"]["mapping"] = Value::Object(mapping);
    }
    Some(Value::Object(merged))
}

/// The properties of an object schema, collected through `$ref` and `allOf` (ignoring the
/// `if`/`then` members of an `allOf`).
#[derive(Debug, Default)]
pub struct ObjectShape {
    /// Documentation of the outermost schema.
    pub description: Option<Value>,
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
        let mut shape = Self {
            description: schema.get("description").cloned(),
            ..Self::default()
        };
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
            // `{type: object}` with optional extra fields: an empty object, mergeable.
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

    /// One object accepting both shapes.
    fn merge(self, other: Self, ca: &Components, cb: &Components) -> Value {
        let mut properties = Map::new();
        for (name, left) in &self.properties {
            let merged = match other.properties.get(name) {
                Some(right) => merge(left, right, ca, cb),
                None => left.clone(),
            };
            properties.insert(name.clone(), merged);
        }
        for (name, right) in &other.properties {
            if !properties.contains_key(name) {
                properties.insert(name.clone(), right.clone());
            }
        }
        let required: Vec<&String> = self
            .required
            .iter()
            .filter(|name| other.required.contains(name))
            .collect();
        let additional = match (&self.additional, &other.additional) {
            (Some(left), Some(right)) => merge(left, right, ca, cb),
            (Some(one), None) | (None, Some(one)) => one.clone(),
            (None, None) => Value::Bool(true),
        };
        let mut merged = json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": additional,
        });
        if let Some(description) = self.description.or(other.description) {
            merged["description"] = description;
        }
        merged
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, json};

    use super::{components, merge};

    #[test]
    fn objects_merge_with_required_only_when_both_require() {
        let official = json!({
            "allOf": [{"$ref": "#/components/schemas/Ok"}],
            "description": "Ack."
        });
        let standalone = json!({
            "allOf": [{"$ref": "#/components/schemas/Ok"}, {"$ref": "#/components/schemas/Creep"}]
        });
        let mut ca = Map::new();
        ca.insert(
            "Ok".to_owned(),
            json!({"type": "object", "properties": {"ok": {"const": 1}}, "required": ["ok"]}),
        );
        let mut cb = ca.clone();
        cb.insert(
            "Creep".to_owned(),
            json!({"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]}),
        );
        let merged = merge(&official, &standalone, &ca, &cb);
        assert_eq!(merged["description"], "Ack.");
        assert_eq!(merged["required"], json!(["ok"]));
        assert_eq!(merged["properties"]["name"], json!({"type": "string"}));
    }

    #[test]
    fn unions_merge_by_reference_or_member_by_member() {
        let empty = Map::new();
        let reference = |name: &str| json!({"$ref": format!("#/components/schemas/{name}")});
        let merged = merge(
            &json!({"oneOf": [reference("A"), reference("B")],
                "discriminator": {"propertyName": "type", "mapping": {"a": "#/components/schemas/A"}}}),
            &json!({"oneOf": [reference("B"), reference("C")],
                "discriminator": {"propertyName": "type", "mapping": {"c": "#/components/schemas/C"}}}),
            &empty,
            &empty,
        );
        assert_eq!(
            merged["oneOf"],
            json!([reference("A"), reference("B"), reference("C")])
        );
        assert_eq!(
            merged["discriminator"]["mapping"],
            json!({"a": "#/components/schemas/A", "c": "#/components/schemas/C"})
        );
        // The standalone profile drops `shard` from both forms of a console event.
        let output = |required: serde_json::Value| {
            json!({"type": "object", "required": required,
                "properties": {"messages": {"type": "string"}, "shard": {"type": "string"}}})
        };
        let merged = merge(
            &json!({"oneOf": [output(json!(["messages", "shard"])), {"type": "object", "required": ["error"]}]}),
            &json!({"oneOf": [output(json!(["messages"])), {"type": "object", "required": ["error"], "title": "x"}]}),
            &empty,
            &empty,
        );
        assert_eq!(merged["oneOf"].as_array().unwrap().len(), 2);
        assert_eq!(merged["oneOf"][0]["required"], json!(["messages"]));
    }

    #[test]
    fn arrays_merge_their_items_and_bare_types_their_type_lists() {
        let empty = Map::new();
        // The standalone profile lets shard lists hold `null` (an unset `shardName`).
        let merged = merge(
            &json!({"type": "array", "items": {"type": "string"}}),
            &json!({"type": "array", "items": {"type": ["string", "null"]}, "description": "Names."}),
            &empty,
            &empty,
        );
        assert_eq!(
            merged,
            json!({"type": "array", "items": {"type": ["string", "null"]}, "description": "Names."})
        );
        // Other array constraints differ: no single array accepts both.
        let merged = merge(
            &json!({"type": "array", "items": {"type": "string"}, "maxItems": 1}),
            &json!({"type": "array", "items": {"type": "integer"}}),
            &empty,
            &empty,
        );
        assert_eq!(merged["anyOf"].as_array().unwrap().len(), 2);
        // A constrained type is not a bare type.
        let merged = merge(
            &json!({"type": "integer", "minimum": 0}),
            &json!({"type": "string"}),
            &empty,
            &empty,
        );
        assert_eq!(merged["anyOf"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn other_differences_become_any_of_both() {
        let empty = Map::new();
        let merged = merge(
            &json!({"type": "array", "items": {"type": "integer"}}),
            &json!({"type": "boolean", "description": "Owned."}),
            &empty,
            &empty,
        );
        assert_eq!(
            merged["anyOf"],
            json!([{"type": "array", "items": {"type": "integer"}}, {"type": "boolean", "description": "Owned."}])
        );
        let mut official = Map::new();
        official.insert(
            "Same".to_owned(),
            json!({"type": "string", "description": "a"}),
        );
        let mut standalone = Map::new();
        standalone.insert(
            "Same".to_owned(),
            json!({"type": "string", "description": "b"}),
        );
        assert_eq!(
            components(&official, &standalone)["Same"]["description"],
            "a"
        );
    }
}
