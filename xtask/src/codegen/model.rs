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

#[cfg(test)]
mod tests;
mod unions;

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::names;
use super::schema::{Components, ObjectShape, ref_name, shape};
use crate::Error;

pub use unions::fallback_components;

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
    /// The component schemas.
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
    /// The schema each definition was lowered from, by definition name (definitions the
    /// lowering creates; not those [`Self::finish`] adds).
    sources: BTreeMap<String, Value>,
    /// The structs that borrow, see [`Self::borrow`].
    borrowing: BTreeSet<String>,
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
            sources: BTreeMap::new(),
            borrowing: BTreeSet::new(),
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

    /// The schema the definition `name` was lowered from; `None` for components lowered to no
    /// definition of their own and for definitions [`Self::finish`] added.
    pub fn source(&self, name: &str) -> Option<&Value> {
        self.sources.get(name)
    }

    /// The inline definitions (named after their location, not components) and the schemas they
    /// were lowered from.
    pub fn inline_sources(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.sources
            .iter()
            .filter(|(name, _)| !self.reserved.contains(name.as_str()))
            .map(|(name, schema)| (name.as_str(), schema))
    }

    /// Lowers `schema`; a definition it needs is named `name`, and definitions nested in it are
    /// named after `name`. Records the schema a new definition `name` comes from.
    pub fn lower(&mut self, schema: &Value, name: &str, fallback: &str) -> Result<Ty, Error> {
        let ty = self.lower_schema(schema, name, fallback)?;
        if self.defs.contains_key(name) && !self.sources.contains_key(name) {
            self.sources.insert(name.to_owned(), schema.clone());
        }
        Ok(ty)
    }

    /// [`Self::lower`] without recording the source.
    fn lower_schema(&mut self, schema: &Value, name: &str, fallback: &str) -> Result<Ty, Error> {
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

    /// Whether `ty` names a string enum.
    pub fn is_str_enum(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Named(name)
            if matches!(self.defs.get(name).map(|def| &def.body), Some(Body::StrEnum(_))))
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

    /// Makes the structs among `names` borrow if they hold a field of a type that is not
    /// `Copy`. Such a struct takes the lifetime `'a` and holds each of these fields as `Cow`
    /// (see [`super::borrow`]).
    pub fn borrow(&mut self, names: &BTreeSet<String>) {
        let borrowing = names
            .iter()
            .filter(|name| {
                matches!(
                    self.defs.get(name.as_str()).map(|def| &def.body),
                    Some(Body::Struct(fields)) if fields.iter().any(|field| !self.is_copy(&field.ty))
                )
            })
            .cloned()
            .collect();
        self.borrowing = borrowing;
    }

    /// Whether the definition `name` borrows (see [`Self::borrow`]).
    pub fn borrows(&self, name: &str) -> bool {
        self.borrowing.contains(name)
    }

    /// Whether the Rust type of `ty` is `Copy`.
    pub fn is_copy(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Bool | Ty::I64 | Ty::U64 | Ty::F64 | Ty::Unit => true,
            Ty::Option(inner) | Ty::Array(inner, _) => self.is_copy(inner),
            Ty::Tuple(items) => items.iter().all(|item| self.is_copy(item)),
            Ty::Named(name) => match self.defs.get(name).map(|def| &def.body) {
                Some(Body::StrEnum(_)) => true,
                Some(Body::Literal(variants)) => is_copy_literal(variants),
                Some(Body::Alias(inner)) => self.is_copy(inner),
                _ => false,
            },
            _ => false,
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

/// Whether a literal enum of `variants` is `Copy`: no variant holds a string.
pub fn is_copy_literal(variants: &[Literal]) -> bool {
    !variants
        .iter()
        .any(|variant| matches!(variant, Literal::Scalar(scalar) if scalar.ty == Ty::String))
}

/// `ty` without `Option` and `Box` layers.
pub fn unwrapped(ty: &Ty) -> &Ty {
    match ty {
        Ty::Option(inner) | Ty::Boxed(inner) => unwrapped(inner),
        other => other,
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
