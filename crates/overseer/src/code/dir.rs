//! Code directories: the official desktop client's layout (one `<module>.js` or `<module>.wasm`
//! file per module, directly in the directory) and the source maps that build tools write next
//! to the modules (`<module>.js.map`).

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::{ModuleKind, Modules};
use crate::Error;

/// The suffix of a source map file. The module of a source map file has the whole file name.
const SOURCE_MAP: &str = ".js.map";

/// Reads the modules of the code directory `dir`. A regular file (or a link to one) is a module
/// when its name ends in:
///
/// - `.js`: the JavaScript module of the name without `.js`, with the text of the file (UTF-8).
/// - `.js.map`: the JavaScript module of the whole name (`main.js.map`), so that
///   `require("main.js.map")` gives the source map. A text that starts with `{` (a JSON source
///   map) becomes `module.exports = <text>;`. Other text stays as it is.
/// - `.wasm`: the `WebAssembly` module of the name without `.wasm`, with the bytes of the file.
///
/// The function ignores subdirectories and other files. It also ignores the files that the
/// server would drop: names that are not UTF-8, and module names that are empty or start with
/// `$`, `.`, `/` or `\` (dotfiles such as `.eslintrc.js`). The extensions are case-sensitive.
/// [`module_of`] gives the module of a file name.
///
/// Runs on tokio's blocking thread pool. Native-only: browsers have no file system.
///
/// # Errors
///
/// Returns [`Error::Io`] when the directory or a module file cannot be read (a `.js` or `.js.map`
/// file that is not UTF-8 fails with [`io::ErrorKind::InvalidData`]). Returns
/// [`Error::InvalidInput`] when two files hold one module, for example `main.js` and
/// `main.wasm`.
pub async fn read_dir(dir: impl AsRef<Path>) -> Result<Modules, Error> {
    let dir = dir.as_ref().to_owned();
    blocking(dir.clone(), move || read(&dir)).await
}

/// Writes `modules` into the code directory `dir` (created when missing) so that [`read_dir`]
/// reads them back. First, the function deletes each module file of `dir` (as [`read_dir`]
/// selects them) that `modules` does not write. Then it writes each module to its file and
/// replaces the existing file. No other file changes. The file of a module is:
///
/// - `<module>.wasm` for a `WebAssembly` module;
/// - `<module>` for a JavaScript module whose name ends in `.js.map` and whose text does not
///   start with `{`, so that [`read_dir`] reads the text back as it is;
/// - `<module>.js` for each other JavaScript module.
///
/// The function checks every module before it deletes or writes a file. Module names must be
/// valid single file names that the server keeps: not empty, not starting with `$` or `.`, and
/// without `/`, `\` or NUL. The function also rejects two names that differ only in case,
/// because a case-insensitive file system (the Windows and macOS default) stores them in one
/// file.
///
/// Runs on tokio's blocking thread pool. Native-only: browsers have no file system.
///
/// # Errors
///
/// Returns [`Error::InvalidInput`] for a name rejected above or a `WebAssembly` module that is
/// not valid base64, and [`Error::Io`] when a file cannot be listed, deleted or written (files
/// handled before the failure stay changed).
pub async fn write_dir(dir: impl AsRef<Path>, modules: &Modules) -> Result<(), Error> {
    let files = files(modules)?;
    let dir = dir.as_ref().to_owned();
    blocking(dir.clone(), move || write(&dir, &files)).await
}

/// The module that the file `file_name` of a code directory holds, as [`read_dir`] selects it:
/// the module name and its kind. A source map file (`main.js.map`) holds the JavaScript module of
/// its whole name. Returns `None` for a file that is not a module.
#[must_use]
pub fn module_of(file_name: &str) -> Option<(&str, ModuleKind)> {
    file_kind(file_name).map(|(name, kind)| (name, kind.module()))
}

/// The kind of a module file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FileKind {
    /// `<module>.js`.
    Js,
    /// `<module>.js.map`: the module has the whole file name.
    SourceMap,
    /// `<module>.wasm`.
    Wasm,
}

impl FileKind {
    /// The kind of the module that a file of this kind holds.
    const fn module(self) -> ModuleKind {
        match self {
            Self::Js | Self::SourceMap => ModuleKind::Js,
            Self::Wasm => ModuleKind::Wasm,
        }
    }
}

/// The module name and the file kind of the file `file_name`; `None` for other files.
fn file_kind(file_name: &str) -> Option<(&str, FileKind)> {
    let (stem, name, kind) = if let Some(stem) = file_name.strip_suffix(SOURCE_MAP) {
        (stem, file_name, FileKind::SourceMap)
    } else if let Some(stem) = file_name.strip_suffix(".js") {
        (stem, stem, FileKind::Js)
    } else {
        let stem = file_name.strip_suffix(".wasm")?;
        (stem, stem, FileKind::Wasm)
    };
    let dropped = stem.is_empty() || stem.starts_with(['$', '.', '/', '\\']);
    (!dropped).then_some((name, kind))
}

/// A module file of a code directory.
struct ModuleFile {
    /// The path of the file.
    path: PathBuf,
    /// The name of the file.
    file_name: String,
    /// The module that the file holds.
    module: String,
    /// The kind of the file.
    kind: FileKind,
}

/// The module files of `dir`, in no particular order.
fn module_files(dir: &Path) -> Result<Vec<ModuleFile>, Error> {
    let io_error = |source| Error::Io {
        path: dir.to_owned(),
        source,
    };
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let path = entry.path();
        let Ok(file_name) = entry.file_name().into_string() else {
            continue;
        };
        if let Some((module, kind)) = file_kind(&file_name)
            && path.is_file()
        {
            let module = module.to_owned();
            found.push(ModuleFile {
                path,
                file_name,
                module,
                kind,
            });
        }
    }
    Ok(found)
}

fn read(dir: &Path) -> Result<Modules, Error> {
    let mut files = module_files(dir)?;
    // In name order, so that an error about two files of one module names them in a fixed order.
    files.sort_unstable_by(|a, b| a.file_name.cmp(&b.file_name));
    let mut modules = Modules::new();
    for file in &files {
        if modules.kind(&file.module).is_some() {
            let first = files
                .iter()
                .find(|other| other.module == file.module)
                .map_or("", |other| other.file_name.as_str());
            return Err(Error::InvalidInput(format!(
                "{} holds two files of the module `{}`: `{first}` and `{}`",
                dir.display(),
                file.module,
                file.file_name
            )));
        }
        let io_error = |source| Error::Io {
            path: file.path.clone(),
            source,
        };
        let module = file.module.clone();
        match file.kind {
            FileKind::Js => {
                modules.insert_js(module, fs::read_to_string(&file.path).map_err(io_error)?);
            }
            FileKind::SourceMap => {
                let text = fs::read_to_string(&file.path).map_err(io_error)?;
                let source = if text.trim_start().starts_with('{') {
                    format!("module.exports = {text};")
                } else {
                    text
                };
                modules.insert_js(module, source);
            }
            FileKind::Wasm => {
                modules.insert_wasm(module, &fs::read(&file.path).map_err(io_error)?);
            }
        }
    }
    Ok(modules)
}

/// One file to write: its name and its contents.
struct File {
    name: String,
    contents: Vec<u8>,
}

/// The files of `modules`, checked as [`write_dir`] documents.
fn files(modules: &Modules) -> Result<Vec<File>, Error> {
    let mut folded = BTreeMap::new();
    let mut files = Vec::with_capacity(modules.len());
    for (name, kind) in modules.iter() {
        if name.is_empty() || name.starts_with(['$', '.']) || name.contains(['/', '\\', '\0']) {
            return Err(Error::InvalidInput(format!(
                "module name `{name}` is not a file name the server keeps"
            )));
        }
        if let Some(other) = folded.insert(name.to_lowercase(), name) {
            return Err(Error::InvalidInput(format!(
                "module names `{other}` and `{name}` differ only in case"
            )));
        }
        files.push(match kind {
            ModuleKind::Js => {
                let source = modules.js(name).unwrap_or_default();
                // `read_dir` wraps a source map file that starts with `{`: such a text goes to a
                // `.js` file, which `read_dir` reads back as it is.
                let name = if name.ends_with(SOURCE_MAP) && !source.trim_start().starts_with('{') {
                    name.to_owned()
                } else {
                    format!("{name}.js")
                };
                File {
                    name,
                    contents: source.as_bytes().to_vec(),
                }
            }
            ModuleKind::Wasm => File {
                name: format!("{name}.wasm"),
                contents: modules
                    .wasm(name)
                    .map_err(|error| Error::InvalidInput(error.to_string()))?
                    .unwrap_or_default(),
            },
        });
    }
    Ok(files)
}

fn write(dir: &Path, files: &[File]) -> Result<(), Error> {
    fs::create_dir_all(dir).map_err(|source| Error::Io {
        path: dir.to_owned(),
        source,
    })?;
    // Delete before writing: on a case-insensitive file system a stale `Main.js` is the same
    // file as the new `main.js`, which must end up with the module's spelling.
    for found in module_files(dir)? {
        if !files.iter().any(|file| file.name == found.file_name) {
            fs::remove_file(&found.path).map_err(|source| Error::Io {
                path: found.path,
                source,
            })?;
        }
    }
    for file in files {
        let path = dir.join(&file.name);
        fs::write(&path, &file.contents).map_err(|source| Error::Io { path, source })?;
    }
    Ok(())
}

/// Runs `work` on the blocking thread pool; `dir` names the directory in the error reported
/// when the runtime shuts down first.
async fn blocking<T: Send + 'static>(
    dir: PathBuf,
    work: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, Error> {
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result,
        Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
        Err(error) => Err(Error::Io {
            path: dir,
            source: io::Error::other(error),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_files_are_js_source_map_and_wasm_files_the_server_keeps() {
        assert_eq!(module_of("main.js"), Some(("main", ModuleKind::Js)));
        assert_eq!(module_of("lib.util.js"), Some(("lib.util", ModuleKind::Js)));
        assert_eq!(module_of("engine.wasm"), Some(("engine", ModuleKind::Wasm)));
        assert_eq!(
            module_of("main.js.map"),
            Some(("main.js.map", ModuleKind::Js))
        );
        assert_eq!(
            module_of("main.js.map.js"),
            Some(("main.js.map", ModuleKind::Js))
        );
        assert_eq!(file_kind("main.js.map").unwrap().1, FileKind::SourceMap);
        assert_eq!(module_of("main.JS"), None);
        assert_eq!(module_of("main.js.LICENSE.txt"), None);
        assert_eq!(module_of("README.md"), None);
        assert_eq!(module_of(".js"), None);
        assert_eq!(module_of(".js.map"), None);
        assert_eq!(module_of(".eslintrc.js"), None);
        assert_eq!(module_of("$x.wasm"), None);
        assert_eq!(module_of("$x.js.map"), None);
        assert_eq!(module_of("\\x.js"), None);
    }

    #[test]
    fn unwritable_names_are_rejected() {
        for name in ["", ".hidden", "$x", "a/b", "a\\b", "a\0b"] {
            let mut modules = Modules::new();
            modules.insert_js(name, "");
            assert!(
                matches!(files(&modules), Err(Error::InvalidInput(_))),
                "{name:?}"
            );
        }
        let mut modules = Modules::new();
        modules.insert_js("Main", "");
        modules.insert_wasm("main", &[0]);
        assert!(matches!(files(&modules), Err(Error::InvalidInput(_))));
        modules.remove("Main");
        modules.insert_js("util", "x");
        // A source map module goes to its own name unless `read_dir` would wrap its text.
        modules.insert_js("util.js.map", "module.exports = {};");
        modules.insert_js("raw.js.map", " {}");
        let files = files(&modules).unwrap();
        assert_eq!(
            files
                .iter()
                .map(|file| (file.name.as_str(), file.contents.as_slice()))
                .collect::<Vec<_>>(),
            [
                ("main.wasm", &[0][..]),
                ("raw.js.map.js", b" {}"),
                ("util.js", b"x"),
                ("util.js.map", b"module.exports = {};"),
            ]
        );
    }
}
