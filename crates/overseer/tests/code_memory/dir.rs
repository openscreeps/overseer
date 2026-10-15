//! Code directories: which files are modules, stale deletion and round trips.

use std::fs;

use overseer::code::{ModuleKind, read_dir, write_dir};
use overseer::core::api::types::UserCodeModules;
use overseer::{Error, Modules};

use crate::{TempDir, WASM};

#[tokio::test]
async fn read_dir_reads_js_text_and_wasm_bytes_only() {
    let dir = TempDir::new("read");
    dir.put("main.js", "module.exports.loop = () => {};");
    dir.put("lib.util.js", "// ü");
    dir.put("engine.wasm", WASM);
    dir.put("README.md", "notes");
    dir.put("main.js.map", "{}");
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
            ("main", ModuleKind::Js)
        ]
    );
    assert_eq!(modules.js("main"), Some("module.exports.loop = () => {};"));
    assert_eq!(modules.js("lib.util"), Some("// ü"));
    assert_eq!(modules.wasm("engine").unwrap().as_deref(), Some(&WASM[..]));
}

#[tokio::test]
async fn read_dir_rejects_a_module_in_both_kinds_and_non_utf8_sources() {
    let dir = TempDir::new("both");
    dir.put("main.js", "");
    dir.put("main.wasm", WASM);
    assert!(matches!(
        read_dir(dir.path()).await,
        Err(Error::InvalidInput(_))
    ));

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
    dir.put("main.js.map", "kept");
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
            "main.js.map",
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
