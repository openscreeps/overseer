//! The modules of a code branch.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use overseer_net::core::api::types::{UserCodeModules, UserCodeModulesValue, UserCodeWasmModule};
use overseer_net::core::socket::types as socket;

/// What a module holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModuleKind {
    /// JavaScript source text.
    Js,
    /// `WebAssembly` bytes (sent base64-encoded as `{binary}`).
    Wasm,
}

impl ModuleKind {
    /// The file extension of the module in a code directory: `js` or `wasm`.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Js => "js",
            Self::Wasm => "wasm",
        }
    }
}

/// A `WebAssembly` module whose `binary` is not standard base64.
#[derive(Debug, thiserror::Error)]
#[error("wasm module `{module}` is not valid base64: {source}")]
pub struct InvalidWasm {
    module: String,
    #[source]
    source: base64::DecodeError,
}

impl InvalidWasm {
    /// The module's name.
    #[must_use]
    pub fn module(&self) -> &str {
        &self.module
    }
}

/// The modules of a code branch by name: JavaScript source text or `WebAssembly` bytes.
///
/// Wraps the wire form ([`UserCodeModules`]), in which `WebAssembly` modules are base64 text:
/// [`Self::insert_wasm`] encodes and [`Self::wasm`] decodes it. Names are kept as given; the
/// server drops modules whose names start with `$`, `.`, `/` or `\` and adds an empty `main`
/// module when there is none.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Modules(UserCodeModules);

impl Modules {
    /// No modules.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of modules.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are no modules.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Every module's name and kind, ordered by name.
    pub fn iter(&self) -> impl Iterator<Item = (&str, ModuleKind)> {
        self.0
            .iter()
            .map(|(name, value)| (name.as_str(), kind(value)))
    }

    /// The kind of module `name`; `None` when there is no such module.
    #[must_use]
    pub fn kind(&self, name: &str) -> Option<ModuleKind> {
        self.0.get(name).map(kind)
    }

    /// The source text of the JavaScript module `name`; `None` when there is no such module or
    /// it is a `WebAssembly` module.
    #[must_use]
    pub fn js(&self, name: &str) -> Option<&str> {
        match self.0.get(name)? {
            UserCodeModulesValue::String(source) => Some(source),
            UserCodeModulesValue::WasmModule(_) => None,
        }
    }

    /// The decoded bytes of the `WebAssembly` module `name`; `None` when there is no such
    /// module or it is a JavaScript module.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidWasm`] when the module's `binary` is not standard base64 (the server
    /// stores what clients upload without checking it).
    pub fn wasm(&self, name: &str) -> Result<Option<Vec<u8>>, InvalidWasm> {
        match self.0.get(name) {
            Some(UserCodeModulesValue::WasmModule(module)) => {
                decode(name, &module.binary).map(Some)
            }
            Some(UserCodeModulesValue::String(_)) | None => Ok(None),
        }
    }

    /// Sets module `name` to the JavaScript `source`, replacing any module of that name.
    pub fn insert_js(&mut self, name: impl Into<String>, source: impl Into<String>) {
        self.0
            .insert(name.into(), UserCodeModulesValue::String(source.into()));
    }

    /// Sets module `name` to the `WebAssembly` `bytes` (base64-encoded), replacing any module of
    /// that name.
    pub fn insert_wasm(&mut self, name: impl Into<String>, bytes: &[u8]) {
        let binary = STANDARD.encode(bytes);
        self.0.insert(
            name.into(),
            UserCodeModulesValue::WasmModule(UserCodeWasmModule { binary }),
        );
    }

    /// Removes module `name`; `false` when there was none.
    pub fn remove(&mut self, name: &str) -> bool {
        self.0.remove(name).is_some()
    }

    /// The wire form.
    #[must_use]
    pub fn as_map(&self) -> &UserCodeModules {
        &self.0
    }

    /// The modules of a `user:<user>/code` event.
    pub(crate) fn from_event(
        modules: std::collections::BTreeMap<String, socket::UserCodeModulesValue>,
    ) -> Self {
        Self(
            modules
                .into_iter()
                .map(|(name, value)| {
                    let value = match value {
                        socket::UserCodeModulesValue::String(source) => {
                            UserCodeModulesValue::String(source)
                        }
                        socket::UserCodeModulesValue::Object(object) => {
                            UserCodeModulesValue::WasmModule(UserCodeWasmModule {
                                binary: object.binary,
                            })
                        }
                    };
                    (name, value)
                })
                .collect(),
        )
    }
}

impl From<UserCodeModules> for Modules {
    fn from(modules: UserCodeModules) -> Self {
        Self(modules)
    }
}

impl From<Modules> for UserCodeModules {
    fn from(modules: Modules) -> Self {
        modules.0
    }
}

fn kind(value: &UserCodeModulesValue) -> ModuleKind {
    match value {
        UserCodeModulesValue::String(_) => ModuleKind::Js,
        UserCodeModulesValue::WasmModule(_) => ModuleKind::Wasm,
    }
}

fn decode(name: &str, binary: &str) -> Result<Vec<u8>, InvalidWasm> {
    STANDARD.decode(binary).map_err(|source| InvalidWasm {
        module: name.to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `\0asm` and version 1: the header of every `WebAssembly` module.
    const HEADER: [u8; 8] = [0, 0x61, 0x73, 0x6d, 1, 0, 0, 0];
    /// `HEADER` in standard base64 (generated with Python's `base64.b64encode`).
    const HEADER_BASE64: &str = "AGFzbQEAAAA=";

    #[test]
    fn wasm_modules_are_base64_on_the_wire() {
        let mut modules = Modules::new();
        modules.insert_js("main", "module.exports.loop = () => {};");
        modules.insert_wasm("engine", &HEADER);

        assert_eq!(
            serde_json::to_value(modules.as_map()).unwrap(),
            serde_json::json!({
                "engine": { "binary": HEADER_BASE64 },
                "main": "module.exports.loop = () => {};",
            })
        );
        assert_eq!(
            modules.wasm("engine").unwrap().as_deref(),
            Some(&HEADER[..])
        );
        assert_eq!(modules.js("main"), Some("module.exports.loop = () => {};"));
        assert_eq!(modules.js("engine"), None);
        assert_eq!(modules.wasm("main").unwrap(), None);
        assert_eq!(
            modules.iter().collect::<Vec<_>>(),
            [("engine", ModuleKind::Wasm), ("main", ModuleKind::Js)]
        );
    }

    #[test]
    fn modules_from_the_wire_decode_on_access() {
        let wire: UserCodeModules = serde_json::from_value(serde_json::json!({
            "engine": { "binary": HEADER_BASE64 },
            "broken": { "binary": "not base64!" },
            "main": "",
        }))
        .unwrap();
        let mut modules = Modules::from(wire);

        assert_eq!(modules.len(), 3);
        assert_eq!(modules.kind("engine"), Some(ModuleKind::Wasm));
        assert_eq!(
            modules.wasm("engine").unwrap().as_deref(),
            Some(&HEADER[..])
        );
        assert_eq!(modules.wasm("broken").unwrap_err().module(), "broken");
        assert!(modules.remove("broken"));
        assert!(!modules.remove("broken"));
        assert_eq!(modules.kind("broken"), None);
    }

    #[test]
    fn event_modules_become_wire_modules() {
        let event: socket::UserCode = serde_json::from_value(serde_json::json!({
            "branch": "default",
            "modules": { "main": "x", "engine": { "binary": HEADER_BASE64 } },
        }))
        .unwrap();
        let modules = Modules::from_event(event.modules);

        assert_eq!(modules.js("main"), Some("x"));
        assert_eq!(
            modules.wasm("engine").unwrap().as_deref(),
            Some(&HEADER[..])
        );
    }
}
