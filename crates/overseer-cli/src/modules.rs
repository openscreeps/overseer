//! The modules of a build directory. [`overseer::code::read_dir`] reads the modules: each `.js`
//! file, each `.js.map` source map file and each `.wasm` file directly in the directory. This
//! module adds the report of the build: the file of each module, its size, and the entries that
//! are not modules. An upload replaces all modules of a branch. Thus a directory without a
//! module is an error.

use std::fmt;
use std::path::{Path, PathBuf};

use overseer::Modules;
use overseer::code::{ModuleKind, module_of, read_dir};

/// The kind of a module file, as the report shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A `.js` file.
    Js,
    /// A `.js.map` file: the module has the whole file name.
    SourceMap,
    /// A `.wasm` file.
    Wasm,
}

impl Kind {
    /// The kind of the file `file_name`, which holds a module of the kind `module`.
    fn of(file_name: &str, module: ModuleKind) -> Self {
        match module {
            ModuleKind::Wasm => Self::Wasm,
            ModuleKind::Js if file_name.ends_with(".js.map") => Self::SourceMap,
            ModuleKind::Js => Self::Js,
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(match self {
            Self::Js => "js",
            Self::SourceMap => "source map",
            Self::Wasm => "wasm",
        })
    }
}

/// The file of a module of a build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Module {
    /// The name of the module.
    pub(crate) name: String,
    /// The kind of the file.
    pub(crate) kind: Kind,
    /// The name of the file.
    pub(crate) file_name: String,
    /// The size of the file in bytes.
    pub(crate) len: u64,
}

/// Why an entry of the build directory is not a module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SkipReason {
    /// A directory or another entry that is not a file.
    NotAFile,
    /// A file whose name is not the name of a module file.
    NotAModule,
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotAFile => "not a file",
            Self::NotAModule => "not a module file (.js, .js.map or .wasm)",
        })
    }
}

/// An entry of the build directory that is not a module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Skipped {
    /// The entry.
    pub(crate) path: PathBuf,
    /// Why it is not a module.
    pub(crate) reason: SkipReason,
}

/// The modules of a build directory, the file of each, and the entries that are not modules.
#[derive(Debug)]
pub(crate) struct Build {
    /// The modules as the upload sends them.
    pub(crate) modules: Modules,
    /// The module files, in the order of their names.
    pub(crate) files: Vec<Module>,
    /// The entries that are not modules, in the order of their paths.
    pub(crate) skipped: Vec<Skipped>,
}

/// Reads the modules of the build directory `dir` and lists its entries.
///
/// # Errors
///
/// Returns an error when the directory or a module file cannot be read, when a `.js` or
/// `.js.map` file is not UTF-8, when two files hold one module, or when the directory has no
/// module.
pub(crate) async fn read(dir: &Path) -> Result<Build, Error> {
    let modules = read_dir(dir).await?;
    if modules.is_empty() {
        return Err(Error::Empty {
            dir: dir.to_owned(),
        });
    }
    let (files, skipped) = list(dir)?;
    Ok(Build {
        modules,
        files,
        skipped,
    })
}

/// Lists the module files and the other entries of `dir`. The module files are the files that
/// [`read_dir`] selects.
fn list(dir: &Path) -> Result<(Vec<Module>, Vec<Skipped>), Error> {
    let io = |path: &Path| {
        let path = path.to_owned();
        move |source| Error::Io { path, source }
    };
    let mut paths = std::fs::read_dir(dir)
        .and_then(|entries| {
            entries
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<Result<Vec<PathBuf>, _>>()
        })
        .map_err(io(dir))?;
    paths.sort();
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    for path in paths {
        let metadata = std::fs::metadata(&path).map_err(io(&path))?;
        if !metadata.is_file() {
            skipped.push(Skipped {
                path,
                reason: SkipReason::NotAFile,
            });
            continue;
        }
        let module = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|file_name| Some((file_name, module_of(file_name)?)));
        match module {
            Some((file_name, (name, kind))) => files.push(Module {
                name: name.to_owned(),
                kind: Kind::of(file_name, kind),
                file_name: file_name.to_owned(),
                len: metadata.len(),
            }),
            None => skipped.push(Skipped {
                path,
                reason: SkipReason::NotAModule,
            }),
        }
    }
    Ok((files, skipped))
}

/// A build directory that the CLI cannot use for an upload.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// The CLI cannot read the modules: a module file is not readable or not UTF-8, or two files
    /// hold one module.
    #[error(transparent)]
    Read(#[from] overseer::Error),
    /// An entry of the directory is not readable.
    #[error("{}: {source}", .path.display())]
    Io {
        /// The entry.
        path: PathBuf,
        /// The read error.
        source: std::io::Error,
    },
    /// The directory has no module.
    #[error(
        "{}: no .js, .js.map or .wasm file, and an upload replaces every module of a branch",
        .dir.display()
    )]
    Empty {
        /// The directory.
        dir: PathBuf,
    },
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// Makes a new, empty directory below the temporary directory. The test removes it.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "overseer-cli-modules-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create the test directory");
        dir
    }

    /// The report lists each module file with its kind and size, and each other entry with the
    /// reason that it is not a module.
    #[tokio::test]
    async fn build_lists_module_files_and_skipped_entries() {
        let dir = temp_dir("build");
        fs::write(dir.join("main.js"), "module.exports.loop = () => {};").unwrap();
        fs::write(dir.join("main.js.map"), r#"{"version":3}"#).unwrap();
        fs::write(dir.join("main_bg.wasm"), b"\0asm").unwrap();
        fs::write(dir.join("main.js.LICENSE.txt"), "notes").unwrap();
        fs::create_dir(dir.join("assets")).unwrap();

        let build = read(&dir).await.expect("a build");
        fs::remove_dir_all(&dir).unwrap();

        let files: Vec<_> = build
            .files
            .iter()
            .map(|file| {
                (
                    file.name.as_str(),
                    file.kind,
                    file.file_name.as_str(),
                    file.len,
                )
            })
            .collect();
        assert_eq!(
            files,
            [
                ("main", Kind::Js, "main.js", 31),
                ("main.js.map", Kind::SourceMap, "main.js.map", 13),
                ("main_bg", Kind::Wasm, "main_bg.wasm", 4),
            ]
        );
        let skipped: Vec<_> = build
            .skipped
            .iter()
            .map(|skipped| (skipped.path.file_name().unwrap().to_owned(), skipped.reason))
            .collect();
        assert_eq!(
            skipped,
            [
                ("assets".into(), SkipReason::NotAFile),
                ("main.js.LICENSE.txt".into(), SkipReason::NotAModule),
            ]
        );
        assert_eq!(
            build.modules.js("main.js.map"),
            Some(r#"module.exports = {"version":3};"#)
        );
    }

    /// A directory without a module is an error: the upload would delete every module of the
    /// branch.
    #[tokio::test]
    async fn a_directory_without_a_module_is_an_error() {
        let dir = temp_dir("empty");
        fs::write(dir.join("README.md"), "notes").unwrap();

        let error = read(&dir).await.expect_err("no module");
        fs::remove_dir_all(&dir).unwrap();

        assert!(matches!(error, Error::Empty { .. }), "{error:?}");
    }
}
