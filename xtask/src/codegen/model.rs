//! Rust type definitions lowered from JSON Schema.
//!
//! Mapping: object schemas (and `allOf` of objects, ignoring `if`/`then`) become structs whose
//! non-required properties are `Option`, without the `ok: 1` success marker; extra properties
//! with a schema (`additionalProperties: {…}` next to `properties`) are kept in a flattened map;
//! maps (`additionalProperties` without `properties`) become `BTreeMap<String, T>`; string enums
//! become enums; fixed-length arrays (`items` as a list) become arrays or tuples. `anyOf`,
//! `oneOf` and type lists become `Option` (for `null`), an `IndexedList` (an array of `T` or an
//! index-keyed object of `T`), a tagged enum (objects told apart by one property, see
//! [`super::union`]), a literal enum (scalars and fixed strings) or an untagged enum. Component
//! schemas keep their names; inline schemas are named after their location.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::merge::{Components, ObjectShape, ref_name, shape};
use super::names;
use super::union::{self, Discriminator};
use crate::Error;

/// A Rust type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ty {
    /// `bool`.
    Bool,
    /// `i64`.
    I64,
    /// `u64`, for integers with a non-negative minimum.
    U64,
    /// `f64`.
    F64,
    /// `String`.
    String,
    /// `serde_json::Value`: a schema without constraints.
    Json,
    /// `()`: no body.
    Unit,
    /// A generated type.
    Named(String),
    /// `Vec<T>`.
    Vec(Box<Ty>),
    /// `BTreeMap<String, T>`.
    Map(Box<Ty>),
    /// `Option<T>`.
    Option(Box<Ty>),
    /// `Box<T>`, breaking a recursive type.
    Boxed(Box<Ty>),
    /// `IndexedList<T>`: an array of `T` or an index-keyed object of `T`.
    IndexedList(Box<Ty>),
    /// `[T; N]`: a fixed-length array of one item schema.
    Array(Box<Ty>, usize),
    /// `(A, B, …)`: a fixed-length array of different item schemas.
    Tuple(Vec<Ty>),
    /// `serde_json::Map<String, serde_json::Value>`: an object kept as raw JSON.
    JsonMap,
    /// A type defined by another module, by its Rust path (for example `api::GameRoomObject`).
    External(String),
}

impl Ty {
    /// The Rust spelling, with generated names prefixed by `prefix` (for example `types::`).
    pub fn render(&self, prefix: &str) -> String {
        match self {
            Self::Bool => "bool".to_owned(),
            Self::I64 => "i64".to_owned(),
            Self::U64 => "u64".to_owned(),
            Self::F64 => "f64".to_owned(),
            Self::String => "String".to_owned(),
            Self::Json => "serde_json::Value".to_owned(),
            Self::Unit => "()".to_owned(),
            Self::Named(name) => format!("{prefix}{name}"),
            Self::Vec(item) => format!("Vec<{}>", item.render(prefix)),
            Self::Map(value) => format!("BTreeMap<String, {}>", value.render(prefix)),
            Self::Option(inner) => format!("Option<{}>", inner.render(prefix)),
            Self::Boxed(inner) => format!("Box<{}>", inner.render(prefix)),
            Self::IndexedList(item) => format!("IndexedList<{}>", item.render(prefix)),
            Self::Array(item, len) => format!("[{}; {len}]", item.render(prefix)),
            Self::Tuple(items) => {
                let items: Vec<String> = items.iter().map(|item| item.render(prefix)).collect();
                format!("({})", items.join(", "))
            }
            Self::JsonMap => "serde_json::Map<String, serde_json::Value>".to_owned(),
            Self::External(path) => path.clone(),
        }
    }

    /// `Option<self>`, unless `self` already is an `Option`.
    fn optional(self) -> Self {
        match self {
            Self::Option(_) => self,
            other => Self::Option(Box::new(other)),
        }
    }

    /// Whether the type or a type it contains satisfies `test`.
    pub fn any(&self, test: &dyn Fn(&Self) -> bool) -> bool {
        if test(self) {
            return true;
        }
        match self {
            Self::Vec(inner)
            | Self::Map(inner)
            | Self::Option(inner)
            | Self::Boxed(inner)
            | Self::IndexedList(inner)
            | Self::Array(inner, _) => inner.any(test),
            Self::Tuple(items) => items.iter().any(|item| item.any(test)),
            _ => false,
        }
    }

    /// Whether the type mentions `BTreeMap`.
    pub fn uses_map(&self) -> bool {
        self.any(&|ty| matches!(ty, Self::Map(_)))
    }

    /// Whether the type mentions `IndexedList`.
    pub fn uses_indexed_list(&self) -> bool {
        self.any(&|ty| matches!(ty, Self::IndexedList(_)))
    }

    /// Whether the type mentions a type of another module.
    pub fn uses_external(&self) -> bool {
        self.any(&|ty| matches!(ty, Self::External(_)))
    }
}

/// A struct field.
#[derive(Clone, Debug)]
pub struct Field {
    /// The JSON property name.
    pub json: String,
    /// The Rust identifier.
    pub ident: String,
    /// The field type.
    pub ty: Ty,
    /// The property description.
    pub doc: Option<String>,
    /// Holds the remaining properties (`#[serde(flatten)]`).
    pub flatten: bool,
    /// An optional nullable property held as `Option<Option<T>>`: `None` when absent,
    /// `Some(None)` when `null`.
    pub present: bool,
}

impl Field {
    /// Whether `Debug` output must not show the value.
    pub fn sensitive(&self) -> bool {
        let name = self.json.to_ascii_lowercase();
        ["password", "token", "secret", "ticket"]
            .iter()
            .any(|word| name.contains(word))
    }
}

/// A variant of a string enum.
#[derive(Clone, Debug)]
pub struct EnumVariant {
    /// The Rust identifier.
    pub ident: String,
    /// The JSON string.
    pub value: String,
}

/// A variant of an untagged enum.
#[derive(Clone, Debug)]
pub struct UnionVariant {
    /// The Rust identifier.
    pub ident: String,
    /// The payload type.
    pub ty: Ty,
}

/// A member of a tagged enum.
#[derive(Clone, Debug)]
pub struct TaggedVariant {
    /// The Rust identifier.
    pub ident: String,
    /// The payload type: a generated struct.
    pub ty: Ty,
    /// The discriminator values selecting the member (empty for the fallback).
    pub values: Vec<String>,
}

/// A method of a tagged enum reading a property every member may have.
#[derive(Clone, Debug)]
pub struct Accessor {
    /// The method name.
    pub ident: String,
    /// The JSON property name.
    pub json: String,
    /// The returned value, without the `Option` of [`Self::optional`].
    pub ty: Ty,
    /// Whether some member may lack the property.
    pub optional: bool,
    /// The property description.
    pub doc: Option<String>,
    /// Per member, in member order: the field read, or `None` when the member lacks it.
    pub fields: Vec<Option<AccessedField>>,
}

/// The field an [`Accessor`] reads from one member.
#[derive(Clone, Debug)]
pub struct AccessedField {
    /// The Rust identifier.
    pub ident: String,
    /// The field type.
    pub ty: Ty,
}

/// An enum of objects told apart by one property.
#[derive(Clone, Debug)]
pub struct Tagged {
    /// The discriminating JSON property.
    pub property: String,
    /// The members, in document order.
    pub variants: Vec<TaggedVariant>,
    /// The member taking every other discriminator value, if any.
    pub fallback: Option<usize>,
    /// Methods reading the properties members share (filled by [`Model::finish`]).
    pub accessors: Vec<Accessor>,
}

/// A variant of a literal enum.
#[derive(Clone, Debug)]
pub enum Literal {
    /// A unit variant for one fixed string.
    Const {
        /// The Rust identifier.
        ident: String,
        /// The JSON string.
        value: String,
    },
    /// A variant holding a scalar.
    Scalar(UnionVariant),
}

/// The body of a definition.
#[derive(Clone, Debug)]
pub enum Body {
    /// A struct with named fields.
    Struct(Vec<Field>),
    /// A string enum.
    StrEnum(Vec<EnumVariant>),
    /// An untagged enum; serde tries the variants in order.
    Untagged(Vec<UnionVariant>),
    /// An enum of objects told apart by one property.
    Tagged(Tagged),
    /// An enum of scalars and fixed strings.
    Literal(Vec<Literal>),
    /// A type alias.
    Alias(Ty),
}

impl Body {
    /// The types a definition mentions.
    pub fn types(&self) -> Vec<&Ty> {
        match self {
            Self::Struct(fields) => fields.iter().map(|field| &field.ty).collect(),
            Self::Untagged(variants) => variants.iter().map(|variant| &variant.ty).collect(),
            Self::Tagged(tagged) => tagged.variants.iter().map(|variant| &variant.ty).collect(),
            Self::Literal(variants) => variants
                .iter()
                .filter_map(|variant| match variant {
                    Literal::Scalar(scalar) => Some(&scalar.ty),
                    Literal::Const { .. } => None,
                })
                .collect(),
            Self::Alias(ty) => vec![ty],
            Self::StrEnum(_) => Vec::new(),
        }
    }
}

/// Choices that differ between the documents a model is built from.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Optional nullable properties become `Option<Option<T>>`, so an absent property (no
    /// change) and `null` (cleared) differ.
    pub absent_vs_null: bool,
    /// Component schemas generated as raw JSON objects, by name.
    pub raw: BTreeSet<String>,
    /// Component schemas whose type another module defines: name to Rust path.
    pub external: BTreeMap<String, String>,
}

/// A generated type definition.
#[derive(Clone, Debug)]
pub struct Def {
    /// The type name.
    pub name: String,
    /// The description, or a generated one.
    pub doc: String,
    /// The definition.
    pub body: Body,
}

/// The type definitions of a document.
#[derive(Debug)]
pub struct Model {
    /// The merged component schemas.
    pub components: Components,
    /// Definitions by name.
    pub defs: BTreeMap<String, Def>,
    /// Component names, reserved before inline definitions are named.
    reserved: BTreeSet<String>,
    /// The component being lowered, the only one that may define its reserved name.
    current: Option<String>,
    /// The generation choices.
    options: Options,
    /// Structs that keep unknown properties in a flattened map: fallback members of tagged
    /// enums.
    keep_unknown: BTreeSet<String>,
}

impl Model {
    /// Lowers every component schema with the default options.
    pub fn new(components: Components) -> Result<Self, Error> {
        Self::with_options(components, Options::default())
    }

    /// Lowers every component schema except the external ones.
    pub fn with_options(components: Components, options: Options) -> Result<Self, Error> {
        let reserved = components.keys().cloned().collect();
        let keep_unknown = fallback_components(&components);
        for name in options.raw.iter().chain(options.external.keys()) {
            if !components.contains_key(name) {
                return Err(format!("no component schema `{name}` to map").into());
            }
        }
        let mut model = Self {
            components,
            defs: BTreeMap::new(),
            reserved,
            current: None,
            options,
            keep_unknown,
        };
        let components = model.components.clone();
        for (name, schema) in &components {
            if model.options.external.contains_key(name) {
                continue;
            }
            model.current = Some(name.clone());
            let fallback = format!("The `{name}` schema.");
            let ty = if model.options.raw.contains(name) {
                Ty::JsonMap
            } else {
                model.lower(schema, name, &fallback)?
            };
            if ty != Ty::Named(name.clone()) {
                let doc = description(schema).unwrap_or(fallback);
                model.insert(Def {
                    name: name.clone(),
                    doc,
                    body: Body::Alias(ty),
                })?;
            }
        }
        model.current = None;
        Ok(model)
    }

    fn insert(&mut self, def: Def) -> Result<(), Error> {
        if self.reserved.contains(&def.name) && self.current.as_ref() != Some(&def.name) {
            return Err(format!(
                "inline type name `{}` collides with a component schema",
                def.name
            )
            .into());
        }
        if self.defs.contains_key(&def.name) {
            return Err(format!("type `{}` is generated twice", def.name).into());
        }
        self.defs.insert(def.name.clone(), def);
        Ok(())
    }

    /// Lowers `schema`; a definition it needs is named `name`, and definitions nested in it are
    /// named after `name`.
    pub fn lower(&mut self, schema: &Value, name: &str, fallback: &str) -> Result<Ty, Error> {
        let Some(map) = schema.as_object() else {
            return Ok(Ty::Json);
        };
        if let Some(reference) = map.get("$ref").and_then(Value::as_str) {
            let target =
                ref_name(reference).ok_or_else(|| format!("unsupported $ref {reference}"))?;
            if !self.reserved.contains(target) {
                return Err(format!("$ref to unknown schema {target}").into());
            }
            if let Some(path) = self.options.external.get(target) {
                return Ok(Ty::External(path.clone()));
            }
            return Ok(Ty::Named(target.to_owned()));
        }
        let doc = description(schema).unwrap_or_else(|| fallback.to_owned());
        if let Some(variants) = map
            .get("anyOf")
            .or_else(|| map.get("oneOf"))
            .and_then(Value::as_array)
        {
            return self.lower_union(schema, variants.clone(), name, &doc);
        }
        if let Some(types) = map.get("type").and_then(Value::as_array) {
            let variants = types
                .iter()
                .map(|ty| {
                    let mut variant = map.clone();
                    variant.insert("type".to_owned(), ty.clone());
                    variant.remove("description");
                    Value::Object(variant)
                })
                .collect();
            return self.lower_union(schema, variants, name, &doc);
        }
        if let Some(members) = map.get("allOf").and_then(Value::as_array) {
            let members: Vec<&Value> = members
                .iter()
                .filter(|member| member.get("if").is_none())
                .collect();
            if let ([member], None) = (members.as_slice(), map.get("properties")) {
                return self.lower(member, name, &doc);
            }
            return self.lower_struct(schema, name, doc);
        }
        if map.contains_key("properties") {
            return self.lower_struct(schema, name, doc);
        }
        if let Some(values) = map.get("enum").and_then(Value::as_array) {
            return self.lower_enum(values, name, doc);
        }
        if let Some(value) = map.get("const") {
            return Ok(match value {
                Value::Bool(_) => Ty::Bool,
                Value::String(_) => Ty::String,
                Value::Number(number) if number.is_u64() => Ty::U64,
                Value::Number(number) if number.is_i64() => Ty::I64,
                Value::Number(_) => Ty::F64,
                _ => Ty::Json,
            });
        }
        let excluded = map
            .get("not")
            .and_then(|not| not.get("enum"))
            .and_then(Value::as_array);
        if map.get("type").is_none()
            && excluded.is_some_and(|values| values.iter().all(Value::is_string))
        {
            // Any string but the listed ones: the discriminator of a fallback union member.
            return Ok(Ty::String);
        }
        let additional = map
            .get("additionalProperties")
            .filter(|value| value.is_object());
        let ty = map.get("type").and_then(Value::as_str);
        Ok(match ty {
            Some("string") => Ty::String,
            Some("integer") if minimum(map).is_some_and(|minimum| minimum >= 0.0) => Ty::U64,
            Some("integer") => Ty::I64,
            Some("number") => Ty::F64,
            Some("boolean") => Ty::Bool,
            Some("array") => self.lower_items(map, name, &doc)?,
            None if map.contains_key("items") || map.contains_key("prefixItems") => {
                self.lower_items(map, name, &doc)?
            }
            Some("object") | None => match additional {
                Some(value) => Ty::Map(Box::new(self.lower(
                    value,
                    &format!("{name}Value"),
                    &doc,
                )?)),
                None if ty.is_some() => Ty::Map(Box::new(Ty::Json)),
                None => Ty::Json,
            },
            _ => Ty::Json,
        })
    }

    /// Lowers the items of an array schema: `Vec<T>`, or an array or tuple when `items` (or
    /// `prefixItems`) lists the schema of each position of a fixed-length array.
    fn lower_items(
        &mut self,
        map: &serde_json::Map<String, Value>,
        name: &str,
        doc: &str,
    ) -> Result<Ty, Error> {
        let positions = map
            .get("prefixItems")
            .or_else(|| map.get("items").filter(|items| items.is_array()))
            .and_then(Value::as_array);
        let Some(positions) = positions else {
            return Ok(match map.get("items") {
                Some(items) => Ty::Vec(Box::new(self.lower(items, &format!("{name}Item"), doc)?)),
                None => Ty::Vec(Box::new(Ty::Json)),
            });
        };
        let count = |key: &str| {
            map.get(key)
                .and_then(Value::as_u64)
                .and_then(|count| usize::try_from(count).ok())
        };
        let len = positions.len();
        if len == 0 || count("minItems") != Some(len) || count("maxItems") != Some(len) {
            return Ok(Ty::Vec(Box::new(Ty::Json)));
        }
        let mut items: Vec<Ty> = Vec::new();
        for (index, position) in positions.iter().enumerate() {
            let ty = if index > 0 && shape(position) == shape(&positions[0]) {
                items[0].clone()
            } else {
                let suffix = if index == 0 {
                    String::new()
                } else {
                    (index + 1).to_string()
                };
                self.lower(position, &format!("{name}Item{suffix}"), doc)?
            };
            items.push(ty);
        }
        Ok(if items.iter().all(|item| *item == items[0]) {
            Ty::Array(Box::new(items.swap_remove(0)), len)
        } else {
            Ty::Tuple(items)
        })
    }

    fn lower_enum(&mut self, values: &[Value], name: &str, doc: String) -> Result<Ty, Error> {
        if values.iter().all(Value::is_u64) {
            return Ok(Ty::U64);
        }
        if values.iter().all(|value| value.is_i64() || value.is_u64()) {
            return Ok(Ty::I64);
        }
        if !values.iter().all(Value::is_string) {
            return Ok(Ty::Json);
        }
        let mut variants: Vec<EnumVariant> = Vec::new();
        for value in values.iter().filter_map(Value::as_str) {
            let ident = names::type_name(value);
            if variants.iter().any(|known| known.ident == ident) {
                return Err(format!("enum `{name}` has two variants named {ident}").into());
            }
            variants.push(EnumVariant {
                ident,
                value: value.to_owned(),
            });
        }
        self.insert(Def {
            name: name.to_owned(),
            doc,
            body: Body::StrEnum(variants),
        })?;
        Ok(Ty::Named(name.to_owned()))
    }

    fn lower_struct(&mut self, schema: &Value, name: &str, doc: String) -> Result<Ty, Error> {
        let object = ObjectShape::of(schema, &self.components)
            .ok_or_else(|| format!("`{name}`: allOf members are not all objects"))?;
        let mut fields: Vec<Field> = Vec::new();
        for (json, property) in &object.properties {
            if json == "ok" && property.get("const") == Some(&Value::from(1)) {
                continue;
            }
            let fallback = format!("The `{json}` property.");
            let mut hint = format!("{name}{}", names::type_name(json));
            if self.reserved.contains(&hint) {
                // A helper component already has the natural name (`GameRoomObjectSpawning`
                // for the `spawning` union that references it): name the field's own type.
                hint.push_str("Field");
            }
            let mut ty = self.lower(property, &hint, &fallback)?;
            let mut present = false;
            if !object.required.contains(json) {
                if self.options.absent_vs_null && matches!(ty, Ty::Option(_)) {
                    ty = Ty::Option(Box::new(ty));
                    present = true;
                } else {
                    ty = ty.optional();
                }
            }
            let mut ident = names::field(json);
            if fields.iter().any(|field| field.ident == ident) {
                ident = format!("{}_raw", ident.trim_start_matches("r#"));
            }
            fields.push(Field {
                json: json.clone(),
                ident,
                ty,
                doc: description(property),
                flatten: false,
                present,
            });
        }
        let other = match &object.additional {
            Some(additional) => {
                Some(self.lower(additional, &format!("{name}Other"), "The other properties.")?)
            }
            None if self.keep_unknown.contains(name) => Some(Ty::Json),
            None => None,
        };
        if let Some(value) = other {
            let ident = if fields.iter().any(|field| field.ident == "other") {
                "other_properties"
            } else {
                "other"
            };
            fields.push(Field {
                json: String::new(),
                ident: ident.to_owned(),
                ty: Ty::Map(Box::new(value)),
                doc: Some("The other properties.".to_owned()),
                flatten: true,
                present: false,
            });
        }
        self.insert(Def {
            name: name.to_owned(),
            doc,
            body: Body::Struct(fields),
        })?;
        Ok(Ty::Named(name.to_owned()))
    }

    /// Lowers the members of the union `owner` (a schema with `anyOf`/`oneOf`, or with a list
    /// of types expanded into `members`).
    fn lower_union(
        &mut self,
        owner: &Value,
        members: Vec<Value>,
        name: &str,
        doc: &str,
    ) -> Result<Ty, Error> {
        let mut flat = Vec::new();
        flatten_union(members, &mut flat);
        let nullable = flat.iter().any(is_null);
        flat.retain(|variant| !is_null(variant));
        let ty = if let Some(item) = indexed_list_item(&flat) {
            Ty::IndexedList(Box::new(self.lower(item, &format!("{name}Item"), doc)?))
        } else if let [single] = flat.as_slice() {
            self.lower(single, name, doc)?
        } else if flat.is_empty() {
            Ty::Json
        } else if let Some(discriminator) = union::discriminator(owner, &flat, &self.components) {
            self.lower_tagged(&flat, discriminator, name, doc)?
        } else if is_literal(&flat) {
            self.lower_literal(&flat, name, doc)?
        } else {
            self.lower_untagged(&flat, name, doc)?
        };
        Ok(if nullable { ty.optional() } else { ty })
    }

    fn lower_tagged(
        &mut self,
        members: &[Value],
        discriminator: Discriminator,
        name: &str,
        doc: &str,
    ) -> Result<Ty, Error> {
        let labels = tagged_labels(members, &discriminator, name);
        let mut variants = Vec::new();
        for ((index, member), label) in members.iter().enumerate().zip(labels) {
            let inline = format!("{name}{label}");
            if discriminator.fallback == Some(index) && member.get("$ref").is_none() {
                self.keep_unknown.insert(inline.clone());
            }
            let fallback = format!("The `{label}` member of `{name}`.");
            let ty = self.lower(member, &inline, &fallback)?;
            if !matches!(ty, Ty::Named(_)) {
                return Err(format!("`{name}`: member `{label}` is not a generated struct").into());
            }
            variants.push(TaggedVariant {
                ident: label,
                ty,
                values: discriminator.values[index].clone(),
            });
        }
        self.insert(Def {
            name: name.to_owned(),
            doc: doc.to_owned(),
            body: Body::Tagged(Tagged {
                property: discriminator.property,
                variants,
                fallback: discriminator.fallback,
                accessors: Vec::new(),
            }),
        })?;
        Ok(Ty::Named(name.to_owned()))
    }

    /// Lowers a union of scalars and fixed strings (see [`is_literal`]).
    fn lower_literal(&mut self, members: &[Value], name: &str, doc: &str) -> Result<Ty, Error> {
        let mut variants: Vec<Literal> = Vec::new();
        let mut idents: Vec<String> = Vec::new();
        for member in members {
            let variant = match member.get("const").and_then(Value::as_str) {
                Some(value) => Literal::Const {
                    ident: names::type_name(value),
                    value: value.to_owned(),
                },
                None => Literal::Scalar(UnionVariant {
                    ident: kind_label(member).to_owned(),
                    ty: self.lower(member, name, doc)?,
                }),
            };
            let ident = match &variant {
                Literal::Const { ident, .. } | Literal::Scalar(UnionVariant { ident, .. }) => ident,
            };
            if idents.contains(ident) {
                return Err(
                    format!("literal union `{name}` has two variants named {ident}").into(),
                );
            }
            idents.push(ident.clone());
            variants.push(variant);
        }
        self.insert(Def {
            name: name.to_owned(),
            doc: doc.to_owned(),
            body: Body::Literal(variants),
        })?;
        Ok(Ty::Named(name.to_owned()))
    }

    fn lower_untagged(&mut self, variants: &[Value], name: &str, doc: &str) -> Result<Ty, Error> {
        let labels = variant_labels(variants, name);
        let mut lowered: Vec<(usize, UnionVariant)> = Vec::new();
        for (variant, label) in variants.iter().zip(labels) {
            let fallback = format!("The `{label}` form of `{name}`.");
            let ty = self.lower(variant, &format!("{name}{label}"), &fallback)?;
            if lowered.iter().any(|(_, known)| known.ty == ty) {
                continue;
            }
            let rank = self.rank(&ty);
            lowered.push((rank, UnionVariant { ident: label, ty }));
        }
        lowered.sort_by_key(|(rank, _)| *rank);
        let variants: Vec<UnionVariant> = lowered.into_iter().map(|(_, variant)| variant).collect();
        if let [only] = variants.as_slice() {
            return Ok(only.ty.clone());
        }
        self.insert(Def {
            name: name.to_owned(),
            doc: doc.to_owned(),
            body: Body::Untagged(variants),
        })?;
        Ok(Ty::Named(name.to_owned()))
    }

    /// The order serde should try an untagged variant in: exact scalars first, numbers after
    /// integers, structs before maps and unconstrained values that accept any object.
    fn rank(&self, ty: &Ty) -> usize {
        match ty {
            Ty::Bool | Ty::I64 | Ty::U64 | Ty::String | Ty::Unit => 0,
            Ty::F64 => 1,
            Ty::Vec(_) | Ty::IndexedList(_) | Ty::Array(..) | Ty::Tuple(_) => 2,
            Ty::Named(name) => match self.defs.get(name).map(|def| &def.body) {
                Some(Body::Alias(inner)) => self.rank(inner),
                _ => 3,
            },
            Ty::External(_) => 3,
            Ty::Option(inner) | Ty::Boxed(inner) => self.rank(inner),
            Ty::Map(_) | Ty::JsonMap => 4,
            Ty::Json => 5,
        }
    }

    /// Completes the model once every type is lowered: boxes recursion and adds the accessors
    /// of tagged enums.
    pub fn finish(&mut self) -> Result<(), Error> {
        self.box_recursion();
        let names: Vec<String> = self.defs.keys().cloned().collect();
        for name in names {
            let accessors = match &self.defs[&name].body {
                Body::Tagged(tagged) => self.accessors(&name, tagged)?,
                _ => continue,
            };
            if let Some(Def {
                body: Body::Tagged(tagged),
                ..
            }) = self.defs.get_mut(&name)
            {
                tagged.accessors = accessors;
            }
        }
        Ok(())
    }

    /// Boxes fields and variants that would make a type contain itself.
    fn box_recursion(&mut self) {
        let names: Vec<String> = self.defs.keys().cloned().collect();
        for name in names {
            let mut def = self.defs[&name].clone();
            match &mut def.body {
                Body::Struct(fields) => {
                    for field in fields {
                        field.ty = self.boxed(&name, &field.ty);
                    }
                }
                Body::Untagged(variants) => {
                    for variant in variants {
                        variant.ty = self.boxed(&name, &variant.ty);
                    }
                }
                Body::Tagged(tagged) => {
                    for variant in &mut tagged.variants {
                        variant.ty = self.boxed(&name, &variant.ty);
                    }
                }
                Body::Alias(_) | Body::StrEnum(_) | Body::Literal(_) => {}
            }
            self.defs.insert(name, def);
        }
    }

    fn boxed(&self, owner: &str, ty: &Ty) -> Ty {
        match ty {
            Ty::Option(inner) => Ty::Option(Box::new(self.boxed(owner, inner))),
            Ty::Named(target) if self.reaches_by_value(target, owner, &mut BTreeSet::new()) => {
                Ty::Boxed(Box::new(ty.clone()))
            }
            other => other.clone(),
        }
    }

    /// Whether `from` contains `to` without indirection (through fields, variants, aliases,
    /// options, arrays and tuples, but not vectors, maps or boxes).
    fn reaches_by_value(&self, from: &str, to: &str, seen: &mut BTreeSet<String>) -> bool {
        if from == to {
            return true;
        }
        if !seen.insert(from.to_owned()) {
            return false;
        }
        let Some(def) = self.defs.get(from) else {
            return false;
        };
        let mut names = Vec::new();
        for ty in def.body.types() {
            by_value_names(ty, &mut names);
        }
        names
            .into_iter()
            .any(|name| self.reaches_by_value(name, to, seen))
    }

    /// The fields of the struct `ty` names, through boxes and aliases.
    fn struct_fields(&self, ty: &Ty) -> Option<&[Field]> {
        match ty {
            Ty::Boxed(inner) => self.struct_fields(inner),
            Ty::Named(name) => match &self.defs.get(name)?.body {
                Body::Struct(fields) => Some(fields),
                Body::Alias(inner) => self.struct_fields(inner),
                _ => None,
            },
            _ => None,
        }
    }

    /// Whether `ty` names a string enum.
    pub fn is_str_enum(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Named(name)
            if matches!(self.defs.get(name).map(|def| &def.body), Some(Body::StrEnum(_))))
    }

    /// Methods of the tagged enum `name` reading the properties its members share: those of
    /// the fallback member (every object has them; members lacking one return `None`), or
    /// without a fallback those every member declares. The discriminator is read by `kind`.
    fn accessors(&self, name: &str, tagged: &Tagged) -> Result<Vec<Accessor>, Error> {
        let mut members = Vec::new();
        for variant in &tagged.variants {
            let fields = self
                .struct_fields(&variant.ty)
                .ok_or_else(|| format!("`{name}`: member `{}` is not a struct", variant.ident))?;
            members.push(fields);
        }
        let declared = |fields: &[Field], json: &str| -> Option<Field> {
            fields
                .iter()
                .find(|field| !field.flatten && field.json == json)
                .cloned()
        };
        let base: Vec<&Field> = match tagged.fallback {
            Some(fallback) => members[fallback]
                .iter()
                .filter(|field| !field.flatten)
                .collect(),
            None => members[0]
                .iter()
                .filter(|field| {
                    !field.flatten
                        && members
                            .iter()
                            .all(|fields| declared(fields, &field.json).is_some())
                })
                .collect(),
        };
        let mut accessors: Vec<Accessor> = Vec::new();
        for field in base {
            let fields: Vec<Option<Field>> = members
                .iter()
                .map(|fields| declared(fields, &field.json))
                .collect();
            if fields.iter().flatten().any(|field| field.present) {
                continue;
            }
            let inners: Vec<Ty> = fields
                .iter()
                .flatten()
                .map(|field| unwrapped(&field.ty).clone())
                .collect();
            let ty = if inners
                .iter()
                .all(|inner| *inner == Ty::String || self.is_str_enum(inner))
            {
                Ty::String
            } else if inners.iter().all(|inner| *inner == inners[0]) {
                inners[0].clone()
            } else if tagged.fallback.is_some() {
                return Err(
                    format!("`{name}`: members disagree on the type of `{}`", field.json).into(),
                );
            } else {
                continue;
            };
            let ident = if field.json == tagged.property {
                "kind".to_owned()
            } else {
                field.ident.clone()
            };
            if accessors.iter().any(|known| known.ident == ident) {
                continue;
            }
            let optional = fields.iter().any(|field| {
                field
                    .as_ref()
                    .is_none_or(|field| matches!(field.ty, Ty::Option(_)))
            });
            accessors.push(Accessor {
                ident,
                json: field.json.clone(),
                ty,
                optional,
                doc: field.doc.clone(),
                fields: fields
                    .into_iter()
                    .map(|field| {
                        field.map(|field| AccessedField {
                            ident: field.ident,
                            ty: field.ty,
                        })
                    })
                    .collect(),
            });
        }
        Ok(accessors)
    }

    /// `ty` with aliases of non-generated types resolved, for endpoint fields.
    pub fn resolve_alias(&self, ty: &Ty) -> Ty {
        match ty {
            Ty::Named(name) => match self.defs.get(name).map(|def| &def.body) {
                Some(Body::Alias(inner)) => self.resolve_alias(inner),
                _ => ty.clone(),
            },
            Ty::Option(inner) => Ty::Option(Box::new(self.resolve_alias(inner))),
            other => other.clone(),
        }
    }

    /// Whether every value of `ty` can be built with `Default`.
    pub fn is_default(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Named(name) => match self.defs.get(name).map(|def| &def.body) {
                Some(Body::Alias(inner)) => self.is_default(inner),
                Some(Body::Struct(fields)) => fields.iter().all(|field| self.is_default(&field.ty)),
                _ => false,
            },
            Ty::Boxed(inner) | Ty::Array(inner, _) => self.is_default(inner),
            Ty::Tuple(items) => items.iter().all(|item| self.is_default(item)),
            Ty::External(_) => false,
            _ => true,
        }
    }
}

/// Appends the generated types `ty` holds by value.
fn by_value_names<'t>(ty: &'t Ty, out: &mut Vec<&'t str>) {
    match ty {
        Ty::Named(name) => out.push(name),
        Ty::Option(inner) | Ty::Array(inner, _) => by_value_names(inner, out),
        Ty::Tuple(items) => {
            for item in items {
                by_value_names(item, out);
            }
        }
        _ => {}
    }
}

/// `ty` without `Option` and `Box` layers.
pub fn unwrapped(ty: &Ty) -> &Ty {
    match ty {
        Ty::Option(inner) | Ty::Boxed(inner) => unwrapped(inner),
        other => other,
    }
}

/// The fallback members of the tagged unions among `components` that are component schemas:
/// their structs keep unknown properties.
fn fallback_components(components: &Components) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for schema in components.values() {
        let Some(members) = schema
            .get("anyOf")
            .or_else(|| schema.get("oneOf"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        let mut flat = Vec::new();
        flatten_union(members.clone(), &mut flat);
        flat.retain(|member| !is_null(member));
        let Some(discriminator) = union::discriminator(schema, &flat, components) else {
            continue;
        };
        let target = discriminator
            .fallback
            .and_then(|fallback| flat[fallback].get("$ref"))
            .and_then(Value::as_str)
            .and_then(ref_name);
        if let Some(target) = target {
            names.insert(target.to_owned());
        }
    }
    names
}

/// Whether union members are scalars and at least one fixed string, held by a literal enum.
fn is_literal(members: &[Value]) -> bool {
    let is_const = |member: &Value| {
        member.get("const").is_some_and(Value::is_string)
            && member.as_object().is_some_and(|map| {
                map.keys()
                    .all(|key| matches!(key.as_str(), "const" | "type" | "description"))
            })
    };
    let is_scalar = |member: &Value| {
        matches!(
            member.get("type").and_then(Value::as_str),
            Some("string" | "integer" | "number" | "boolean")
        ) && ["const", "enum", "properties", "items"]
            .iter()
            .all(|key| member.get(key).is_none())
    };
    members.iter().any(is_const)
        && members
            .iter()
            .all(|member| is_const(member) || is_scalar(member))
}

/// Variant names of a tagged enum: referenced schema names as for untagged enums; inline
/// members are named by their `title`, `Other` for the fallback, or their first value.
fn tagged_labels(members: &[Value], discriminator: &Discriminator, owner: &str) -> Vec<String> {
    let mut labels = variant_labels(members, owner);
    for (index, member) in members.iter().enumerate() {
        if member.get("$ref").is_some() || member.get("title").is_some() {
            continue;
        }
        labels[index] = match discriminator.values[index].first() {
            _ if discriminator.fallback == Some(index) => "Other".to_owned(),
            Some(value) => names::type_name(value),
            None => labels[index].clone(),
        };
    }
    dedupe(&mut labels);
    labels
}

/// Appends the position to labels that repeat an earlier one.
fn dedupe(labels: &mut [String]) {
    for index in 0..labels.len() {
        if labels[..index].contains(&labels[index]) {
            labels[index] = format!("{}{}", labels[index], index + 1);
        }
    }
}

/// The `description` of a schema.
pub fn description(schema: &Value) -> Option<String> {
    schema
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The numeric `minimum` of a schema.
fn minimum(map: &serde_json::Map<String, Value>) -> Option<f64> {
    map.get("minimum").and_then(Value::as_f64)
}

/// Whether a union variant is `{type: null}`.
fn is_null(variant: &Value) -> bool {
    variant.get("type").and_then(Value::as_str) == Some("null")
}

/// Appends the variants of a union, expanding nested unions that add no constraints.
fn flatten_union(variants: Vec<Value>, out: &mut Vec<Value>) {
    for variant in variants {
        let nested = variant
            .get("anyOf")
            .or_else(|| variant.get("oneOf"))
            .and_then(Value::as_array)
            .filter(|_| {
                variant.as_object().is_some_and(|map| {
                    map.keys()
                        .all(|key| matches!(key.as_str(), "anyOf" | "oneOf" | "description"))
                })
            })
            .cloned();
        match nested {
            Some(nested) => flatten_union(nested, out),
            None => out.push(variant),
        }
    }
}

/// `T` when the variants are exactly an array of `T` and an object whose values are `T`.
fn indexed_list_item(variants: &[Value]) -> Option<&Value> {
    let [first, second] = variants else {
        return None;
    };
    let is = |value: &Value, ty: &str| value.get("type").and_then(Value::as_str) == Some(ty);
    let (array, object) = if is(first, "array") {
        (first, second)
    } else {
        (second, first)
    };
    if !is(array, "array") || !is(object, "object") || object.get("properties").is_some() {
        return None;
    }
    let item = array.get("items")?;
    let value = object
        .get("additionalProperties")
        .filter(|value| value.is_object())?;
    (shape(item) == shape(value)).then_some(item)
}

/// Variant names of an untagged enum: the referenced schema names without the words they all
/// share; for inline variants their `title`, or the JSON kind (inline objects of one kind are
/// told apart by the first property only they require).
fn variant_labels(variants: &[Value], owner: &str) -> Vec<String> {
    let refs: Vec<Option<Vec<String>>> = variants
        .iter()
        .map(|variant| {
            variant
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(ref_name)
                .map(names::words)
        })
        .collect();
    let named: Vec<&Vec<String>> = refs.iter().flatten().collect();
    let (prefix, suffix) = if named.len() >= 2 {
        common_affixes(&named)
    } else {
        (0, 0)
    };
    let owner = names::words(owner);
    let kinds: Vec<&str> = variants.iter().map(kind_label).collect();
    let mut labels: Vec<String> = variants
        .iter()
        .zip(&refs)
        .enumerate()
        .map(|(index, (variant, words))| match words {
            Some(words) => {
                // Also drop the words the enum's own name starts with: `UserBadgeType` holds
                // `CustomPaths`, not `UserBadgeCustomPaths`.
                let words = &words[prefix..words.len() - suffix];
                let shared = words.iter().zip(&owner).take_while(|(a, b)| a == b).count();
                if shared < words.len() {
                    words[shared..].concat()
                } else {
                    words.last().cloned().unwrap_or_default()
                }
            }
            None => match variant.get("title").and_then(Value::as_str) {
                Some(title) => names::type_name(title),
                None if kinds.iter().filter(|kind| **kind == kinds[index]).count() > 1 => {
                    own_required(variants, index)
                        .map_or_else(|| kinds[index].to_owned(), names::type_name)
                }
                None => kinds[index].to_owned(),
            },
        })
        .collect();
    dedupe(&mut labels);
    labels
}

/// The first property variant `index` requires that no other variant requires.
fn own_required(variants: &[Value], index: usize) -> Option<&str> {
    fn required(variant: &Value) -> Vec<&str> {
        variant
            .get("required")
            .and_then(Value::as_array)
            .map(|names| names.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default()
    }
    required(&variants[index]).into_iter().find(|name| {
        variants
            .iter()
            .enumerate()
            .all(|(other, variant)| other == index || !required(variant).contains(name))
    })
}

/// Word counts shared at the start and at the end of every name, leaving each a word.
fn common_affixes(names: &[&Vec<String>]) -> (usize, usize) {
    let shortest = names.iter().map(|words| words.len()).min().unwrap_or(0);
    let shared = |pick: &dyn Fn(&Vec<String>, usize) -> String| {
        (0..shortest)
            .take_while(|&index| {
                let word = pick(names[0], index);
                names.iter().all(|words| pick(words, index) == word)
            })
            .count()
    };
    let prefix = shared(&|words, index| words[index].clone());
    let suffix = shared(&|words, index| words[words.len() - 1 - index].clone());
    if prefix + suffix >= shortest {
        (0, 0)
    } else {
        (prefix, suffix)
    }
}

/// The variant name for an inline union member.
fn kind_label(variant: &Value) -> &'static str {
    if variant.get("const").is_some_and(Value::is_boolean) {
        return "Bool";
    }
    match variant.get("type").and_then(Value::as_str) {
        Some("string") => "String",
        Some("integer") => "Integer",
        Some("number") => "Number",
        Some("boolean") => "Bool",
        Some("array") => "List",
        Some("object") if variant.get("properties").is_some() => "Object",
        Some("object") => "Map",
        _ if variant.get("properties").is_some() || variant.get("allOf").is_some() => "Object",
        _ => "Value",
    }
}

#[cfg(test)]
mod tests {
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
}
