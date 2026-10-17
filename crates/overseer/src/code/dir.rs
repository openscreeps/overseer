//! Code directories in the official desktop client's layout: one `<module>.js` (source text)
//! or `<module>.wasm` (binary) file per module, directly in the directory.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::{ModuleKind, Modules};
use crate::Error;

/// Reads the modules of the code directory `dir`: every regular file (or link to one) named
/// `<module>.js` becomes a JavaScript module with the file's text (UTF-8), every
/// `<module>.wasm` file a `WebAssembly` module with the file's bytes. Subdirectories and other
/// files are ignored, and so are files the server would drop: names that are not UTF-8 and
/// module names that are empty or start with `$`, `.`, `/` or `\` (dotfiles such as
/// `.eslintrc.js`). Extensions are matched case-sensitively.
///
/// Runs on tokio's blocking thread pool. Native-only: browsers have no file system.
///
/// # Errors
///
/// Returns [`Error::Io`] when the directory or a module file cannot be read (a `.js` file that
/// is not UTF-8 fails with [`io::ErrorKind::InvalidData`]) and [`Error::InvalidInput`] when
/// `<module>.js` and `<module>.wasm` both exist.
pub async fn read_dir(dir: impl AsRef<Path>) -> Result<Modules, Error> {
    let dir = dir.as_ref().to_owned();
    blocking(dir.clone(), move || read(&dir)).await
}

/// Writes `modules` into the code directory `dir` (created when missing) so that
/// [`read_dir`] reads them back: first deletes every module file of `dir` (as [`read_dir`]
/// selects them) that is not one of `modules` with the same kind, then writes each module to
/// `<module>.js` or `<module>.wasm`, replacing existing files. No other file is touched.
///
/// Every module is checked before anything is deleted or written. Module names must be valid
/// single file names that the server keeps: not empty, not starting with `$` or `.`, and
/// without `/`, `\` or NUL. Two names that differ only in case are rejected as well, because
/// case-insensitive file systems (the Windows and macOS defaults) would store them in one
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

/// The module a file of a code directory holds, by its file name; `None` for other files.
fn module_of(file_name: &str) -> Option<(&str, ModuleKind)> {
    let (name, kind) = if let Some(name) = file_name.strip_suffix(".js") {
        (name, ModuleKind::Js)
    } else {
        (file_name.strip_suffix(".wasm")?, ModuleKind::Wasm)
    };
    let dropped = name.is_empty() || name.starts_with(['$', '.', '/', '\\']);
    (!dropped).then_some((name, kind))
}

/// The module files of `dir`: `(path, module name, kind)` in no particular order.
fn module_files(dir: &Path) -> Result<Vec<(PathBuf, String, ModuleKind)>, Error> {
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
        if let Some((name, kind)) = module_of(&file_name)
            && path.is_file()
        {
            found.push((path, name.to_owned(), kind));
        }
    }
    Ok(found)
}

fn read(dir: &Path) -> Result<Modules, Error> {
    let mut modules = Modules::new();
    for (path, name, kind) in module_files(dir)? {
        if modules.kind(&name).is_some() {
            return Err(Error::InvalidInput(format!(
                "{} holds both `{name}.js` and `{name}.wasm`",
                dir.display()
            )));
        }
        match kind {
            ModuleKind::Js => {
                let source = fs::read_to_string(&path).map_err(|source| Error::Io {
                    path: path.clone(),
                    source,
                })?;
                modules.insert_js(name, source);
            }
            ModuleKind::Wasm => {
                let bytes = fs::read(&path).map_err(|source| Error::Io {
                    path: path.clone(),
                    source,
                })?;
                modules.insert_wasm(name, &bytes);
            }
        }
    }
    Ok(modules)
}

/// One file to write: its name, the module it holds and its contents.
struct File {
    module: String,
    kind: ModuleKind,
    contents: Vec<u8>,
}

impl File {
    fn name(&self) -> String {
        format!("{}.{}", self.module, self.kind.extension())
    }
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
        let contents = match kind {
            ModuleKind::Js => modules.js(name).unwrap_or_default().as_bytes().to_vec(),
            ModuleKind::Wasm => modules
                .wasm(name)
                .map_err(|error| Error::InvalidInput(error.to_string()))?
                .unwrap_or_default(),
        };
        files.push(File {
            module: name.to_owned(),
            kind,
            contents,
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
    for (path, name, kind) in module_files(dir)? {
        let kept = files
            .iter()
            .any(|file| file.module == name && file.kind == kind);
        if !kept {
            fs::remove_file(&path).map_err(|source| Error::Io { path, source })?;
        }
    }
    for file in files {
        let path = dir.join(file.name());
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
    fn module_files_are_js_and_wasm_files_the_server_keeps() {
        assert_eq!(module_of("main.js"), Some(("main", ModuleKind::Js)));
        assert_eq!(module_of("lib.util.js"), Some(("lib.util", ModuleKind::Js)));
        assert_eq!(module_of("engine.wasm"), Some(("engine", ModuleKind::Wasm)));
        assert_eq!(module_of("main.JS"), None);
        assert_eq!(module_of("main.js.map"), None);
        assert_eq!(module_of("README.md"), None);
        assert_eq!(module_of(".js"), None);
        assert_eq!(module_of(".eslintrc.js"), None);
        assert_eq!(module_of("$x.wasm"), None);
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
        let files = files(&modules).unwrap();
        assert_eq!(
            files
                .iter()
                .map(|file| (file.name(), file.contents.as_slice()))
                .collect::<Vec<_>>(),
            [
                ("main.wasm".to_owned(), &[0][..]),
                ("util.js".to_owned(), b"x")
            ]
        );
    }
}
