use std::sync::Arc;

use serde_json::json;

use super::{Completion, Names, Needs, Suggestion, byte_offset, complete, utf16_len};

/// No labels.
const NONE: [&str; 0] = [];

/// The suggestions for `text` with the caret at its end.
fn at_end(text: &str, names: &Names) -> Completion {
    complete(text, utf16_len(text), names, &[])
}

/// The labels of `completion`.
fn labels(completion: &Completion) -> Vec<&str> {
    completion
        .suggestions
        .iter()
        .map(|suggestion| suggestion.label.as_str())
        .collect()
}

/// The suggestion labelled `label`.
fn find<'a>(completion: &'a Completion, label: &str) -> &'a Suggestion {
    completion
        .suggestions
        .iter()
        .find(|suggestion| suggestion.label == label)
        .unwrap_or_else(|| panic!("no suggestion {label} in {:?}", labels(completion)))
}

/// Names with creeps, a spawn, rooms and a `Memory`.
fn names() -> Names {
    Names {
        creeps: vec!["c100".to_owned(), "c101".to_owned(), "my creep".to_owned()],
        spawns: vec!["Spawn1".to_owned()],
        rooms: vec!["W1N1".to_owned()],
        memory: Some(Arc::new(json!({
            "creeps": { "c100": { "role": "harvester" } },
            "ticks": 12,
        }))),
    }
}

#[test]
fn narrows_to_the_members_of_a_known_object() {
    let names = Names::default();
    let completion = at_end("Game.c", &names);
    let some = labels(&completion);
    for member in ["creeps", "cpu", "constructionSites"] {
        assert!(some.contains(&member), "{member} in {some:?}");
    }
    // Names that start with the word come before names that contain it.
    assert_eq!(
        labels(&at_end("Game.cr", &names)),
        ["creeps", "powerCreeps"]
    );
    let completion = at_end("Game.cpu.", &names);
    let cpu = labels(&completion);
    assert!(
        cpu.contains(&"getUsed") && cpu.contains(&"bucket"),
        "{cpu:?}"
    );
    // A member of a creep, whatever its name; optional chaining and brackets too.
    assert_eq!(
        labels(&at_end("Game.creeps['c100'].moveT", &names)),
        ["moveTo"]
    );
    assert_eq!(labels(&at_end("Game?.cr", &names))[0], "creeps");
    assert_eq!(
        labels(&at_end("Game.rooms.W1N1.controller.lev", &names)),
        ["level"]
    );
}

#[test]
fn completes_constants_and_globals() {
    let names = Names::default();
    assert_eq!(
        labels(&at_end("FIND_SO", &names)),
        ["FIND_SOURCES", "FIND_SOURCES_ACTIVE"]
    );
    let resources = at_end("RESOURCE_", &names);
    assert_eq!(resources.suggestions.len(), super::LIMIT);
    assert_eq!(resources.suggestions[0].label, "RESOURCE_ENERGY");
    assert_eq!(resources.suggestions[0].detail, "\"energy\"");
    assert_eq!(
        find(&at_end("STRUCTURE_W", &names), "STRUCTURE_WALL").detail,
        "\"constructedWall\""
    );
    assert_eq!(
        find(&at_end("ERR_NOT_IN", &names), "ERR_NOT_IN_RANGE").detail,
        "-9"
    );
    assert_eq!(labels(&at_end("RANGED_", &names)), ["RANGED_ATTACK"]);
    assert_eq!(
        labels(&at_end("Mem", &names)),
        ["Memory", "RawMemory", "InterShardMemory"]
    );
    // The global scope needs a word: no list after a space.
    assert_eq!(labels(&at_end("Game.time + ", &names)), NONE);
}

#[test]
fn completes_own_names_and_asks_for_them() {
    let names = names();
    let creeps = at_end("Game.creeps.", &names);
    assert_eq!(labels(&creeps), ["c100", "c101", "my creep"]);
    assert_eq!(creeps.suggestions[0].detail, "creep");
    assert_eq!(
        creeps.needs,
        Needs {
            objects: true,
            memory: false
        }
    );
    assert_eq!(labels(&at_end("Game.spawns.S", &names)), ["Spawn1"]);
    assert_eq!(labels(&at_end("Game.rooms.w", &names)), ["W1N1"]);
    // Not loaded yet: nothing, but the names are asked for.
    let empty = at_end("Game.creeps.", &Names::default());
    assert_eq!(labels(&empty), NONE);
    assert!(empty.needs.objects);
    assert!(!at_end("Game.cpu.", &names).needs.objects);
}

#[test]
fn completes_memory_keys_at_any_depth() {
    let names = names();
    let root = at_end("Memory.", &names);
    assert_eq!(labels(&root), ["creeps", "ticks"]);
    assert_eq!(find(&root, "ticks").detail, "number");
    assert!(root.needs.memory);
    assert_eq!(labels(&at_end("Memory.creeps.c", &names)), ["c100"]);
    assert_eq!(labels(&at_end("Memory.creeps.c100.r", &names)), ["role"]);
    // A number has no keys.
    assert_eq!(labels(&at_end("Memory.ticks.", &names)), NONE);
    let unloaded = at_end("Memory.", &Names::default());
    assert_eq!(labels(&unloaded), NONE);
    assert!(unloaded.needs.memory);
}

#[test]
fn ranks_same_case_prefix_then_any_case_prefix_then_substring() {
    let names = Names {
        memory: Some(Arc::new(json!({
            "xalpha": 1, "Alpha": 1, "beta": 1, "alpha2": 1, "al": 1,
        }))),
        ..Names::default()
    };
    // The exact name is no suggestion.
    assert_eq!(
        labels(&at_end("Memory.al", &names)),
        ["alpha2", "Alpha", "xalpha"]
    );
    // One character matches prefixes only.
    let completion = at_end("Memory.a", &names);
    let one = labels(&completion);
    assert_eq!(one.len(), 3, "{one:?}");
    assert_eq!(one[2], "Alpha");
}

#[test]
fn finds_the_chain_at_the_caret() {
    let names = Names::default();
    // In the middle of a line, before other code.
    let text = "x(Game.cr, 1)";
    let completion = complete(text, 9, &names, &[]);
    let creeps = find(&completion, "creeps");
    assert_eq!((creeps.start, creeps.end), (7, 9));
    assert_eq!(creeps.apply(text), ("x(Game.creeps, 1)".to_owned(), 13));
    // On a line of a multi-line input.
    let text = "let a = 1;\nGame.cr\nfoo();";
    let completion = complete(text, 18, &names, &[]);
    assert_eq!(
        find(&completion, "creeps").apply(text),
        ("let a = 1;\nGame.creeps\nfoo();".to_owned(), 22)
    );
}

#[test]
fn counts_offsets_in_utf16_units() {
    let names = Names::default();
    // The emoji is 2 UTF-16 units and 4 bytes; `é` is 1 unit and 2 bytes.
    let text = "'🎉é'; Game.cr";
    let caret = utf16_len(text);
    assert_eq!(caret, 14);
    let completion = complete(text, caret, &names, &[]);
    let creeps = find(&completion, "creeps");
    assert_eq!((creeps.start, creeps.end), (12, 14));
    assert_eq!(creeps.apply(text), ("'🎉é'; Game.creeps".to_owned(), 18));
    // An offset inside the emoji is its start.
    assert_eq!(byte_offset(text, 2), 1);
    assert_eq!(byte_offset(text, 99), text.len());
}

#[test]
fn quotes_names_that_are_no_identifiers() {
    let names = names();
    let text = "Game.creeps.my";
    let completion = at_end(text, &names);
    let creep = find(&completion, "my creep");
    assert_eq!(creep.insert, "['my creep']");
    assert_eq!(
        creep.apply(text),
        ("Game.creeps['my creep']".to_owned(), 23)
    );
    let text = "Game.creeps?.my";
    let (applied, _) = find(&at_end(text, &names), "my creep").apply(text);
    assert_eq!(applied, "Game.creeps?.['my creep']");
}

#[test]
fn offers_nothing_in_strings_comments_and_unknown_objects() {
    let names = Names::default();
    for text in [
        "'Game.cr",
        "\"FIND_SO",
        "`x ${Game.cr",
        "// Game.cr",
        "/* FIND_SO",
        "foo().cr",
        "a[0].cr",
        "1.5",
        "x = 1.",
    ] {
        assert_eq!(labels(&at_end(text, &names)), NONE, "{text}");
    }
    // Code after a closed string and a closed comment.
    assert_eq!(
        labels(&at_end("'a' + /* b */ Game.cr", &names))[0],
        "creeps"
    );
}

#[test]
fn suggests_earlier_commands_that_start_with_the_input() {
    let names = Names::default();
    let history = [
        "Game.time".to_owned(),
        "Game.cpu.getUsed()".to_owned(),
        "Game.time + 1".to_owned(),
    ];
    let text = "Game.t";
    let completion = complete(text, utf16_len(text), &names, &history);
    assert_eq!(labels(&completion), ["time", "Game.time + 1", "Game.time"]);
    let command = find(&completion, "Game.time + 1");
    assert_eq!(command.detail, "history");
    assert_eq!(command.apply(text), ("Game.time + 1".to_owned(), 13));
    // Only with the caret at the end, and never the input itself.
    assert!(
        complete(text, 5, &names, &history)
            .suggestions
            .iter()
            .all(|suggestion| suggestion.detail != "history")
    );
    let text = "Game.time";
    assert_eq!(
        labels(&complete(text, utf16_len(text), &names, &history)),
        ["Game.time + 1"]
    );
}
