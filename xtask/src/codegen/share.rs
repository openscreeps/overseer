//! Sharing types between contract modules without merging their schemas.
//!
//! Every contract module is generated from its own document. A component whose schema has the
//! shape (annotations stripped, see [`shape`]) of the same-named component of a base module's
//! document is not generated again when it plays the same role in tagged unions and every
//! component it references is the same type in both modules: the module re-exports the base
//! module's type. Inline schemas (named after their location in a component or operation) are
//! shared the same way, by the definitions both modules' models lower them to. A types module
//! then emits the definitions its roots (its components and its operations' types) reach
//! without passing through a shared type.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::model::{Body, Model, Ty, fallback_components};
use super::schema::{Components, ref_name, shape};

/// Where the types of a module's components are defined.
#[derive(Clone, Debug)]
pub struct Paths {
    /// The Rust path of the module's types, such as `crate::api::official::types`.
    pub module: String,
    /// Components whose type another module defines, by name: the path of the defining type.
    pub shared: BTreeMap<String, String>,
}

impl Paths {
    /// A module defining every component type itself.
    pub fn new(module: impl Into<String>) -> Self {
        Self {
            module: module.into(),
            shared: BTreeMap::new(),
        }
    }

    /// The path of the type defining component `name`.
    pub fn canonical(&self, name: &str) -> String {
        self.shared
            .get(name)
            .cloned()
            .unwrap_or_else(|| format!("{}::{name}", self.module))
    }

    /// Shares with the module `base` (generated from `base_components`) every component of
    /// `local` that is not shared yet, has the shape of `base`'s component of the same name, is
    /// a fallback member of a tagged union in both documents or in neither (such a struct keeps
    /// unknown properties), and whose referenced components are the same types in both modules.
    pub fn share(&mut self, local: &Components, base_components: &Components, base: &Self) {
        let local_fallbacks = fallback_components(local);
        let base_fallbacks = fallback_components(base_components);
        let mut shared: BTreeMap<String, String> = local
            .iter()
            .filter(|(name, _)| !self.shared.contains_key(name.as_str()))
            .filter(|(name, schema)| {
                base_components
                    .get(name.as_str())
                    .is_some_and(|other| shape(schema) == shape(other))
                    && local_fallbacks.contains(name.as_str())
                        == base_fallbacks.contains(name.as_str())
            })
            .map(|(name, _)| (name.clone(), base.canonical(name)))
            .collect();
        loop {
            let resolve = |name: &str| {
                shared
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| self.canonical(name))
            };
            let diverged: Vec<String> = shared
                .keys()
                .filter(|name| {
                    let mut targets = Vec::new();
                    references(&local[name.as_str()], &mut targets);
                    targets
                        .iter()
                        .any(|target| resolve(target) != base.canonical(target))
                })
                .cloned()
                .collect();
            if diverged.is_empty() {
                break;
            }
            for name in diverged {
                shared.remove(&name);
            }
        }
        self.shared.extend(shared);
    }

    /// Shares with the module `base` (lowered to `base_model`) every inline definition of
    /// `local` (named after its location, not a component) that is not shared yet, was lowered
    /// from a schema of the shape of `base_model`'s definition of the same name, and whose
    /// referenced components are the same types in both modules. Run after [`Self::share`]: a
    /// definition nested in a shared component is shared too, so modules sharing with this one
    /// find its defining type. Definitions nested in a shared one have the same shape, so they
    /// are shared alike.
    pub fn share_inline(&mut self, local: &Model, base_model: &Model, base: &Self) {
        let shared: Vec<(String, String)> = local
            .inline_sources()
            .filter(|(name, _)| !self.shared.contains_key(*name))
            .filter(|(name, schema)| {
                base_model
                    .source(name)
                    .is_some_and(|other| shape(schema) == shape(other))
            })
            .filter(|(_, schema)| {
                let mut targets = Vec::new();
                references(schema, &mut targets);
                targets
                    .iter()
                    .all(|target| self.canonical(target) == base.canonical(target))
            })
            .map(|(name, _)| (name.to_owned(), base.canonical(name)))
            .collect();
        self.shared.extend(shared);
    }
}

/// Appends the component names `schema` references.
pub fn references<'v>(schema: &'v Value, out: &mut Vec<&'v str>) {
    match schema {
        Value::Object(map) => {
            for (key, value) in map {
                match (key.as_str(), value.as_str().and_then(ref_name)) {
                    ("$ref", Some(target)) => out.push(target),
                    _ => references(value, out),
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                references(item, out);
            }
        }
        _ => {}
    }
}

/// The definitions a types module emits and the types it re-exports.
#[derive(Debug, Default)]
pub struct Selection {
    /// Names of the definitions to emit.
    pub defs: BTreeSet<String>,
    /// Re-exported types: name to the path of the defining type.
    pub reexports: BTreeMap<String, String>,
}

impl Selection {
    /// Every definition of `model`.
    #[cfg(test)]
    pub fn all(model: &Model) -> Self {
        Self {
            defs: model.defs.keys().cloned().collect(),
            reexports: BTreeMap::new(),
        }
    }

    /// The definitions of `model` that `roots` reach, stopping at the types `shared` maps to
    /// the path of their type: those are re-exported, with the shared types they reference, so
    /// the module names every type its own types use.
    pub fn reached(model: &Model, roots: Vec<Ty>, shared: &BTreeMap<String, String>) -> Self {
        let mut selection = Self::default();
        // Each type with whether it was reached through a shared type.
        let mut pending: Vec<(Ty, bool)> = roots.into_iter().map(|ty| (ty, false)).collect();
        while let Some((ty, through_shared)) = pending.pop() {
            let mut names = Vec::new();
            named(&ty, &mut names);
            for name in names {
                let reached = if let Some(path) = shared.get(name) {
                    selection
                        .reexports
                        .insert(name.to_owned(), path.clone())
                        .is_none()
                } else {
                    // A type the shared one uses but this module does not share is defined by
                    // the module defining the shared type only.
                    !through_shared && selection.defs.insert(name.to_owned())
                };
                let Some(def) = model.defs.get(name).filter(|_| reached) else {
                    continue;
                };
                let inside = through_shared || shared.contains_key(name);
                pending.extend(def.body.types().into_iter().map(|ty| (ty.clone(), inside)));
                if let Body::Tagged(tagged) = &def.body {
                    for accessor in &tagged.accessors {
                        pending.push((accessor.ty.clone(), inside));
                    }
                }
            }
        }
        selection
    }
}

/// Appends the generated type names `ty` mentions.
fn named<'t>(ty: &'t Ty, out: &mut Vec<&'t str>) {
    match ty {
        Ty::Named(name) => out.push(name),
        Ty::Vec(inner)
        | Ty::Map(inner)
        | Ty::Option(inner)
        | Ty::Boxed(inner)
        | Ty::IndexedList(inner)
        | Ty::Array(inner, _) => named(inner, out),
        Ty::Tuple(items) => {
            for item in items {
                named(item, out);
            }
        }
        Ty::Bool
        | Ty::I64
        | Ty::U64
        | Ty::F64
        | Ty::String
        | Ty::Json
        | Ty::Unit
        | Ty::JsonMap
        | Ty::External(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::{Map, Value, json};

    use super::{Paths, Selection};
    use crate::codegen::emit::{self, Module};
    use crate::codegen::model::{Model, Ty};

    fn components(value: Value) -> Map<String, Value> {
        let Value::Object(map) = value else {
            panic!("components must be an object")
        };
        map
    }

    fn reference(name: &str) -> Value {
        json!({"$ref": format!("#/components/schemas/{name}")})
    }

    #[test]
    fn identical_components_are_reexported_and_differing_ones_generated() {
        let official = components(json!({
            "Badge": {"type": "object", "properties": {"color": {"type": "string"}}, "description": "Official."},
            "User": {"type": "object", "properties": {"badge": reference("Badge")}},
            "Room": {"type": "object", "properties": {"shard": {"type": "string"}}, "required": ["shard"]},
            "Rooms": {"type": "array", "items": reference("Room")},
            "Unused": {"type": "string"}
        }));
        let local = components(json!({
            // Documentation differs only: shared.
            "Badge": {"type": "object", "properties": {"color": {"type": "string"}}, "description": "Local."},
            "User": {"type": "object", "properties": {"badge": reference("Badge")}},
            // No shard on a single-world server: generated.
            "Room": {"type": "object", "properties": {"shard": {"type": "string"}}},
            // The same schema, but `Room` is another type here: generated too.
            "Rooms": {"type": "array", "items": reference("Room")},
            "Own": {"type": "object", "properties": {"rooms": reference("Rooms"), "user": reference("User")}}
        }));
        let base = Paths::new("crate::api::official::types");
        let mut paths = Paths::new("crate::api::community::types");
        paths.share(&local, &official, &base);
        assert_eq!(
            paths.shared.iter().collect::<Vec<_>>(),
            [
                (
                    &"Badge".to_owned(),
                    &"crate::api::official::types::Badge".to_owned()
                ),
                (
                    &"User".to_owned(),
                    &"crate::api::official::types::User".to_owned()
                ),
            ]
        );
        // A plugin sharing with the community module re-exports the type that defines it.
        let mut plugin = Paths::new("crate::api::plugins::x::types");
        plugin.share(&local, &local, &paths);
        assert_eq!(
            plugin.canonical("Badge"),
            "crate::api::official::types::Badge"
        );
        assert_eq!(
            plugin.canonical("Room"),
            "crate::api::community::types::Room"
        );
        let mut model = Model::new(local.clone()).unwrap();
        model.finish().unwrap();
        let roots = local.keys().map(|name| Ty::Named(name.clone())).collect();
        let selection = Selection::reached(&model, roots, &paths.shared);
        assert_eq!(
            selection.defs.iter().collect::<Vec<_>>(),
            ["Own", "Room", "Rooms"]
        );
        let module = Module {
            doc: "//! Test.\n",
            support: "crate::api",
        };
        let source = emit::types(&model, &module, &selection);
        assert!(source.contains("pub use crate::api::official::types::{Badge, User};"));
        assert!(source.contains("pub struct Room {"));
        assert!(source.contains("pub type Rooms = Vec<Room>;"));
        assert!(!source.contains("pub struct Badge"));
        // A plugin module emits only what its operations reach.
        let selection =
            Selection::reached(&model, vec![Ty::Named("Rooms".to_owned())], &paths.shared);
        assert_eq!(selection.defs.iter().collect::<Vec<_>>(), ["Room", "Rooms"]);
        assert_eq!(selection.reexports, BTreeMap::new());
    }

    #[test]
    fn identical_inline_schemas_and_official_only_components_are_reexported() {
        let renderer = |description: &str| {
            json!({"type": "object", "description": description,
                "properties": {"resources": {"type": "object", "additionalProperties": {"type": "string"}}},
                "required": ["resources"]})
        };
        let official = components(json!({
            "Version": {"type": "object", "properties": {
                "renderer": renderer("Official."),
                "shards": {"type": "array", "items": {"type": "string"}}}},
            "Token": {"type": "object", "properties": {"badge": reference("Badge")}},
            "Badge": {"type": "object", "properties": {"color": {"type": "string"}}}
        }));
        let community = components(json!({
            "Badge": {"type": "object", "properties": {"color": {"type": "string"}}}
        }));
        // A plugin on the community contract: its own `Version` (nullable shards) with the
        // official renderer, and a token type the community contract lacks.
        let plugin = components(json!({
            "Version": {"type": "object", "properties": {
                "renderer": renderer("Plugin."),
                "shards": {"type": "array", "items": {"type": ["string", "null"]}}}},
            "Token": {"type": "object", "properties": {"badge": reference("Badge")}},
            "Badge": {"type": "object", "properties": {"color": {"type": "string"}}}
        }));
        let lowered = |components: &Map<String, Value>| {
            let mut model = Model::new(components.clone()).unwrap();
            model.finish().unwrap();
            model
        };
        let (official_model, community_model, plugin_model) =
            (lowered(&official), lowered(&community), lowered(&plugin));
        let official_paths = Paths::new("crate::api::official::types");
        let mut community_paths = Paths::new("crate::api::community::types");
        community_paths.share(&community, &official, &official_paths);
        community_paths.share_inline(&community_model, &official_model, &official_paths);
        let mut paths = Paths::new("crate::api::plugins::x::types");
        paths.share(&plugin, &community, &community_paths);
        paths.share_inline(&plugin_model, &community_model, &community_paths);
        paths.share(&plugin, &official, &official_paths);
        paths.share_inline(&plugin_model, &official_model, &official_paths);
        assert_eq!(
            paths.shared.iter().collect::<Vec<_>>(),
            [
                (
                    &"Badge".to_owned(),
                    &"crate::api::official::types::Badge".to_owned()
                ),
                (
                    &"Token".to_owned(),
                    &"crate::api::official::types::Token".to_owned()
                ),
                (
                    &"VersionRenderer".to_owned(),
                    &"crate::api::official::types::VersionRenderer".to_owned()
                ),
            ]
        );
        // The module re-exports what its own types use, also through a shared type.
        let selection = Selection::reached(
            &plugin_model,
            vec![
                Ty::Named("Version".to_owned()),
                Ty::Named("Token".to_owned()),
            ],
            &paths.shared,
        );
        assert_eq!(selection.defs.iter().collect::<Vec<_>>(), ["Version"]);
        assert_eq!(
            selection.reexports.keys().collect::<Vec<_>>(),
            ["Badge", "Token", "VersionRenderer"]
        );
    }
}
