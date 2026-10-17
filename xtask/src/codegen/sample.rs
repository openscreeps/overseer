//! Sample values for the generated request-building tests.

use serde_json::{Map, Value};

use super::schema::{Components, ObjectShape, ref_name};

/// The sample string of string parameters: needs escaping in queries and paths.
pub const STRING: &str = "a b/é";

/// A minimal JSON instance of `schema`: required properties only, the first alternative of a
/// union, the first value of an enum.
pub fn instance(schema: &Value, components: &Components) -> Value {
    instance_at(schema, components, 0)
}

fn instance_at(schema: &Value, components: &Components, depth: usize) -> Value {
    if depth > 24 {
        return Value::Null;
    }
    let Some(map) = schema.as_object() else {
        return Value::Null;
    };
    if let Some(target) = map.get("$ref").and_then(Value::as_str).and_then(ref_name) {
        return components.get(target).map_or(Value::Null, |target| {
            instance_at(target, components, depth + 1)
        });
    }
    if let Some(variants) = map
        .get("anyOf")
        .or_else(|| map.get("oneOf"))
        .and_then(Value::as_array)
    {
        let not_null = variants
            .iter()
            .find(|variant| variant.get("type").and_then(Value::as_str) != Some("null"));
        return not_null.map_or(Value::Null, |variant| {
            instance_at(variant, components, depth + 1)
        });
    }
    if let Some(members) = map.get("allOf").and_then(Value::as_array) {
        let members: Vec<&Value> = members
            .iter()
            .filter(|member| member.get("if").is_none())
            .collect();
        if let ([member], None) = (members.as_slice(), map.get("properties")) {
            // A `$ref` with its own description, as the model lowers it.
            return instance_at(member, components, depth + 1);
        }
    }
    if map.contains_key("allOf") || map.contains_key("properties") {
        let Some(object) = ObjectShape::of(schema, components) else {
            return Value::Null;
        };
        let mut instance = Map::new();
        for name in &object.required {
            if let Some(property) = object.properties.get(name) {
                instance.insert(name.clone(), instance_at(property, components, depth + 1));
            }
        }
        return Value::Object(instance);
    }
    if let Some(first) = map
        .get("enum")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
    {
        return first.clone();
    }
    if let Some(value) = map.get("const") {
        return value.clone();
    }
    let ty = match map.get("type") {
        Some(Value::Array(types)) => types
            .iter()
            .filter_map(Value::as_str)
            .find(|ty| *ty != "null"),
        Some(ty) => ty.as_str(),
        None => None,
    };
    let minimum = map.get("minimum");
    match ty {
        Some("string") => Value::from("x"),
        Some("integer") => Value::from(minimum.and_then(Value::as_i64).unwrap_or(0)),
        Some("number") => Value::from(minimum.and_then(Value::as_f64).unwrap_or(0.0)),
        Some("boolean") => Value::Bool(false),
        Some("array") => Value::Array(Vec::new()),
        Some("object") => Value::Object(Map::new()),
        _ => Value::Null,
    }
}

/// `value` encoded as `application/x-www-form-urlencoded`, as `url` encodes query pairs.
pub fn form_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                out.push(char::from(byte));
            }
            b' ' => out.push('+'),
            other => push_escaped(&mut out, other),
        }
    }
    out
}

/// `value` percent-encoded as one path segment: everything but unreserved characters.
pub fn segment_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            other => push_escaped(&mut out, other),
        }
    }
    out
}

/// Appends `%XX` with upper-case hex digits.
fn push_escaped(out: &mut String, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out.push('%');
    out.push(char::from(HEX[usize::from(byte >> 4)]));
    out.push(char::from(HEX[usize::from(byte & 0x0f)]));
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, json};

    use super::{form_encode, instance, segment_encode};

    #[test]
    fn instances_hold_required_properties_only() {
        let mut components = Map::new();
        components.insert(
            "Name".to_owned(),
            json!({"type": "string", "enum": ["b", "c"]}),
        );
        let schema = json!({
            "type": "object",
            "properties": {
                "name": {"$ref": "#/components/schemas/Name"},
                "count": {"type": "integer", "minimum": 1},
                "list": {"anyOf": [{"type": "null"}, {"type": "array"}]},
                "skipped": {"type": "string"}
            },
            "required": ["name", "count", "list"]
        });
        assert_eq!(
            instance(&schema, &components),
            json!({"name": "b", "count": 1, "list": []})
        );
    }

    #[test]
    fn encoders_match_query_and_path_rules() {
        assert_eq!(form_encode("a b/é&"), "a+b%2F%C3%A9%26");
        assert_eq!(segment_encode("a b/é~"), "a%20b%2F%C3%A9~");
    }
}
