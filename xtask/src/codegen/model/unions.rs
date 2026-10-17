//! Lowering of unions (`anyOf`, `oneOf` and type lists).
//!
//! A union becomes `Option` (for `null`), an `IndexedList`, a tagged enum (see
//! [`super::super::union`]), a literal enum or an untagged enum. This module also names the
//! variants, orders the untagged variants for serde and adds the accessors of tagged enums.

use std::collections::BTreeSet;

use serde_json::Value;

use super::super::names;
use super::super::schema::{Components, ref_name, shape};
use super::super::union::{self, Discriminator};
use super::{
    AccessedField, Accessor, Body, Def, Field, Literal, Model, Tagged, TaggedVariant, Ty,
    UnionVariant, unwrapped,
};
use crate::Error;

impl Model {
    /// Lowers the members of the union `owner` (a schema with `anyOf`/`oneOf`, or with a list
    /// of types expanded into `members`).
    pub(super) fn lower_union(
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

    /// Methods of the tagged enum `name` reading the properties its members share: those of
    /// the fallback member (every object has them; members lacking one return `None`), or
    /// without a fallback those every member declares. The discriminator is read by `kind`.
    pub(super) fn accessors(&self, name: &str, tagged: &Tagged) -> Result<Vec<Accessor>, Error> {
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
}

/// The fallback members of the tagged unions among `components` that are component schemas:
/// their structs keep unknown properties.
pub fn fallback_components(components: &Components) -> BTreeSet<String> {
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
