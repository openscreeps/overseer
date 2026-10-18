//! Rust source of tagged and literal enums, which need hand-written serde impls.
//!
//! A tagged enum serializes as its member (`#[serde(untagged)]`, members keep the discriminator
//! field) and deserializes by reading the discriminator first. JSON objects are unordered and
//! the discriminator often follows other properties (room objects start with `_id`), so the
//! object is buffered as a `serde_json::Map` and the selected member deserialized from it; no
//! member is tried and discarded. A literal enum visits the scalar directly.

use std::fmt::Write as _;

use super::{doc, variant_doc};
use crate::codegen::model::{
    AccessedField, Accessor, Literal, Model, Tagged, Ty, UnionVariant, is_copy_literal, unwrapped,
};

/// Discriminator values listed in a variant's documentation; longer lists are summarised.
const LISTED_VALUES: usize = 8;

/// A tagged enum: serialized as its member, deserialized by the discriminator, with accessors.
pub fn tagged(out: &mut String, model: &Model, name: &str, tagged: &Tagged) {
    let property = &tagged.property;
    out.push_str("#[derive(Clone, Debug, PartialEq, Serialize)]\n#[serde(untagged)]\n");
    w!(out, "pub enum {name} {{\n");
    for (index, variant) in tagged.variants.iter().enumerate() {
        let text = if tagged.fallback == Some(index) {
            format!("Any other `{property}`.")
        } else if variant.values.len() > LISTED_VALUES {
            format!(
                "`{property}` is one of {} values (see the member type).",
                variant.values.len()
            )
        } else {
            let values: Vec<String> = variant
                .values
                .iter()
                .map(|value| format!("`{value}`"))
                .collect();
            format!("`{property}` is {}.", values.join(", "))
        };
        doc(out, &text);
        w!(out, "{}({}),\n", variant.ident, variant.ty.render(""));
    }
    out.push_str("}\n\n");
    emit_deserialize(out, name, tagged);
    if !tagged.accessors.is_empty() {
        w!(out, "\nimpl {name} {{\n");
        for (index, accessor) in tagged.accessors.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            emit_accessor(out, model, tagged, accessor);
        }
        out.push_str("}\n");
    }
}

/// `Deserialize`: reads the discriminator, then deserializes the member it selects.
fn emit_deserialize(out: &mut String, name: &str, tagged: &Tagged) {
    let property = &tagged.property;
    w!(
        out,
        "impl<'de> Deserialize<'de> for {name} {{
fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {{
// `{property}` may follow the other properties, so the object is buffered before the member
// it selects is known.
let object = serde_json::Map::<String, serde_json::Value>::deserialize(deserializer)?;
let member = match object.get({property:?}).and_then(serde_json::Value::as_str) {{
"
    );
    for (index, variant) in tagged.variants.iter().enumerate() {
        if tagged.fallback == Some(index) {
            continue;
        }
        let values: Vec<String> = variant
            .values
            .iter()
            .map(|value| format!("{value:?}"))
            .collect();
        w!(out, "Some({}) => {index},\n", values.join(" | "));
    }
    if let Some(fallback) = tagged.fallback {
        w!(out, "Some(_) => {fallback},\n");
    } else {
        let values: Vec<String> = tagged
            .variants
            .iter()
            .flat_map(|variant| &variant.values)
            .map(|value| format!("{value:?}"))
            .collect();
        w!(
            out,
            "Some(other) => return Err(serde::de::Error::unknown_variant(other, &[{}])),\n",
            values.join(", ")
        );
    }
    w!(
        out,
        "None => return Err(serde::de::Error::missing_field({property:?})),\n}};\nlet object = serde_json::Value::Object(object);\nmatch member {{\n"
    );
    let last = tagged.variants.len() - 1;
    for (index, variant) in tagged.variants.iter().enumerate() {
        let pattern = if index == last {
            "_".to_owned()
        } else {
            index.to_string()
        };
        w!(
            out,
            "{pattern} => {}::deserialize(object).map(Self::{}),\n",
            type_path(&variant.ty),
            variant.ident
        );
    }
    out.push_str("}\n.map_err(serde::de::Error::custom)\n}\n}\n");
}

/// The path to call an associated function of `ty` through.
fn type_path(ty: &Ty) -> String {
    match ty {
        Ty::Boxed(inner) => format!("Box::<{}>", inner.render("")),
        other => other.render(""),
    }
}

/// One accessor method: a `match` reading the field of every member.
fn emit_accessor(out: &mut String, model: &Model, tagged: &Tagged, accessor: &Accessor) {
    let fallback = format!("The `{}` property.", accessor.json);
    let mut text = accessor.doc.clone().unwrap_or(fallback);
    if accessor.optional
        && accessor.fields.iter().any(Option::is_none)
        && !text.ends_with(['.', '!', '?'])
    {
        text.push('.');
    }
    if accessor.fields.iter().any(Option::is_none) {
        text.push_str("\n\n`None` for members without the property.");
    }
    doc(out, &text);
    let copy = is_copy(model, &accessor.ty);
    let mut returned = if accessor.ty == Ty::String {
        "&str".to_owned()
    } else if copy {
        accessor.ty.render("")
    } else {
        format!("&{}", accessor.ty.render(""))
    };
    if accessor.optional {
        returned = format!("Option<{returned}>");
    }
    w!(
        out,
        "#[must_use]\npub fn {}(&self) -> {returned} {{\nmatch self {{\n",
        accessor.ident
    );
    let mut absent = Vec::new();
    for (variant, field) in tagged.variants.iter().zip(&accessor.fields) {
        match field {
            Some(field) => w!(
                out,
                "Self::{}(member) => {},\n",
                variant.ident,
                read(model, accessor, field, copy)
            ),
            None => absent.push(format!("Self::{}(_)", variant.ident)),
        }
    }
    if !absent.is_empty() {
        w!(out, "{} => None,\n", absent.join(" | "));
    }
    out.push_str("}\n}\n");
}

/// The expression reading `field` of `member` as the accessor returns it.
fn read(model: &Model, accessor: &Accessor, field: &AccessedField, copy: bool) -> String {
    let place = format!("member.{}", field.ident);
    let optional = matches!(field.ty, Ty::Option(_));
    let inner = unwrapped(&field.ty);
    let boxed = matches!(&field.ty, Ty::Boxed(_))
        || matches!(&field.ty, Ty::Option(inner) if matches!(**inner, Ty::Boxed(_)));
    let expr = if accessor.ty == Ty::String && model.is_str_enum(inner) {
        if optional {
            format!("{place}.map({}::as_str)", inner.render(""))
        } else {
            format!("{place}.as_str()")
        }
    } else if copy {
        place
    } else if optional {
        if accessor.ty == Ty::String || boxed {
            format!("{place}.as_deref()")
        } else {
            format!("{place}.as_ref()")
        }
    } else {
        format!("&{place}")
    };
    if accessor.optional && !optional {
        format!("Some({expr})")
    } else {
        expr
    }
}

/// Whether accessors return `ty` by value.
fn is_copy(model: &Model, ty: &Ty) -> bool {
    matches!(ty, Ty::Bool | Ty::I64 | Ty::U64 | Ty::F64) || model.is_str_enum(ty)
}

/// A literal enum: scalars and unit variants for fixed strings.
pub fn literal(out: &mut String, name: &str, variants: &[Literal]) {
    let scalars: Vec<&UnionVariant> = variants
        .iter()
        .filter_map(|variant| match variant {
            Literal::Scalar(scalar) => Some(scalar),
            Literal::Const { .. } => None,
        })
        .collect();
    let has = |ty: Ty| scalars.iter().any(|scalar| scalar.ty == ty);
    let mut derives = vec!["Clone"];
    if is_copy_literal(variants) {
        derives.push("Copy");
    }
    derives.extend(["Debug", "PartialEq"]);
    if !has(Ty::F64) {
        derives.push("Eq");
    }
    w!(
        out,
        "#[derive({})]\npub enum {name} {{\n",
        derives.join(", ")
    );
    for variant in variants {
        match variant {
            Literal::Const { ident, value } => {
                doc(out, &format!("`{value}`."));
                w!(out, "{ident},\n");
            }
            Literal::Scalar(scalar) => {
                doc(out, &variant_doc(&scalar.ty));
                w!(out, "{}({}),\n", scalar.ident, scalar.ty.render(""));
            }
        }
    }
    out.push_str("}\n\n");
    w!(
        out,
        "impl Serialize for {name} {{\nfn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {{\nmatch self {{\n"
    );
    for variant in variants {
        match variant {
            Literal::Const { ident, value } => {
                w!(
                    out,
                    "Self::{ident} => serializer.serialize_str({value:?}),\n"
                );
            }
            Literal::Scalar(scalar) => {
                w!(
                    out,
                    "Self::{}(value) => value.serialize(serializer),\n",
                    scalar.ident
                );
            }
        }
    }
    out.push_str("}\n}\n}\n\n");
    let mut expecting: Vec<String> = Vec::new();
    for variant in variants {
        expecting.push(match variant {
            Literal::Const { value, .. } => format!("`{value}`"),
            Literal::Scalar(scalar) => match scalar.ty {
                Ty::Bool => "a boolean".to_owned(),
                Ty::I64 | Ty::U64 => "an integer".to_owned(),
                Ty::F64 => "a number".to_owned(),
                _ => "a string".to_owned(),
            },
        });
    }
    let expecting = match expecting.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} or {last}", rest.join(", ")),
        _ => expecting.join(""),
    };
    w!(
        out,
        "impl<'de> Deserialize<'de> for {name} {{
fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {{
struct LiteralVisitor;

impl serde::de::Visitor<'_> for LiteralVisitor {{
type Value = {name};

fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{
f.write_str({expecting:?})
}}
"
    );
    emit_visits(out, name, variants, &scalars);
    out.push_str("}\n\ndeserializer.deserialize_any(LiteralVisitor)\n}\n}\n");
}

/// The `visit_*` methods of a literal enum's visitor.
fn emit_visits(out: &mut String, name: &str, variants: &[Literal], scalars: &[&UnionVariant]) {
    const CAST: &str =
        "#[expect(clippy::cast_precision_loss, reason = \"JSON numbers are doubles\")]\n";
    let has = |ty: Ty| {
        scalars
            .iter()
            .find(|scalar| scalar.ty == ty)
            .map(|scalar| &scalar.ident)
    };
    let visit = |out: &mut String, attribute: &str, kind: &str, body: String| {
        w!(
            out,
            "\n{attribute}fn visit_{kind}<E: serde::de::Error>(self, value: {kind}) -> Result<{name}, E> {{\n{body}\n}}\n"
        );
    };
    let convert = |ident: &str, target: &str, unexpected: &str| {
        format!(
            "{target}::try_from(value).map({name}::{ident}).map_err(|_| E::invalid_value(serde::de::Unexpected::{unexpected}(value), &self))"
        )
    };
    if let Some(ident) = has(Ty::Bool) {
        visit(out, "", "bool", format!("Ok({name}::{ident}(value))"));
    }
    let (unsigned, signed, float) = (has(Ty::U64), has(Ty::I64), has(Ty::F64));
    match (unsigned, signed, float) {
        (Some(ident), _, _) => visit(out, "", "u64", format!("Ok({name}::{ident}(value))")),
        (None, Some(ident), _) => visit(out, "", "u64", convert(ident, "i64", "Unsigned")),
        (None, None, Some(ident)) => {
            visit(
                out,
                CAST,
                "u64",
                format!("Ok({name}::{ident}(value as f64))"),
            );
        }
        (None, None, None) => {}
    }
    match (signed, unsigned, float) {
        (Some(ident), _, _) => visit(out, "", "i64", format!("Ok({name}::{ident}(value))")),
        (None, Some(ident), _) => visit(out, "", "i64", convert(ident, "u64", "Signed")),
        (None, None, Some(ident)) => {
            visit(
                out,
                CAST,
                "i64",
                format!("Ok({name}::{ident}(value as f64))"),
            );
        }
        (None, None, None) => {}
    }
    if let Some(ident) = float {
        visit(out, "", "f64", format!("Ok({name}::{ident}(value))"));
    }
    w!(
        out,
        "\nfn visit_str<E: serde::de::Error>(self, value: &str) -> Result<{name}, E> {{\nmatch value {{\n"
    );
    for variant in variants {
        if let Literal::Const { ident, value } = variant {
            w!(out, "{value:?} => Ok({name}::{ident}),\n");
        }
    }
    match has(Ty::String) {
        Some(ident) => w!(out, "other => Ok({name}::{ident}(other.to_owned())),\n"),
        None => out.push_str(
            "other => Err(E::invalid_value(serde::de::Unexpected::Str(other), &self)),\n",
        ),
    }
    out.push_str("}\n}\n");
}
