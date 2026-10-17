//! Operations of a document.

use serde_json::{Map, Value};

use super::model::{Body, Model, Ty, description};
use super::names;
use super::schema::{Components, ObjectShape, ref_name};
use crate::Error;

/// Headers the session sends itself; operations do not expose them as fields.
const SESSION_HEADERS: &[&str] = &["X-Username", "X-Server-Password"];

/// HTTP methods in the order operations of one path are generated.
const METHODS: &[&str] = &[
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// How a reply body is read; mirrors `overseer_core::api::Format`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// JSON with the `ok`/`error` envelope: every server's success reply carries `ok`.
    Envelope,
    /// A bare JSON document, also a reply of the envelope's shape without `ok` from some
    /// server.
    Json,
    /// Text such as SVG.
    Text,
    /// No documented body.
    Empty,
}

/// Token requirement; mirrors `overseer_core::api::Auth`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Auth {
    /// `security: []`.
    None,
    /// `security: [{}, {Token: []}]`.
    Optional,
    /// `security: [{Token: []}]`.
    Required,
}

/// Where a parameter goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Location {
    /// The query string.
    Query,
    /// A `{name}` segment of the path template.
    Path,
}

/// A query or path parameter.
#[derive(Clone, Debug)]
pub struct Param {
    /// The parameter name.
    pub name: String,
    /// The field identifier.
    pub ident: String,
    /// The value type: a string, number, boolean or string enum.
    pub ty: Ty,
    /// Whether the parameter must be sent.
    pub required: bool,
    /// Query or path.
    pub location: Location,
    /// The description.
    pub doc: Option<String>,
}

/// One operation.
#[derive(Clone, Debug)]
pub struct Operation {
    /// The `operationId`.
    pub id: String,
    /// The endpoint struct name.
    pub name: String,
    /// Upper-case HTTP method.
    pub method: String,
    /// The path template relative to the server base, without a leading `/`.
    pub path: String,
    /// The summary.
    pub summary: Option<String>,
    /// The description.
    pub description: Option<String>,
    /// Token requirement.
    pub auth: Auth,
    /// Reply format.
    pub format: Format,
    /// Parameters, in document order.
    pub params: Vec<Param>,
    /// The JSON request body type and its sample schema.
    pub body: Option<(Ty, Value)>,
    /// The success body type.
    pub response: Ty,
}

/// One document, for resolving references.
struct Document<'a> {
    root: &'a Value,
}

impl Document<'_> {
    /// Follows `$ref`s within the document (parameters, responses, schemas).
    fn resolve<'v>(&'v self, mut value: &'v Value) -> Result<&'v Value, Error> {
        for _ in 0..16 {
            let Some(reference) = value.get("$ref").and_then(Value::as_str) else {
                return Ok(value);
            };
            let pointer = reference
                .strip_prefix('#')
                .ok_or_else(|| format!("external $ref {reference}"))?;
            value = self
                .root
                .pointer(pointer)
                .ok_or_else(|| format!("dangling $ref {reference}"))?;
        }
        Err("$ref chain too long".into())
    }

    /// Every operation: method, path and operation object, in path then method order.
    fn operations(&self) -> Vec<(&str, &str, &Value)> {
        let mut operations = Vec::new();
        if let Some(paths) = self.root.get("paths").and_then(Value::as_object) {
            for (path, item) in paths {
                for method in METHODS {
                    if let Some(operation) = item.get(*method) {
                        operations.push((*method, path.as_str(), operation));
                    }
                }
            }
        }
        operations
    }

    /// The non-header parameters of an operation, resolved.
    fn params<'v>(&'v self, operation: &'v Value) -> Result<Vec<&'v Value>, Error> {
        let mut params = Vec::new();
        for param in operation
            .get("parameters")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let param = self.resolve(param)?;
            let name = param
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match param.get("in").and_then(Value::as_str) {
                Some("header") if SESSION_HEADERS.contains(&name) => {}
                Some("query" | "path") => params.push(param),
                other => return Err(format!("unsupported parameter {name} in {other:?}").into()),
            }
        }
        Ok(params)
    }

    /// The reply format and success schema (without the `ApiError` alternative).
    fn response(&self, operation: &Value) -> Result<(Format, Option<Value>), Error> {
        let Some(success) = operation.pointer("/responses/200") else {
            return Ok((Format::Empty, None));
        };
        let success = self.resolve(success)?;
        let Some(content) = success.get("content").and_then(Value::as_object) else {
            return Ok((Format::Empty, None));
        };
        if let Some(json) = content.get("application/json") {
            let schema = json.get("schema").ok_or("JSON response without schema")?;
            return Ok(match without_api_error(schema) {
                Some(schema) => (Format::Envelope, Some(schema)),
                None => (Format::Json, Some(schema.clone())),
            });
        }
        if content
            .keys()
            .any(|kind| kind.starts_with("text/") || kind.ends_with("+xml"))
        {
            return Ok((Format::Text, None));
        }
        Err(format!(
            "unsupported response content {:?}",
            content.keys().collect::<Vec<_>>()
        )
        .into())
    }

    /// Why the endpoint runtime cannot express `operation`, if it cannot: a success body that
    /// is neither JSON nor text (`Format` has no bytes), or a path parameter spanning several
    /// segments (`x-openscreeps-wildcard`; a `Path` segment is percent-encoded whole).
    fn unsupported(&self, operation: &Value) -> Result<Option<String>, Error> {
        for param in self.params(operation)? {
            if param.get("x-openscreeps-wildcard") == Some(&Value::Bool(true)) {
                let name = param
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                return Ok(Some(format!("wildcard path parameter `{name}`")));
            }
        }
        let Some(success) = operation.pointer("/responses/200") else {
            return Ok(None);
        };
        let content = self
            .resolve(success)?
            .get("content")
            .and_then(Value::as_object);
        let Some(content) = content else {
            return Ok(None);
        };
        let readable = content.keys().any(|kind| {
            kind == "application/json" || kind.starts_with("text/") || kind.ends_with("+xml")
        });
        Ok((!readable).then(|| {
            let kinds: Vec<&str> = content.keys().map(String::as_str).collect();
            format!("binary response `{}`", kinds.join("`, `"))
        }))
    }

    /// The JSON request body schema.
    fn body<'v>(&'v self, operation: &'v Value) -> Result<Option<&'v Value>, Error> {
        let Some(body) = operation.get("requestBody") else {
            return Ok(None);
        };
        let body = self.resolve(body)?;
        body.pointer("/content/application~1json/schema")
            .map(Some)
            .ok_or_else(|| "request body without a JSON schema".into())
    }
}

/// `schema` without its `ApiError` alternative, or `None` when it has none.
fn without_api_error(schema: &Value) -> Option<Value> {
    let variants = schema.get("anyOf")?.as_array()?;
    let is_error = |variant: &Value| {
        variant
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(ref_name)
            == Some("ApiError")
    };
    if !variants.iter().any(is_error) {
        return None;
    }
    let rest: Vec<Value> = variants
        .iter()
        .filter(|variant| !is_error(variant))
        .cloned()
        .collect();
    if let [single] = rest.as_slice() {
        return Some(single.clone());
    }
    let mut map = Map::new();
    map.insert("anyOf".to_owned(), Value::Array(rest));
    Some(Value::Object(map))
}

/// Whether every success reply `schema` (resolved against `components`) describes requires
/// `ok`: an object requiring it, or a union whose every member but `null` (a reply without an
/// envelope, read as an absent body) does.
fn requires_ok(schema: &Value, components: &Components) -> bool {
    if let Some(target) = schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(ref_name)
        && let Some(target) = components.get(target)
        && target.get("anyOf").is_some()
    {
        return requires_ok(target, components);
    }
    if let Some(members) = schema.get("anyOf").and_then(Value::as_array) {
        let is_null = |member: &Value| member.get("type").and_then(Value::as_str) == Some("null");
        return members
            .iter()
            .filter(|member| !is_null(member))
            .all(|member| requires_ok(member, components));
    }
    ObjectShape::of(schema, components)
        .is_some_and(|shape| shape.required.iter().any(|name| name == "ok"))
}

/// Maps the security requirement alternatives.
fn auth(operation: &Value) -> Result<Auth, Error> {
    let security = operation
        .get("security")
        .and_then(Value::as_array)
        .ok_or("operation without security")?;
    let anonymous = security
        .iter()
        .any(|item| item.as_object().is_some_and(Map::is_empty));
    let token = security.iter().any(|item| item.get("Token").is_some());
    Ok(match (anonymous || security.is_empty(), token) {
        (_, false) => Auth::None,
        (true, true) => Auth::Optional,
        (false, true) => Auth::Required,
    })
}

/// Whether a composed plugin document marks `operation` as one its plugin provides, adapts or
/// adds (`x-plugins`).
pub fn is_plugin_operation(operation: &Value) -> bool {
    operation
        .get("x-plugins")
        .and_then(Value::as_array)
        .is_some_and(|plugins| !plugins.is_empty())
}

/// The operations built from a document, and those skipped as `"<operationId>: <reason>"`
/// because the endpoint runtime cannot express them (see `Document::unsupported`).
pub type Built = (Vec<Operation>, Vec<String>);

/// Builds the operations of `document` that `selected` accepts, lowering their types into
/// `model`.
pub fn operations(
    model: &mut Model,
    document: &Value,
    selected: fn(&Value) -> bool,
) -> Result<Built, Error> {
    let document = Document { root: document };
    let mut operations = Vec::new();
    let mut skipped = Vec::new();
    for (method, path, operation) in document.operations() {
        if !selected(operation) {
            continue;
        }
        let id =
            operation_id(operation).ok_or_else(|| format!("{method} {path} has no operationId"))?;
        if let Some(reason) = document.unsupported(operation)? {
            skipped.push(format!("{id}: {reason}"));
            continue;
        }
        operations.push(build(model, &document, method, path, operation)?);
    }
    Ok((operations, skipped))
}

fn operation_id(operation: &Value) -> Option<&str> {
    operation.get("operationId").and_then(Value::as_str)
}

/// Builds one operation.
fn build(
    model: &mut Model,
    document: &Document<'_>,
    method: &str,
    path: &str,
    operation: &Value,
) -> Result<Operation, Error> {
    let id = operation_id(operation).unwrap_or_default().to_owned();
    let name = names::type_name(&id);
    let context = |error: Error| -> Error { format!("operation {id}: {error}").into() };
    let auth = auth(operation).map_err(context)?;
    let (format, response) = document.response(operation).map_err(context)?;
    // An `{ok}` envelope is demanded only where every server's success reply carries `ok`.
    let format = match (format, &response) {
        (Format::Envelope, Some(schema)) if !requires_ok(schema, &model.components) => Format::Json,
        (format, _) => format,
    };
    let mut params = Vec::new();
    for param in document.params(operation).map_err(context)? {
        params.push(lower_param(model, &name, param).map_err(context)?);
    }
    for param in &params {
        let placeholder = format!("{{{}}}", param.name);
        if (param.location == Location::Path) != path.contains(&placeholder) {
            return Err(context(
                format!("path parameter {} does not match {path}", param.name).into(),
            ));
        }
    }
    let body = match document.body(operation).map_err(context)? {
        Some(schema) => {
            let fallback = format!("Request body of `endpoints::{name}`.");
            let ty = model
                .lower(schema, &format!("{name}Request"), &fallback)
                .map_err(context)?;
            Some((ty, schema.clone()))
        }
        None => None,
    };
    let response = match (format, response) {
        (Format::Text, _) => Ty::String,
        (_, None) => Ty::Unit,
        (_, Some(schema)) => {
            let fallback = format!("Reply of `endpoints::{name}`.");
            model
                .lower(&schema, &format!("{name}Response"), &fallback)
                .map_err(context)?
        }
    };
    Ok(Operation {
        id,
        name,
        method: method.to_ascii_uppercase(),
        path: path.trim_start_matches('/').to_owned(),
        summary: operation
            .get("summary")
            .and_then(Value::as_str)
            .map(str::to_owned),
        description: description(operation),
        auth,
        format,
        params,
        body,
        response,
    })
}

fn lower_param(model: &mut Model, operation: &str, param: &Value) -> Result<Param, Error> {
    let name = param
        .get("name")
        .and_then(Value::as_str)
        .ok_or("parameter without name")?;
    let location = match param.get("in").and_then(Value::as_str) {
        Some("path") => Location::Path,
        _ => Location::Query,
    };
    let required = param
        .get("required")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || location == Location::Path;
    let schema = param
        .get("schema")
        .ok_or_else(|| format!("parameter {name} without schema"))?;
    let fallback = format!("The `{name}` parameter.");
    let ty = model.lower(
        schema,
        &format!("{operation}{}", names::type_name(name)),
        &fallback,
    )?;
    let ty = model.resolve_alias(&ty);
    let scalar = match &ty {
        Ty::String | Ty::I64 | Ty::U64 | Ty::F64 | Ty::Bool => true,
        Ty::Named(enumeration) => matches!(
            model.defs.get(enumeration).map(|def| &def.body),
            Some(Body::StrEnum(_))
        ),
        _ => false,
    };
    if !scalar {
        return Err(format!("parameter {name} is not a scalar: {ty:?}").into());
    }
    Ok(Param {
        name: name.to_owned(),
        ident: names::field(name),
        ty,
        required,
        location,
        doc: description(param),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{Auth, Format, Location, is_plugin_operation, operations};
    use crate::codegen::model::{Model, Ty};
    use crate::codegen::schema::Components;

    fn schemas(document: &Value) -> &Components {
        document["components"]["schemas"].as_object().unwrap()
    }

    #[test]
    fn operations_map_security_parameters_and_formats() {
        let document = json!({
            "paths": {
                "/api/game/tick": {"get": {
                    "operationId": "gameTick",
                    "security": [],
                    "parameters": [{"name": "X-Server-Password", "in": "header", "schema": {"type": "string"}}],
                    "responses": {"200": {"content": {"application/json": {"schema": {"anyOf": [
                        {"type": "object", "properties": {"tick": {"type": "number"}}},
                        {"$ref": "#/components/schemas/ApiError"}
                    ]}}}}}
                }},
                "/api/user/rooms": {"get": {
                    "operationId": "userRooms",
                    "security": [{"Token": []}],
                    "parameters": [{"$ref": "#/components/parameters/LegacyUsername"}],
                    "responses": {"200": {"content": {"application/json": {"schema": {"anyOf": [
                        {"$ref": "#/components/schemas/Sharded"},
                        {"$ref": "#/components/schemas/ApiError"}
                    ]}}}}}
                }},
                "/room-history/{shard}/{room}/{tick}.json": {"get": {
                    "operationId": "roomHistory",
                    "security": [],
                    "parameters": [
                        {"name": "shard", "in": "path", "required": true, "schema": {"type": "string"}},
                        {"name": "room", "in": "path", "required": true, "schema": {"type": "string"}},
                        {"name": "tick", "in": "path", "required": true, "schema": {"type": "integer", "minimum": 0}}
                    ],
                    "responses": {"200": {"content": {"application/json": {"schema": {"type": "object"}}}}}
                }},
                "/api/user/badge-svg": {"get": {
                    "operationId": "userBadgeSvg",
                    "security": [{}, {"Token": []}],
                    "parameters": [{"name": "username", "in": "query", "required": true, "schema": {"type": "string"}}],
                    "responses": {"200": {"content": {"image/svg+xml": {"schema": {"type": "string"}}}}}
                }},
                "/api/servers/like": {"post": {
                    "operationId": "serversLike",
                    "security": [{"Token": []}],
                    "requestBody": {"content": {"application/json": {"schema": {"type": "object", "additionalProperties": true}}}},
                    "responses": {"200": {"description": "Not read."}}
                }}
            },
            "components": {
                "parameters": {"LegacyUsername": {"name": "X-Username", "in": "header", "schema": {"type": "string"}}},
                "schemas": {
                    "ApiError": {"type": "object", "properties": {"error": {"type": "string"}}, "required": ["error"]},
                    "Sharded": {"type": "object", "properties": {
                        "ok": {"type": "integer", "const": 1},
                        "shards": {"type": "object", "additionalProperties": {"type": "array", "items": {"type": "string"}}}
                    }, "required": ["ok", "shards"]}
                }
            }
        });
        let mut model = Model::new(schemas(&document).clone()).unwrap();
        let (operations, skipped) = operations(&mut model, &document, |_| true).unwrap();
        assert_eq!(skipped, Vec::<String>::new());
        let summary: Vec<(&str, &str, Auth, Format)> = operations
            .iter()
            .map(|operation| {
                (
                    operation.name.as_str(),
                    operation.path.as_str(),
                    operation.auth,
                    operation.format,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                // Its `{error}`-or-reply schema documents no `ok`: no envelope is demanded.
                ("GameTick", "api/game/tick", Auth::None, Format::Json),
                (
                    "ServersLike",
                    "api/servers/like",
                    Auth::Required,
                    Format::Empty
                ),
                (
                    "UserBadgeSvg",
                    "api/user/badge-svg",
                    Auth::Optional,
                    Format::Text
                ),
                (
                    "UserRooms",
                    "api/user/rooms",
                    Auth::Required,
                    Format::Envelope
                ),
                (
                    "RoomHistory",
                    "room-history/{shard}/{room}/{tick}.json",
                    Auth::None,
                    Format::Json
                ),
            ]
        );
        let history = &operations[4];
        assert!(
            history
                .params
                .iter()
                .all(|param| param.location == Location::Path && param.required)
        );
        assert_eq!(history.params[2].ty, Ty::U64);
        assert!(
            operations[0].params.is_empty() && operations[3].params.is_empty(),
            "session headers are not fields"
        );
        assert_eq!(operations[1].response, Ty::Unit);
        assert_eq!(operations[2].response, Ty::String);
        assert_eq!(operations[3].response, Ty::Named("Sharded".to_owned()));
    }

    #[test]
    fn plugin_modules_build_only_plugin_operations_and_skip_unsupported_ones() {
        let reply = json!({"200": {"content": {"application/json": {"schema": {"anyOf": [
            {"type": "object", "properties": {"ok": {"const": 1}, "name": {"type": "string"}}, "required": ["ok"]},
            {"$ref": "#/components/schemas/ApiError"}
        ]}}}}});
        let document = json!({
            "paths": {
                "/api/auth/me": {"get": {
                    "operationId": "authMe", "security": [{"Token": []}], "responses": reply
                }},
                "/api/authmod": {"get": {
                    "operationId": "authModInfo", "security": [], "responses": reply,
                    "x-plugins": [{"name": "screepsmod-auth", "status": "addition"}]
                }},
                "/api/user/name": {"get": {
                    "operationId": "userName", "security": [{"Token": []}], "responses": reply,
                    "x-plugins": [{"name": "screepsmod-auth", "status": "provided"}]
                }},
                "/assets/{path}": {"get": {
                    "operationId": "platformAsset", "security": [],
                    "parameters": [{"name": "path", "in": "path", "required": true,
                        "x-openscreeps-wildcard": true, "schema": {"type": "string"}}],
                    "responses": {"200": {"content": {"*/*": {"schema": {"type": "string", "format": "binary"}}}}},
                    "x-plugins": [{"name": "screepsmod-auth", "status": "provided"}]
                }}
            },
            "components": {"schemas": {
                "ApiError": {"type": "object", "properties": {"error": {"type": "string"}}, "required": ["error"]}
            }}
        });
        let mut model = Model::new(schemas(&document).clone()).unwrap();
        let (built, skipped) = operations(&mut model, &document, is_plugin_operation).unwrap();
        let names: Vec<&str> = built
            .iter()
            .map(|operation| operation.name.as_str())
            .collect();
        assert_eq!(names, ["AuthModInfo", "UserName"]);
        assert_eq!(built[0].format, Format::Envelope);
        assert_eq!(skipped, ["platformAsset: wildcard path parameter `path`"]);
        assert!(model.defs.contains_key("UserNameResponse"));
        assert!(!model.defs.contains_key("AuthMeResponse"));
    }
}
