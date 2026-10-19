//! Code directories: which files are modules, source maps, stale deletion and round trips.

use std::fs;

use overseer::code::{ModuleKind, read_dir, write_dir};
use overseer::model::code::UserCodeModules;
use overseer::{Error, Modules};

use crate::{TempDir, WASM};

#[tokio::test]
async fn read_dir_reads_js_text_source_maps_and_wasm_bytes_only() {
    let dir = TempDir::new("read");
    dir.put("main.js", "module.exports.loop = () => {};");
    dir.put("lib.util.js", "// ü");
    dir.put("engine.wasm", WASM);
    dir.put("README.md", "notes");
    dir.put("main.js.map", "\n{\"version\":3}");
    dir.put("lib.util.js.map", "module.exports = {};");
    dir.put("main.js.LICENSE.txt", "ignored: not a module suffix");
    dir.put(
        "LOUD.JS",
        "ignored: the extension is matched case-sensitively",
    );
    dir.put(".eslintrc.js", "ignored: the server drops dotted names");
    dir.put("$tmp.js", "ignored: the server drops `$` names");
    fs::create_dir(dir.path().join("nested.js")).unwrap();

    let modules = read_dir(dir.path()).await.unwrap();

    assert_eq!(
        modules.iter().collect::<Vec<_>>(),
        [
            ("engine", ModuleKind::Wasm),
            ("lib.util", ModuleKind::Js),
            ("lib.util.js.map", ModuleKind::Js),
            ("main", ModuleKind::Js),
            ("main.js.map", ModuleKind::Js)
        ]
    );
    assert_eq!(modules.js("main"), Some("module.exports.loop = () => {};"));
    assert_eq!(modules.js("lib.util"), Some("// ü"));
    assert_eq!(modules.wasm("engine").unwrap().as_deref(), Some(&WASM[..]));
    // A JSON source map becomes a module that exports it; a source map module stays as it is.
    assert_eq!(
        modules.js("main.js.map"),
        Some("module.exports = \n{\"version\":3};")
    );
    assert_eq!(modules.js("lib.util.js.map"), Some("module.exports = {};"));
}

#[tokio::test]
async fn read_dir_rejects_two_files_of_one_module_and_non_utf8_sources() {
    for (name, first, second) in [
        ("both", "main.js", "main.wasm"),
        ("map", "main.js.map", "main.js.map.js"),
    ] {
        let dir = TempDir::new(name);
        dir.put(first, "");
        dir.put(second, WASM);
        let error = read_dir(dir.path()).await.unwrap_err();
        let message = error.to_string();
        assert!(matches!(error, Error::InvalidInput(_)), "{error:?}");
        assert!(
            message.contains(first) && message.contains(second),
            "{message}"
        );
    }

    let dir = TempDir::new("latin1");
    dir.put("main.js", [0xff, 0xfe]);
    assert!(matches!(
        read_dir(dir.path()).await,
        Err(Error::Io { path, .. }) if path == dir.path().join("main.js")
    ));

    let missing = dir.path().join("missing");
    assert!(matches!(
        read_dir(&missing).await,
        Err(Error::Io { path, .. }) if path == missing
    ));
}

#[tokio::test]
async fn write_dir_deletes_stale_modules_and_keeps_other_files() {
    let dir = TempDir::new("write");
    dir.put("main.js", "old main");
    dir.put("stale.js", "deleted: not in the branch");
    dir.put("old.wasm", WASM);
    dir.put("engine.js", "deleted: the module is now WebAssembly");
    dir.put("notes.txt", "kept");
    dir.put(".eslintrc.js", "kept: not a module file");
    dir.put("main.js.map", "deleted: the branch has no source map");
    dir.put("main.js.LICENSE.txt", "kept: not a module file");
    fs::create_dir(dir.path().join("lib.js")).unwrap();
    let mut modules = Modules::new();
    modules.insert_js("main", "new main");
    modules.insert_wasm("engine", &WASM);

    write_dir(dir.path(), &modules).await.unwrap();

    assert_eq!(
        dir.entries(),
        [
            ".eslintrc.js",
            "engine.wasm",
            "lib.js/",
            "main.js",
            "main.js.LICENSE.txt",
            "notes.txt"
        ]
    );
    assert_eq!(fs::read(dir.path().join("engine.wasm")).unwrap(), WASM);
    assert_eq!(
        fs::read_to_string(dir.path().join("main.js")).unwrap(),
        "new main"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join(".eslintrc.js")).unwrap(),
        "kept: not a module file"
    );
}

#[tokio::test]
async fn written_directories_read_back_the_same_modules() {
    let root = TempDir::new("round-trip");
    // A missing directory is created.
    let dir = root.path().join("a").join("b");
    let mut modules = Modules::new();
    modules.insert_js("main", "require('util');\n");
    modules.insert_js("util", "");
    modules.insert_js("ünïcode", "'✓'");
    modules.insert_js("main.js.map", "module.exports = {\"version\":3};");
    // `read_dir` would wrap this text in a `.js.map` file, so it goes to `raw.js.map.js`.
    modules.insert_js("raw.js.map", "{}");
    modules.insert_wasm("engine", &WASM);
    modules.insert_wasm("empty", &[]);

    write_dir(&dir, &modules).await.unwrap();
    assert_eq!(read_dir(&dir).await.unwrap(), modules);

    // Writing a smaller branch leaves exactly its modules.
    modules.remove("util");
    modules.remove("engine");
    write_dir(&dir, &modules).await.unwrap();
    assert_eq!(read_dir(&dir).await.unwrap(), modules);
}

#[tokio::test]
async fn rejected_modules_leave_the_directory_untouched() {
    let dir = TempDir::new("rejected");
    dir.put("stale.js", "kept: nothing is written");
    for name in ["", ".hidden", "$x", "lib/util", "lib\\util"] {
        let mut modules = Modules::new();
        modules.insert_js("main", "");
        modules.insert_js(name, "");
        assert!(
            matches!(
                write_dir(dir.path(), &modules).await,
                Err(Error::InvalidInput(_))
            ),
            "{name:?}"
        );
    }
    let mut modules = Modules::new();
    modules.insert_js("Main", "");
    modules.insert_js("main", "");
    assert!(matches!(
        write_dir(dir.path(), &modules).await,
        Err(Error::InvalidInput(_))
    ));
    let wire: UserCodeModules =
        serde_json::from_value(serde_json::json!({ "engine": { "binary": "not base64!" } }))
            .unwrap();
    assert!(matches!(
        write_dir(dir.path(), &Modules::from(wire)).await,
        Err(Error::InvalidInput(_))
    ));

    assert_eq!(dir.entries(), ["stale.js"]);
}
