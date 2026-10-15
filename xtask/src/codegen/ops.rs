//! Operations of the official and the standalone document.

use serde_json::{Map, Value};

use super::merge::{self, Components, ref_name, shape};
use super::model::{Body, Model, Ty, description};
use super::names;
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
    /// JSON with the `ok`/`error` envelope.
    Envelope,
    /// A bare JSON document.
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
    /// Whether only the standalone document has the operation.
    pub standalone_only: bool,
}

/// One document with its components, for resolving references.
struct Document<'a> {
    root: &'a Value,
    components: &'a Components,
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

/// Builds every operation of both documents, lowering their types into `model`.
pub fn operations(
    model: &mut Model,
    official: &Value,
    official_components: &Components,
    standalone: &Value,
    standalone_components: &Components,
) -> Result<Vec<Operation>, Error> {
    let official = Document {
        root: official,
        components: official_components,
    };
    let standalone = Document {
        root: standalone,
        components: standalone_components,
    };
    let standalone_ops: Vec<(&str, &str, &Value)> = standalone.operations();
    let official_ops = official.operations();
    let mut operations = Vec::new();
    for &(method, path, operation) in &official_ops {
        let id =
            operation_id(operation).ok_or_else(|| format!("{method} {path} has no operationId"))?;
        let twin = by_id(&standalone_ops, id).map(|twin| (&standalone, twin));
        operations.push(build(model, &official, method, path, operation, twin)?);
    }
    for &(method, path, operation) in &standalone_ops {
        let id =
            operation_id(operation).ok_or_else(|| format!("{method} {path} has no operationId"))?;
        if by_id(&official_ops, id).is_none() {
            let mut built = build(model, &standalone, method, path, operation, None)?;
            built.standalone_only = true;
            operations.push(built);
        }
    }
    Ok(operations)
}

fn operation_id(operation: &Value) -> Option<&str> {
    operation.get("operationId").and_then(Value::as_str)
}

/// The operation with `operationId` `id`.
fn by_id<'v>(operations: &[(&str, &str, &'v Value)], id: &str) -> Option<&'v Value> {
    operations
        .iter()
        .find(|(_, _, operation)| operation_id(operation) == Some(id))
        .map(|&(_, _, operation)| operation)
}

/// Builds one operation; `twin` is the same operation in the standalone document, whose
/// response may differ and is then merged.
fn build(
    model: &mut Model,
    document: &Document<'_>,
    method: &str,
    path: &str,
    operation: &Value,
    twin: Option<(&Document<'_>, &Value)>,
) -> Result<Operation, Error> {
    let id = operation_id(operation).unwrap_or_default().to_owned();
    let name = names::type_name(&id);
    let context = |error: Error| -> Error { format!("operation {id}: {error}").into() };
    let auth = auth(operation).map_err(context)?;
    let (format, mut response) = document.response(operation).map_err(context)?;
    if let Some((twin_document, twin)) = twin {
        if auth != self::auth(twin).map_err(context)? {
            return Err(context("security differs between the documents".into()));
        }
        let own_params: Vec<Value> = document
            .params(operation)
            .map_err(context)?
            .into_iter()
            .map(shape)
            .collect();
        let twin_params: Vec<Value> = twin_document
            .params(twin)
            .map_err(context)?
            .into_iter()
            .map(shape)
            .collect();
        if own_params != twin_params {
            return Err(context("parameters differ between the documents".into()));
        }
        let own_body = document.body(operation).map_err(context)?.map(shape);
        let twin_body = twin_document.body(twin).map_err(context)?.map(shape);
        if own_body != twin_body {
            return Err(context(
                "request bodies differ between the documents".into(),
            ));
        }
        let (twin_format, twin_response) = twin_document.response(twin).map_err(context)?;
        if twin_format != format {
            return Err(context("reply formats differ between the documents".into()));
        }
        if let (Some(own), Some(other)) = (&response, &twin_response) {
            response = Some(merge::merge(
                own,
                other,
                document.components,
                twin_document.components,
            ));
        }
    }
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
        standalone_only: false,
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

    use super::{Auth, Format, Location, operations};
    use crate::codegen::merge;
    use crate::codegen::model::{Body, Model, Ty};

    fn schemas(document: &Value) -> &merge::Components {
        document["components"]["schemas"].as_object().unwrap()
    }

    #[test]
    fn operations_map_security_parameters_formats_and_server_differences() {
        let official = json!({
            "paths": {
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
        let mut standalone = official.clone();
        standalone["paths"]["/api/user/rooms"]["get"]["responses"]["200"]["content"]["application/json"]
            ["schema"]["anyOf"][0] = json!({"$ref": "#/components/schemas/Flat"});
        standalone["components"]["schemas"]["Flat"] = json!({"type": "object", "properties": {
            "ok": {"type": "integer", "const": 1},
            "rooms": {"type": "array", "items": {"type": "string"}}
        }, "required": ["ok", "rooms"]});
        standalone["paths"]["/api/game/tick"] = json!({"get": {
            "operationId": "gameTick",
            "security": [],
            "parameters": [{"name": "X-Server-Password", "in": "header", "schema": {"type": "string"}}],
            "responses": {"200": {"content": {"application/json": {"schema": {"anyOf": [
                {"type": "object", "properties": {"tick": {"type": "number"}}},
                {"$ref": "#/components/schemas/ApiError"}
            ]}}}}}
        }});
        let mut model =
            Model::new(merge::components(schemas(&official), schemas(&standalone))).unwrap();
        let operations = operations(
            &mut model,
            &official,
            schemas(&official),
            &standalone,
            schemas(&standalone),
        )
        .unwrap();
        let summary: Vec<(&str, &str, Auth, Format, bool)> = operations
            .iter()
            .map(|operation| {
                (
                    operation.name.as_str(),
                    operation.path.as_str(),
                    operation.auth,
                    operation.format,
                    operation.standalone_only,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (
                    "ServersLike",
                    "api/servers/like",
                    Auth::Required,
                    Format::Empty,
                    false
                ),
                (
                    "UserBadgeSvg",
                    "api/user/badge-svg",
                    Auth::Optional,
                    Format::Text,
                    false
                ),
                (
                    "UserRooms",
                    "api/user/rooms",
                    Auth::Required,
                    Format::Envelope,
                    false
                ),
                (
                    "RoomHistory",
                    "room-history/{shard}/{room}/{tick}.json",
                    Auth::None,
                    Format::Json,
                    false
                ),
                (
                    "GameTick",
                    "api/game/tick",
                    Auth::None,
                    Format::Envelope,
                    true
                ),
            ]
        );
        let history = &operations[3];
        assert!(
            history
                .params
                .iter()
                .all(|param| param.location == Location::Path && param.required)
        );
        assert_eq!(history.params[2].ty, Ty::U64);
        assert!(
            operations[2].params.is_empty(),
            "session headers are not fields"
        );
        assert_eq!(operations[0].response, Ty::Unit);
        assert_eq!(operations[1].response, Ty::String);
        // The official and standalone replies merge into one struct accepting both.
        assert_eq!(
            operations[2].response,
            Ty::Named("UserRoomsResponse".to_owned())
        );
        let Body::Struct(fields) = &model.defs["UserRoomsResponse"].body else {
            panic!("merged reply is a struct");
        };
        let fields: Vec<(&str, bool)> = fields
            .iter()
            .map(|field| (field.json.as_str(), matches!(field.ty, Ty::Option(_))))
            .collect();
        assert_eq!(fields, [("rooms", true), ("shards", true)]);
    }
}
