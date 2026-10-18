//! `cargo xtask codegen`: the typed HTTP and socket APIs of `overseer-core` from `reference/`.
//!
//! Every contract is generated on its own, as one module per document: `api::official`,
//! `api::openscreeps` and `api::community` (each `types` and `endpoints`, with the endpoints'
//! request tests), `api::plugins::<plugin>` (only the operations the plugin provides, adapts or
//! adds) and `socket::official`, `socket::openscreeps` and `socket::community` (`types`). No
//! schemas are merged across documents; a type the base document (the official one; for plugins
//! the community one, then the official one) defines identically, component or inline schema, is
//! re-exported from the base module instead of generated again (see [`share`]). Socket schemas
//! the contract's HTTP document defines identically are its HTTP types. A request body that no
//! other type holds borrows the values it sends (see [`borrow`]). Every file is formatted with
//! rustfmt.

mod borrow;
mod docs;
mod emit;
mod model;
mod names;
mod ops;
mod sample;
mod schema;
mod share;
mod socket;
mod union;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::Error;
use model::{Model, Ty};
use ops::Operation;
use schema::Components;
use share::{Paths, Selection};

/// The `overseer-core` sources, relative to the workspace root.
const CORE: &str = "crates/overseer-core/src";

/// A contract with an HTTP and a socket document.
struct Contract {
    /// The module name under `api` and `socket`.
    module: &'static str,
    /// The contract, as named in documentation.
    title: &'static str,
    /// The HTTP document, relative to `reference/`.
    http: &'static str,
    /// The socket document, relative to `reference/`.
    socket: &'static str,
}

/// The official contract: the base other contracts share types with.
const OFFICIAL: Contract = Contract {
    module: "official",
    title: "the official contract (screeps.com)",
    http: "openapi.json",
    socket: "asyncapi.json",
};

/// The `OpenScreeps` contract: the official one composed with the standalone profile.
const OPENSCREEPS: Contract = Contract {
    module: "openscreeps",
    title: "the `OpenScreeps` contract (the official one composed with the standalone profile)",
    http: "openscreeps.openapi.json",
    socket: "openscreeps.asyncapi.json",
};

/// The community contract: the official one composed with the community profile, the stock
/// Screeps server without mods; the base of the plugins.
const COMMUNITY: Contract = Contract {
    module: "community",
    title: "the community contract (the official one composed with the community profile: the \
            stock Screeps server without mods)",
    http: "community.openapi.json",
    socket: "community.asyncapi.json",
};

/// The plugin documents in `reference/`: `<plugin>.openapi.json`.
const PLUGINS: &str = "plugins";

/// The suffix of a plugin document's file name.
const PLUGIN_SUFFIX: &str = ".openapi.json";

/// Generated files: path relative to [`CORE`] and unformatted source.
type Files = Vec<(String, String)>;

/// Generates the files; with `check`, fails when a file on disk differs instead of writing.
pub fn run(root: &Path, check: bool) -> Result<(), Error> {
    let reference = root.join("reference");
    let mut files = Files::new();
    let report = contracts(&reference, &mut files)?;
    let mut stale = Vec::new();
    for (path, source) in files {
        let path = format!("{CORE}/{path}");
        let formatted = rustfmt(root, &source).map_err(|error| format!("{path}: {error}"))?;
        let target = root.join(&path);
        let current = fs::read_to_string(&target).ok();
        if current.as_deref() == Some(formatted.as_str()) {
            continue;
        }
        if check {
            stale.push(path);
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&target, formatted)?;
            println!("wrote {path}");
        }
    }
    if !stale.is_empty() {
        return Err(format!(
            "stale generated files (run `cargo xtask codegen`): {}",
            stale.join(", ")
        )
        .into());
    }
    println!("{}:", if check { "Verified" } else { "Generated" });
    for line in report {
        println!("  {line}");
    }
    Ok(())
}

/// The per-contract modules. Returns one report line per module.
fn contracts(reference: &Path, files: &mut Files) -> Result<Vec<String>, Error> {
    let official = read_json(&reference.join(OFFICIAL.http))?;
    let openscreeps = read_json(&reference.join(OPENSCREEPS.http))?;
    let community = read_json(&reference.join(COMMUNITY.http))?;
    let mut official_lowered = lower(&official, false)?;
    let mut openscreeps_lowered = lower(&openscreeps, false)?;
    let mut community_lowered = lower(&community, false)?;
    let mut plugin_documents = plugins(reference)?
        .into_iter()
        .map(|plugin| {
            let document = read_json(&reference.join(plugin_file(&plugin)))?;
            let lowered = lower(&document, true)?;
            Ok((plugin, document, lowered))
        })
        .collect::<Result<Vec<_>, Error>>()?;
    // Every document takes part: a module re-exports the types of another module.
    let borrowed = borrow::bodies(
        [&official_lowered, &openscreeps_lowered, &community_lowered]
            .into_iter()
            .chain(plugin_documents.iter().map(|(_, _, lowered)| lowered))
            .map(|lowered| (&lowered.model, lowered.operations.as_slice())),
    );
    for lowered in [
        &mut official_lowered,
        &mut openscreeps_lowered,
        &mut community_lowered,
    ]
    .into_iter()
    .chain(plugin_documents.iter_mut().map(|(_, _, lowered)| lowered))
    {
        lowered.model.borrow(&borrowed);
    }
    let official_paths = Paths::new(types_path("api", OFFICIAL.module));
    let mut openscreeps_paths = Paths::new(types_path("api", OPENSCREEPS.module));
    openscreeps_paths.share(
        components(&openscreeps)?,
        components(&official)?,
        &official_paths,
    );
    openscreeps_paths.share_inline(
        &openscreeps_lowered.model,
        &official_lowered.model,
        &official_paths,
    );
    let mut community_paths = Paths::new(types_path("api", COMMUNITY.module));
    community_paths.share(
        components(&community)?,
        components(&official)?,
        &official_paths,
    );
    community_paths.share_inline(
        &community_lowered.model,
        &official_lowered.model,
        &official_paths,
    );
    let contracts = [
        (&OFFICIAL, &official, &official_lowered, &official_paths),
        (
            &OPENSCREEPS,
            &openscreeps,
            &openscreeps_lowered,
            &openscreeps_paths,
        ),
        (&COMMUNITY, &community, &community_lowered, &community_paths),
    ];
    let mut report = Vec::new();
    for (contract, document, lowered, paths) in contracts {
        let (title, file) = (contract.title, contract.http);
        let shared = if contract.module == OFFICIAL.module {
            ""
        } else {
            " Types the official document defines identically (components with every schema \
             they reference, and inline schemas) are re-exports of \
             [`crate::api::official::types`]."
        };
        let module = HttpModule {
            module: contract.module.to_owned(),
            root: module_doc(&[&format!(
                "The HTTP API of {title}, generated from `reference/{file}`: [`endpoints`] holds \
                 one [`Endpoint`](crate::api::Endpoint) per operation and [`types`] their schemas."
            )]),
            types: module_doc(&[
                &format!("Schema types of {title}, from `reference/{file}`."),
                &format!(
                    "One type per component schema, plus types for inline schemas named after \
                     their location.{shared} Unknown fields are ignored."
                ),
            ]),
            endpoints: module_doc(&[&format!(
                "Endpoints of {title}: one per operation of `reference/{file}`."
            )]),
            plugin: false,
        };
        report.push(http_module(files, &module, document, lowered, paths)?);
    }
    let mut root = module_doc(&[
        "HTTP APIs of community server plugins: one module per plugin, generated from \
         `reference/plugins/<plugin>.openapi.json` (the plugin's contract composed on the \
         community contract) with only the operations the plugin provides, adapts or adds.",
    ]);
    root.push('\n');
    for (plugin, document, lowered) in &plugin_documents {
        let file = plugin_file(plugin);
        let name = names::snake(plugin);
        let mut paths = Paths::new(types_path("api", &format!("{PLUGINS}::{name}")));
        // The plugin's contract is composed on the community one; what the community contract
        // lacks may still be the official type (API tokens, for example).
        paths.share(
            components(document)?,
            components(&community)?,
            &community_paths,
        );
        paths.share_inline(&lowered.model, &community_lowered.model, &community_paths);
        paths.share(
            components(document)?,
            components(&official)?,
            &official_paths,
        );
        paths.share_inline(&lowered.model, &official_lowered.model, &official_paths);
        let module = HttpModule {
            module: format!("{PLUGINS}::{name}"),
            root: module_doc(&[&format!(
                "The HTTP API of the {plugin} plugin, generated from `reference/{file}`: \
                 [`endpoints`] holds one [`Endpoint`](crate::api::Endpoint) per operation the \
                 plugin provides, adapts or adds and [`types`] their schemas."
            )]),
            types: module_doc(&[
                &format!(
                    "Schema types of the {plugin} plugin, from `reference/{file}` (the plugin \
                     composed on the community contract)."
                ),
                "The types the operations the plugin provides, adapts or adds need, plus types \
                 for inline schemas named after their location. Types the community document \
                 defines identically (components with every schema they reference, and inline \
                 schemas) are re-exported from the module defining the community type, and \
                 those only the official document defines identically from \
                 [`crate::api::official::types`]. Unknown fields are ignored.",
            ]),
            endpoints: module_doc(&[&format!(
                "Endpoints of the {plugin} plugin: one per operation it provides, adapts or adds \
                 (`x-plugins`) in `reference/{file}`."
            )]),
            plugin: true,
        };
        report.push(http_module(files, &module, document, lowered, &paths)?);
        let _ = writeln!(root, "pub mod {name};");
    }
    files.push((format!("api/{PLUGINS}.rs"), root));
    let contracts = [
        (&OFFICIAL, &official, &official_paths),
        (&OPENSCREEPS, &openscreeps, &openscreeps_paths),
        (&COMMUNITY, &community, &community_paths),
    ];
    report.extend(sockets(reference, files, contracts, &borrowed)?);
    Ok(report)
}

/// The socket modules of `contracts` (the official one first), each with its HTTP document
/// and the paths of its HTTP types. Returns one report line per module. A socket type cannot
/// be one of the `borrowed` request bodies: an event payload has no lifetime.
fn sockets(
    reference: &Path,
    files: &mut Files,
    contracts: [(&Contract, &Value, &Paths); 3],
    borrowed: &BTreeSet<String>,
) -> Result<Vec<String>, Error> {
    let official_document = read_json(&reference.join(OFFICIAL.socket))?;
    let official_components = components(&official_document)?;
    let mut official_paths = Paths::new(types_path("socket", OFFICIAL.module));
    let mut report = Vec::new();
    for (contract, http, http_paths) in contracts {
        let is_official = contract.module == OFFICIAL.module;
        let document = read_json(&reference.join(contract.socket))?;
        let socket_components = components(&document)?;
        // The schemas the contract's HTTP types stand for.
        let counterparts = socket::shared(socket_components, components(http)?)?;
        let mut paths = Paths::new(types_path("socket", contract.module));
        paths.shared = counterparts
            .iter()
            .map(|(name, counterpart)| (name.clone(), http_paths.canonical(counterpart)))
            .collect();
        if !is_official {
            paths.share(socket_components, official_components, &official_paths);
        }
        // HTTP types are named by path; official socket types are re-exported.
        let mut external = BTreeMap::new();
        let mut reexported = BTreeMap::new();
        for (name, path) in &paths.shared {
            if let Some(counterpart) = counterparts.get(name) {
                let http_types = types_path("api", contract.module);
                external.insert(name.clone(), format!("{http_types}::{counterpart}"));
            } else if path.starts_with("crate::api::") {
                external.insert(name.clone(), path.clone());
            } else {
                reexported.insert(name.clone(), path.clone());
            }
        }
        if let Some(body) = external
            .values()
            .filter_map(|path| path.rsplit("::").next())
            .find(|name| borrowed.contains(*name))
        {
            let module = contract.module;
            return Err(format!("socket {module}: a schema is the request body `{body}`").into());
        }
        let roots: Vec<Ty> = socket_components
            .keys()
            .filter(|name| !external.contains_key(name.as_str()))
            .map(|name| Ty::Named(name.clone()))
            .collect();
        let external_count = external.len();
        let mut model = Model::with_options(socket_components.clone(), socket::options(external))?;
        model.finish()?;
        let selection = Selection::reached(&model, roots, &reexported);
        let (title, file, module) = (contract.title, contract.socket, contract.module);
        let official_note = if is_official {
            ""
        } else {
            " Other schemas the official socket document defines identically, with every schema \
             they reference, are re-exports of [`crate::socket::official::types`]."
        };
        let types = module_doc(&[
            &format!("Schema types of the socket API of {title}, from `reference/{file}`."),
            &format!(
                "One type per component schema, plus types for inline schemas named after their \
                 location. Schemas the contract's HTTP document defines identically (room \
                 objects) are its [`crate::api::{module}::types`] types.{official_note} Optional \
                 properties that may be `null` are `Option<Option<T>>`: `None` when absent, \
                 `Some(None)` when `null`. Unknown fields are ignored."
            ),
        ]);
        let types_module = emit::Module {
            doc: &types,
            support: "crate::api",
        };
        let root = module_doc(&[&format!(
            "The socket API of {title}, generated from `reference/{file}`: [`types`] holds the \
             event payloads."
        )]);
        files.push((
            format!("socket/{module}.rs"),
            format!("{root}\npub mod types;\n"),
        ));
        files.push((
            format!("socket/{module}/types.rs"),
            emit::types(&model, &types_module, &selection),
        ));
        report.push(format!(
            "socket::{module}: {} components ({} HTTP types, {} re-exported, {} generated), {} \
             types defined",
            socket_components.len(),
            external_count,
            selection.reexports.len(),
            socket_components.len() - external_count - reexported.len(),
            selection.defs.len()
        ));
        if is_official {
            official_paths = paths;
        }
    }
    Ok(report)
}

/// The documentation and kind of one generated HTTP module.
struct HttpModule {
    /// The module path below `api`, such as `official` or `plugins::screepsmod_auth`.
    module: String,
    /// The documentation of the module root, as `//!` lines.
    root: String,
    /// The documentation of the types module, as `//!` lines.
    types: String,
    /// The first paragraph of the endpoints module's documentation, as `//!` lines.
    endpoints: String,
    /// Whether the module holds only the operations a plugin touches (`x-plugins`) and the
    /// types they need, rather than every operation and component.
    plugin: bool,
}

/// One HTTP document lowered: its type definitions, the operations generated and the reasons of
/// those skipped.
struct Lowered {
    model: Model,
    operations: Vec<Operation>,
    skipped: Vec<String>,
}

/// Lowers `document`: every operation, or only those a plugin touches (`x-plugins`).
fn lower(document: &Value, plugin: bool) -> Result<Lowered, Error> {
    let components = components(document)?;
    let mut model = Model::new(components.clone())?;
    let selected: fn(&Value) -> bool = if plugin {
        ops::is_plugin_operation
    } else {
        |_| true
    };
    let (operations, skipped) = ops::operations(&mut model, document, selected)?;
    unique(&operations)?;
    model.finish()?;
    Ok(Lowered {
        model,
        operations,
        skipped,
    })
}

/// Generates one HTTP module from `document`, lowered to `lowered`, re-exporting the types
/// `paths` shares. Returns the report line.
fn http_module(
    files: &mut Files,
    module: &HttpModule,
    document: &Value,
    lowered: &Lowered,
    paths: &Paths,
) -> Result<String, Error> {
    let components = components(document)?;
    let Lowered {
        model,
        operations,
        skipped,
    } = lowered;
    let mut roots: Vec<Ty> = Vec::new();
    if !module.plugin {
        roots.extend(components.keys().map(|name| Ty::Named(name.clone())));
    }
    for operation in operations {
        roots.push(operation.response.clone());
        roots.extend(operation.body.iter().map(|(ty, _)| ty.clone()));
        roots.extend(operation.params.iter().map(|param| param.ty.clone()));
    }
    let selection = Selection::reached(model, roots, &paths.shared);
    let directory = format!("api/{}", module.module.replace("::", "/"));
    files.push((
        format!("{directory}.rs"),
        format!("{}\npub mod endpoints;\npub mod types;\n", module.root),
    ));
    let types_module = emit::Module {
        doc: &module.types,
        support: "crate::api",
    };
    files.push((
        format!("{directory}/types.rs"),
        emit::types(model, &types_module, &selection),
    ));
    let endpoints_module = emit::Endpoints {
        doc: &module.endpoints,
        api: "crate::api",
    };
    files.push((
        format!("{directory}/endpoints.rs"),
        emit::endpoints(model, operations, &endpoints_module),
    ));
    files.push((
        format!("{directory}/endpoints/tests.rs"),
        emit::tests(model, operations, &types_path("api", &module.module)),
    ));
    let generated = components
        .keys()
        .filter(|name| selection.defs.contains(name.as_str()))
        .count();
    let reexported = selection
        .reexports
        .keys()
        .filter(|name| components.contains_key(name.as_str()))
        .count();
    Ok(format!(
        "api::{}: {} endpoints, {} components ({} re-exported, {} generated), {} types \
         defined, {} inline types re-exported{}",
        module.module,
        operations.len(),
        reexported + generated,
        reexported,
        generated,
        selection.defs.len(),
        selection.reexports.len() - reexported,
        skipped_note(skipped)
    ))
}

/// The report suffix naming the operations the generator skipped, with the reasons.
fn skipped_note(skipped: &[String]) -> String {
    if skipped.is_empty() {
        String::new()
    } else {
        format!("; skipped {}", skipped.join(", "))
    }
}

/// Module documentation: `//!` lines wrapped at 100 columns, a blank one between paragraphs.
fn module_doc(paragraphs: &[&str]) -> String {
    let mut out = String::new();
    for (index, paragraph) in paragraphs.iter().enumerate() {
        if index > 0 {
            out.push_str("//!\n");
        }
        let mut line = String::from("//!");
        for word in paragraph.split_whitespace() {
            if line.len() > 3 && line.len() + 1 + word.len() > 100 {
                out.push_str(&line);
                out.push('\n');
                line = String::from("//!");
            }
            line.push(' ');
            line.push_str(word);
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// The Rust path of the types module of `module` below `crate::<area>`.
fn types_path(area: &str, module: &str) -> String {
    format!("crate::{area}::{module}::types")
}

/// The plugins vendored in `reference/plugins/`, by name, sorted.
fn plugins(reference: &Path) -> Result<Vec<String>, Error> {
    let mut plugins = Vec::new();
    for entry in fs::read_dir(reference.join(PLUGINS))? {
        let file = entry?.file_name().to_string_lossy().into_owned();
        if let Some(plugin) = file.strip_suffix(PLUGIN_SUFFIX) {
            plugins.push(plugin.to_owned());
        }
    }
    plugins.sort();
    Ok(plugins)
}

/// The document of `plugin`, relative to `reference/`.
fn plugin_file(plugin: &str) -> String {
    format!("{PLUGINS}/{plugin}{PLUGIN_SUFFIX}")
}

/// Fails when two operations generate the same endpoint.
fn unique(operations: &[Operation]) -> Result<(), Error> {
    let mut names = BTreeSet::new();
    for operation in operations {
        if !names.insert(operation.name.as_str()) {
            return Err(format!("two operations generate the endpoint {}", operation.name).into());
        }
    }
    Ok(())
}

fn read_json(path: &Path) -> Result<Value, Error> {
    let text = fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?)
}

/// The component schemas of a document.
fn components(document: &Value) -> Result<&Components, Error> {
    document
        .pointer("/components/schemas")
        .and_then(Value::as_object)
        .ok_or_else(|| "document without components.schemas".into())
}

/// Formats `source` with the toolchain's rustfmt, as `cargo fmt` would.
fn rustfmt(root: &Path, source: &str) -> Result<String, Error> {
    let rustfmt = std::env::var_os("RUSTFMT").unwrap_or_else(|| "rustfmt".into());
    let mut child = Command::new(rustfmt)
        .args(["--edition", "2024"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot run rustfmt: {error}"))?;
    let mut stdin = child.stdin.take().ok_or("rustfmt has no stdin")?;
    let input = source.to_owned();
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let output = child.wait_with_output()?;
    writer
        .join()
        .map_err(|_| "rustfmt input thread panicked")??;
    if !output.status.success() {
        return Err(format!(
            "rustfmt failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?)
}
