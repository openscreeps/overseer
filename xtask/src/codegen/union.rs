//! Discriminated unions: `oneOf`/`anyOf` members told apart by the value of one property.
//!
//! The property is the `OpenAPI` `discriminator.propertyName` or, without one, a property every
//! member constrains to fixed strings. A member selects its values with `const` or `enum` (or
//! through the discriminator `mapping`); at most one member takes every other value, declared
//! as `not: {enum: [...]}`.

use serde_json::Value;

use super::schema::{Components, ref_name};

/// How the members of a union are told apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discriminator {
    /// The JSON property naming the member.
    pub property: String,
    /// Per member, in member order: the property values selecting it (empty for the fallback).
    pub values: Vec<Vec<String>>,
    /// The member taking every other value, if any.
    pub fallback: Option<usize>,
}

/// What one member requires of the discriminating property.
enum Selects {
    Values(Vec<String>),
    Fallback,
}

/// The discriminator of the union `owner` (the schema holding the `oneOf`/`anyOf`, for its
/// `discriminator` keyword) with `members` (nested unions flattened, `null` removed), or `None`
/// when the members are not objects told apart by one property.
pub fn discriminator(
    owner: &Value,
    members: &[Value],
    components: &Components,
) -> Option<Discriminator> {
    if members.len() < 2 {
        return None;
    }
    // OpenAPI writes `discriminator: {propertyName}`, AsyncAPI 3 `discriminator: <name>`.
    let explicit = owner
        .pointer("/discriminator/propertyName")
        .or_else(|| owner.get("discriminator"))
        .and_then(Value::as_str);
    let candidates: Vec<String> = if let Some(property) = explicit {
        vec![property.to_owned()]
    } else {
        let mut names = Vec::new();
        property_names(&members[0], components, 0, &mut names);
        names
    };
    candidates
        .into_iter()
        .find_map(|property| by_property(owner, members, components, property))
}

/// The discriminator when `property` tells every member apart.
fn by_property(
    owner: &Value,
    members: &[Value],
    components: &Components,
    property: String,
) -> Option<Discriminator> {
    let mapping = owner
        .pointer("/discriminator/mapping")
        .and_then(Value::as_object);
    let mut values = Vec::new();
    let mut fallback = None;
    for (index, member) in members.iter().enumerate() {
        let mut constraints = Vec::new();
        if !constraints_of(member, &property, components, 0, &mut constraints) {
            return None;
        }
        let selects = selects(&constraints);
        let mut own = match selects {
            Some(Selects::Values(own)) => own,
            Some(Selects::Fallback) => {
                if fallback.replace(index).is_some() {
                    return None;
                }
                values.push(Vec::new());
                continue;
            }
            None => Vec::new(),
        };
        let target = member
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(ref_name);
        if let (Some(mapping), Some(target)) = (mapping, target) {
            for (value, reference) in mapping {
                let points_here = reference.as_str().and_then(ref_name) == Some(target);
                if points_here && !own.contains(value) {
                    own.push(value.clone());
                }
            }
        }
        if own.is_empty() {
            return None;
        }
        values.push(own);
    }
    let mut seen: Vec<&String> = Vec::new();
    for value in values.iter().flatten() {
        if seen.contains(&value) {
            return None;
        }
        seen.push(value);
    }
    Some(Discriminator {
        property,
        values,
        fallback,
    })
}

/// The values a member's constraints on the property select.
fn selects(constraints: &[&Value]) -> Option<Selects> {
    for constraint in constraints {
        if let Some(value) = constraint.get("const").and_then(Value::as_str) {
            return Some(Selects::Values(vec![value.to_owned()]));
        }
        if let Some(values) = constraint.get("enum").and_then(Value::as_array) {
            let strings: Option<Vec<String>> = values
                .iter()
                .map(|value| value.as_str().map(str::to_owned))
                .collect();
            return strings.map(Selects::Values);
        }
    }
    constraints
        .iter()
        .any(|constraint| constraint.pointer("/not/enum").is_some())
        .then_some(Selects::Fallback)
}

/// Appends the schemas `member` (through `$ref` and `allOf`, ignoring `if`/`then` members)
/// declares for `property`; returns whether the member is object-like.
fn constraints_of<'v>(
    member: &'v Value,
    property: &str,
    components: &'v Components,
    depth: usize,
    out: &mut Vec<&'v Value>,
) -> bool {
    if depth > 32 {
        return false;
    }
    if let Some(target) = member
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(ref_name)
    {
        return components
            .get(target)
            .is_some_and(|target| constraints_of(target, property, components, depth + 1, out));
    }
    if member.get("anyOf").is_some() || member.get("oneOf").is_some() {
        return false;
    }
    let mut object_like = member.get("type").and_then(Value::as_str) == Some("object");
    if let Some(members) = member.get("allOf").and_then(Value::as_array) {
        for part in members.iter().filter(|part| part.get("if").is_none()) {
            if !constraints_of(part, property, components, depth + 1, out) {
                return false;
            }
            object_like = true;
        }
    }
    if let Some(properties) = member.get("properties").and_then(Value::as_object) {
        object_like = true;
        if let Some(schema) = properties.get(property) {
            out.push(schema);
        }
    }
    object_like
}

/// Appends the property names `member` declares, in document order.
fn property_names(member: &Value, components: &Components, depth: usize, out: &mut Vec<String>) {
    if depth > 32 {
        return;
    }
    if let Some(target) = member
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(ref_name)
    {
        if let Some(target) = components.get(target) {
            property_names(target, components, depth + 1, out);
        }
        return;
    }
    for part in member
        .get("allOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        property_names(part, components, depth + 1, out);
    }
    for name in member
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|properties| properties.keys())
    {
        if !out.contains(name) {
            out.push(name.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value, json};

    use super::{Discriminator, discriminator};

    fn components(value: Value) -> Map<String, Value> {
        let Value::Object(map) = value else {
            panic!("components must be an object")
        };
        map
    }

    #[test]
    fn explicit_discriminators_use_const_enum_mapping_and_not_enum() {
        let components = components(json!({
            "Base": {"type": "object", "properties": {"type": {"type": "string"}, "x": {"type": "integer"}}},
            "Creep": {"allOf": [{"$ref": "#/components/schemas/Base"},
                {"type": "object", "properties": {"type": {"const": "creep"}}}]},
            "Drop": {"allOf": [{"$ref": "#/components/schemas/Base"},
                {"type": "object", "properties": {"type": {"enum": ["energy", "power"]}}}]},
            "Mapped": {"allOf": [{"$ref": "#/components/schemas/Base"}]},
            "Other": {"allOf": [{"$ref": "#/components/schemas/Base"},
                {"type": "object", "properties": {"type": {"not": {"enum": ["creep", "energy", "power", "lab"]}}}}]}
        }));
        let owner = json!({
            "oneOf": [],
            "discriminator": {"propertyName": "type", "mapping": {
                "creep": "#/components/schemas/Creep", "lab": "#/components/schemas/Mapped"
            }}
        });
        let members: Vec<Value> = ["Creep", "Drop", "Mapped", "Other"]
            .iter()
            .map(|name| json!({"$ref": format!("#/components/schemas/{name}")}))
            .collect();
        assert_eq!(
            discriminator(&owner, &members, &components),
            Some(Discriminator {
                property: "type".to_owned(),
                values: vec![
                    vec!["creep".to_owned()],
                    vec!["energy".to_owned(), "power".to_owned()],
                    vec!["lab".to_owned()],
                    vec![],
                ],
                fallback: Some(3),
            })
        );
    }

    #[test]
    fn implicit_discriminators_need_disjoint_fixed_values() {
        let empty = Map::new();
        let line = json!({"type": "object", "properties": {"t": {"const": "l"}, "x1": {"type": "number"}}});
        let circle =
            json!({"type": "object", "properties": {"x": {"type": "number"}, "t": {"const": "c"}}});
        let found = discriminator(&json!({}), &[line.clone(), circle], &empty).unwrap();
        assert_eq!(found.property, "t");
        assert_eq!(found.fallback, None);
        let duplicate = json!({"type": "object", "properties": {"t": {"const": "l"}}});
        assert_eq!(
            discriminator(&json!({}), &[line.clone(), duplicate], &empty),
            None
        );
        let free = json!({"type": "object", "properties": {"t": {"type": "string"}}});
        assert_eq!(
            discriminator(&json!({}), &[line.clone(), free], &empty),
            None
        );
        assert_eq!(
            discriminator(&json!({}), &[line, json!({"type": "string"})], &empty),
            None
        );
    }
}
