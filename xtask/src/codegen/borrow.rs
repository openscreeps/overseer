//! Request bodies that borrow the values they send.
//!
//! A borrowing request body takes the lifetime `'a` and holds each field of a type that is not
//! `Copy` as `Cow<'a, …>`. The caller can then send its own strings, lists and objects without a
//! copy, and the endpoint that holds the body takes the same lifetime. A struct borrows only if
//! it is the request body of an operation and no type, reply or parameter of any document
//! mentions it. A type that holds the body would need the lifetime too. The decision depends on
//! the name only, so a module that re-exports the type of another module agrees with it.

use std::collections::BTreeSet;

use super::model::{Body, Model, Ty};
use super::ops::Operation;
use super::share::named;

/// The names of the request bodies of the operations of `documents` (each a model and its
/// operations) that no definition, reply or parameter of any of the documents mentions.
pub fn bodies<'d>(
    documents: impl IntoIterator<Item = (&'d Model, &'d [Operation])>,
) -> BTreeSet<String> {
    let mut bodies = BTreeSet::new();
    let mut mentioned = Vec::new();
    for (model, operations) in documents {
        for def in model.defs.values() {
            for ty in def.body.types() {
                named(ty, &mut mentioned);
            }
            if let Body::Tagged(tagged) = &def.body {
                for accessor in &tagged.accessors {
                    named(&accessor.ty, &mut mentioned);
                }
            }
        }
        for operation in operations {
            named(&operation.response, &mut mentioned);
            for param in &operation.params {
                named(&param.ty, &mut mentioned);
            }
            match &operation.body {
                Some((Ty::Named(name), _)) => {
                    bodies.insert(name.clone());
                }
                Some((ty, _)) => named(ty, &mut mentioned),
                None => {}
            }
        }
    }
    for name in mentioned {
        bodies.remove(name);
    }
    bodies
}

/// The Rust type of the field `ty` of a borrowing struct: a `Copy` type as it is, any other type
/// as `Cow<'a, …>` inside its `Option` layers. A `String` borrows as `str` and a `Vec` as a
/// slice, also through an alias.
pub fn field_type(model: &Model, ty: &Ty) -> String {
    match ty {
        _ if model.is_copy(ty) => ty.render(""),
        Ty::Option(inner) => format!("Option<{}>", field_type(model, inner)),
        _ => match model.resolve_alias(ty) {
            Ty::String => "Cow<'a, str>".to_owned(),
            Ty::Vec(item) => format!("Cow<'a, [{}]>", item.render("")),
            _ => format!("Cow<'a, {}>", ty.render("")),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::json;

    use super::{bodies, field_type};
    use crate::codegen::model::{Model, Ty};
    use crate::codegen::ops::{Auth, Format, Operation};

    /// A `POST` operation `name` with the body type `body` and the reply type `response`.
    fn operation(name: &str, body: Ty, response: Ty) -> Operation {
        Operation {
            id: name.to_owned(),
            name: name.to_owned(),
            method: "POST".to_owned(),
            path: format!("api/{name}"),
            summary: None,
            description: None,
            auth: Auth::Required,
            format: Format::Envelope,
            params: Vec::new(),
            body: Some((body, json!({}))),
            response,
        }
    }

    #[test]
    fn only_bodies_that_nothing_holds_borrow_their_fields() {
        let components = json!({
            "Prefs": {"type": "object", "properties": {"mail": {"type": "string"}}},
            "Me": {"type": "object", "properties": {
                "prefs": {"$ref": "#/components/schemas/Prefs"}
            }},
            "Shard": {"type": "string"},
            "Upload": {"type": "object", "properties": {
                "branch": {"type": "string"},
                "files": {"type": "array", "items": {"type": "string"}},
                "shard": {"$ref": "#/components/schemas/Shard"},
                "count": {"type": "integer"},
                "kind": {"enum": ["js", "wasm"]}
            }, "required": ["branch", "files"]},
            "Flag": {"type": "object", "properties": {"on": {"type": "boolean"}}}
        });
        let mut model = Model::new(components.as_object().unwrap().clone()).unwrap();
        let named = |name: &str| Ty::Named(name.to_owned());
        let operations = [
            operation("SetPrefs", named("Prefs"), named("Me")),
            operation("Upload", named("Upload"), Ty::Unit),
            operation("SetFlag", named("Flag"), Ty::Unit),
        ];
        let borrowed = bodies([(&model, operations.as_slice())]);
        // A reply holds the body `Prefs`: it stays owned.
        assert_eq!(
            borrowed,
            BTreeSet::from(["Flag".to_owned(), "Upload".to_owned()])
        );
        model.borrow(&borrowed);
        // `Flag` holds only a `Copy` field, so it needs no lifetime.
        assert!(!model.borrows("Flag"));
        assert!(model.borrows("Upload"));
        let rendered = |ty: &Ty| field_type(&model, ty);
        assert_eq!(rendered(&Ty::String), "Cow<'a, str>");
        assert_eq!(
            rendered(&Ty::Option(Box::new(Ty::Vec(Box::new(Ty::String))))),
            "Option<Cow<'a, [String]>>"
        );
        assert_eq!(rendered(&Ty::Option(Box::new(Ty::I64))), "Option<i64>");
        assert_eq!(rendered(&named("UploadKind")), "UploadKind");
        assert_eq!(
            rendered(&Ty::Option(Box::new(named("Shard")))),
            "Option<Cow<'a, str>>"
        );
        assert_eq!(rendered(&named("Me")), "Cow<'a, Me>");
    }
}
