//! Tests of the lowering of JSON Schema into a [`Model`]: objects, maps, enums, unions,
//! aliases, recursion and inline names.

use serde_json::{Map, Value, json};

use super::{Body, Model, Ty};

/// Lowers `schemas` as components.
fn model(schemas: Value) -> Model {
    let Value::Object(components) = schemas else {
        panic!("components must be an object");
    };
    let mut model = Model::new(components).unwrap();
    model.finish().unwrap();
    model
}

fn fields(model: &Model, name: &str) -> Vec<(String, String, Ty)> {
    let Body::Struct(fields) = &model.defs[name].body else {
        panic!("{name} is not a struct");
    };
    fields
        .iter()
        .map(|field| (field.json.clone(), field.ident.clone(), field.ty.clone()))
        .collect()
}

fn named(name: &str) -> Ty {
    Ty::Named(name.to_owned())
}

fn option(ty: Ty) -> Ty {
    Ty::Option(Box::new(ty))
}

#[test]
fn objects_merge_all_of_drop_ok_and_keep_required() {
    let model = model(json!({
        "Ok": {"type": "object", "properties": {"ok": {"type": "integer", "const": 1}}, "required": ["ok"]},
        "Reply": {"allOf": [
            {"$ref": "#/components/schemas/Ok"},
            {"type": "object", "properties": {
                "_id": {"type": "string"},
                "type": {"type": "string"},
                "count": {"type": "integer", "minimum": 0},
                "ratio": {"type": ["number", "null"]}
            }, "required": ["_id"]},
            {"if": {"properties": {"type": {"const": "x"}}}, "then": {"required": ["count"]}}
        ]}
    }));
    assert_eq!(
        fields(&model, "Reply"),
        [
            ("_id".to_owned(), "id".to_owned(), Ty::String),
            ("count".to_owned(), "count".to_owned(), option(Ty::U64)),
            ("ratio".to_owned(), "ratio".to_owned(), option(Ty::F64)),
            ("type".to_owned(), "r#type".to_owned(), option(Ty::String)),
        ]
    );
    assert!(matches!(&model.defs["Ok"].body, Body::Struct(fields) if fields.is_empty()));
}

#[test]
fn maps_enums_and_indexed_lists() {
    let model = model(json!({
        "Effect": {"type": "object", "properties": {"endTime": {"type": "integer"}}},
        "Holder": {"type": "object", "properties": {
            "store": {"type": "object", "additionalProperties": {"type": ["integer", "null"]}},
            "kind": {"type": "string", "enum": ["ranged_attack", "move"]},
            "effects": {"anyOf": [
                {"type": "array", "items": {"$ref": "#/components/schemas/Effect"}},
                {"type": "object", "additionalProperties": {"$ref": "#/components/schemas/Effect"}},
                {"type": "null"}
            ]},
            "open": {"type": "object", "additionalProperties": true}
        }}
    }));
    let fields = fields(&model, "Holder");
    assert_eq!(
        fields[0].2,
        option(Ty::IndexedList(Box::new(named("Effect"))))
    );
    assert_eq!(fields[1].2, option(named("HolderKind")));
    assert_eq!(fields[2].2, option(Ty::Map(Box::new(Ty::Json))));
    assert_eq!(fields[3].2, option(Ty::Map(Box::new(option(Ty::I64)))));
    let Body::StrEnum(variants) = &model.defs["HolderKind"].body else {
        panic!("HolderKind is not an enum");
    };
    let variants: Vec<(&str, &str)> = variants
        .iter()
        .map(|variant| (variant.ident.as_str(), variant.value.as_str()))
        .collect();
    assert_eq!(
        variants,
        [("RangedAttack", "ranged_attack"), ("Move", "move")]
    );
}

#[test]
fn unions_become_untagged_enums_with_short_variant_names() {
    let model = model(json!({
        "TerrainEncoded": {"type": "object", "properties": {"terrain": {"type": "string"}}, "required": ["terrain"]},
        "TerrainTiles": {"type": "object", "properties": {"x": {"type": "integer"}}, "required": ["x"]},
        "Terrain": {"anyOf": [
            {"$ref": "#/components/schemas/TerrainEncoded"},
            {"$ref": "#/components/schemas/TerrainTiles"}
        ]},
        "Badge": {"type": "object", "properties": {
            "type": {"anyOf": [{"$ref": "#/components/schemas/BadgePaths"}, {"type": "integer"}]},
            "param": {"anyOf": [{"type": "number"}, {"type": "integer"}, {"type": "string"}]}
        }},
        "BadgePaths": {"type": "object", "properties": {"path1": {"type": "string"}}}
    }));
    let variants = |name: &str| -> Vec<(String, Ty)> {
        let Body::Untagged(variants) = &model.defs[name].body else {
            panic!("{name} is not an untagged enum");
        };
        variants
            .iter()
            .map(|variant| (variant.ident.clone(), variant.ty.clone()))
            .collect()
    };
    assert_eq!(
        variants("Terrain"),
        [
            ("Encoded".to_owned(), named("TerrainEncoded")),
            ("Tiles".to_owned(), named("TerrainTiles"))
        ]
    );
    // Scalars first, integers before numbers, structs last.
    assert_eq!(
        variants("BadgeType"),
        [
            ("Integer".to_owned(), Ty::I64),
            ("Paths".to_owned(), named("BadgePaths"))
        ]
    );
    assert_eq!(
        variants("BadgeParam"),
        [
            ("Integer".to_owned(), Ty::I64),
            ("String".to_owned(), Ty::String),
            ("Number".to_owned(), Ty::F64)
        ]
    );
}

#[test]
fn primitive_components_are_aliases_and_recursion_is_boxed() {
    let model = model(json!({
        "RoomName": {"type": "string", "pattern": "^[WE][0-9]+[NS][0-9]+$"},
        "Node": {"type": "object", "properties": {
            "next": {"$ref": "#/components/schemas/Node"},
            "children": {"type": "array", "items": {"$ref": "#/components/schemas/Node"}},
            "room": {"$ref": "#/components/schemas/RoomName"}
        }}
    }));
    assert!(matches!(
        &model.defs["RoomName"].body,
        Body::Alias(Ty::String)
    ));
    assert_eq!(model.resolve_alias(&named("RoomName")), Ty::String);
    let fields = fields(&model, "Node");
    assert_eq!(fields[0].2, option(Ty::Vec(Box::new(named("Node")))));
    assert_eq!(fields[1].2, option(Ty::Boxed(Box::new(named("Node")))));
    assert_eq!(fields[2].2, option(named("RoomName")));
}

#[test]
fn inline_names_colliding_with_components_are_rejected_or_suffixed() {
    let model = model(json!({
        "ObjSpawning": {"type": "object", "properties": {"name": {"type": "string"}}},
        "Obj": {"type": "object", "properties": {
            "spawning": {"anyOf": [{"type": "boolean"}, {"$ref": "#/components/schemas/ObjSpawning"}]}
        }}
    }));
    assert_eq!(
        fields(&model, "Obj")[0].2,
        option(named("ObjSpawningField"))
    );
    let mut components = Map::new();
    components.insert("AItem".to_owned(), json!({"type": "string"}));
    components.insert(
        "A".to_owned(),
        json!({"type": "array", "items": {"type": "object", "properties": {"x": {"type": "string"}}}}),
    );
    assert!(Model::new(components).is_err());
}

#[test]
fn tagged_unions_name_members_and_keep_unknown_fallback_properties() {
    let model = model(json!({
        "Visual": {"oneOf": [
            {"type": "object", "properties": {"t": {"const": "l"}, "x1": {"type": "number"}}, "required": ["t"]},
            {"type": "object", "title": "circle", "properties": {"t": {"const": "c"}}, "required": ["t"]},
            {"type": "object", "properties": {"t": {"not": {"enum": ["l", "c"]}}}, "required": ["t"]}
        ]}
    }));
    let Body::Tagged(tagged) = &model.defs["Visual"].body else {
        panic!("Visual is not a tagged enum");
    };
    let members: Vec<(&str, &Ty, &[String])> = tagged
        .variants
        .iter()
        .map(|variant| {
            (
                variant.ident.as_str(),
                &variant.ty,
                variant.values.as_slice(),
            )
        })
        .collect();
    assert_eq!(
        members,
        [
            ("L", &named("VisualL"), &["l".to_owned()][..]),
            ("Circle", &named("VisualCircle"), &["c".to_owned()][..]),
            ("Other", &named("VisualOther"), &[][..]),
        ]
    );
    assert_eq!(tagged.fallback, Some(2));
    let accessors: Vec<(&str, bool)> = tagged
        .accessors
        .iter()
        .map(|accessor| (accessor.ident.as_str(), accessor.optional))
        .collect();
    assert_eq!(accessors, [("kind", false)]);
    let other = fields(&model, "VisualOther");
    assert_eq!(
        other[1],
        (
            String::new(),
            "other".to_owned(),
            Ty::Map(Box::new(Ty::Json))
        )
    );
    assert_eq!(
        fields(&model, "VisualL").len(),
        2,
        "no catch-all on members"
    );
}

#[test]
fn fixed_length_arrays_and_inline_objects_told_apart_by_required_properties() {
    let model = model(json!({
        "Holder": {"type": "object", "properties": {
            "pos": {"type": "array", "minItems": 2, "maxItems": 2,
                "items": [{"type": "integer", "minimum": 0}, {"type": "integer", "minimum": 0}]},
            "pair": {"type": "array", "minItems": 2, "maxItems": 2,
                "prefixItems": [{"type": "string"}, {"type": "boolean"}]},
            "open": {"type": "array", "items": [{"type": "string"}]}
        }},
        "Console": {"oneOf": [
            {"type": "object", "required": ["messages"], "properties": {"messages": {"type": "string"}}},
            {"type": "object", "required": ["error"], "properties": {"error": {"type": "string"}}}
        ]}
    }));
    let fields = fields(&model, "Holder");
    assert_eq!(fields[0].2, option(Ty::Vec(Box::new(Ty::Json))));
    assert_eq!(fields[1].2, option(Ty::Tuple(vec![Ty::String, Ty::Bool])));
    assert_eq!(fields[2].2, option(Ty::Array(Box::new(Ty::U64), 2)));
    let Body::Untagged(variants) = &model.defs["Console"].body else {
        panic!("Console is not an untagged enum");
    };
    let labels: Vec<&str> = variants
        .iter()
        .map(|variant| variant.ident.as_str())
        .collect();
    assert_eq!(labels, ["Messages", "Error"]);
}
