//! Tests of the inspector: fields per kind, names and colours.

use std::collections::BTreeMap;

use overseer::model::world::{GameRoomObject, RoomUser};
use serde_json::{Value as Json, json};

use super::colours::{PART, part_class, resource_class};
use super::fields::inspect;
use super::{Context, Field, Part, Tone, Value, kind_label, title};
use crate::ui::player::Player;

fn object(value: Json) -> GameRoomObject {
    serde_json::from_value(value).unwrap()
}

fn users() -> BTreeMap<String, RoomUser> {
    let mut users = BTreeMap::new();
    users.insert(
        "u1".to_owned(),
        serde_json::from_value(json!({"_id": "u1", "username": "alice"})).unwrap(),
    );
    users.insert(
        "u2".to_owned(),
        serde_json::from_value(json!({"_id": "u2", "username": "bob"})).unwrap(),
    );
    users
}

fn field<'a>(fields: &'a [Field], label: &str) -> &'a Value {
    &fields
        .iter()
        .find(|field| field.label == label)
        .unwrap_or_else(|| panic!("no field {label} in {fields:?}"))
        .value
}

fn text(value: &str) -> Value {
    Value::Text(value.to_owned())
}

#[test]
fn creep_fields() {
    let users = users();
    let context = Context {
        game_time: Some(1000),
        me: Some("u1"),
        users: &users,
    };
    let creep = object(json!({
        "_id": "c1", "type": "creep", "room": "W1N1", "x": 7, "y": 43, "name": "w16",
        "user": "u1", "hits": 250, "hitsMax": 300, "ageTime": 2400, "fatigue": 2,
        "store": {"energy": 30, "H": null}, "storeCapacity": 50,
        "body": [
            {"type": "work", "hits": 100, "boost": "UO"},
            {"type": "carry", "hits": 100},
            {"type": "move", "hits": 0}
        ],
        "actionLog": {"say": {"message": "hi", "isPublic": true}}
    }));
    let inspected = inspect(&creep, &context);
    assert_eq!(inspected.title, "w16");
    assert_eq!(inspected.kind, "Creep");
    assert_eq!(
        inspected.owner,
        Some(Player {
            id: "u1".to_owned(),
            name: "alice".to_owned()
        })
    );
    assert!(inspected.own);
    let fields = &inspected.fields;
    assert_eq!(
        field(fields, "Hits"),
        &Value::Meter {
            value: 250,
            max: 300,
            tone: Tone::Hits
        }
    );
    assert_eq!(field(fields, "Ticks to live"), &text("1\u{202f}400"));
    assert_eq!(field(fields, "Fatigue"), &text("2"));
    assert_eq!(
        field(fields, "Store"),
        &Value::Store {
            resources: vec![("energy".to_owned(), 30)],
            used: 30,
            capacity: Some(50)
        }
    );
    assert_eq!(
        field(fields, "Body"),
        &Value::Body(vec![
            Part {
                kind: "work".to_owned(),
                boost: Some("UO".to_owned()),
                alive: true
            },
            Part {
                kind: "carry".to_owned(),
                boost: None,
                alive: true
            },
            Part {
                kind: "move".to_owned(),
                boost: None,
                alive: false
            },
        ])
    );
    assert_eq!(field(fields, "Saying"), &text("hi"));

    let spawning = object(json!({
        "_id": "c2", "type": "creep", "room": "W1N1", "x": 1, "y": 1, "name": "new",
        "user": "u2", "hits": 100, "hitsMax": 100, "spawning": true, "store": {}, "body": []
    }));
    let inspected = inspect(&spawning, &context);
    assert!(!inspected.own);
    assert_eq!(
        inspected.owner,
        Some(Player {
            id: "u2".to_owned(),
            name: "bob".to_owned()
        })
    );
    assert_eq!(field(&inspected.fields, "Ticks to live"), &text("Spawning"));
}

#[test]
fn spawn_and_extension_fields() {
    let users = users();
    let context = Context {
        game_time: Some(100),
        me: Some("u1"),
        users: &users,
    };
    let spawn = object(json!({
        "_id": "s1", "type": "spawn", "room": "W1N1", "x": 18, "y": 36, "name": "Spawn1",
        "user": "u1", "hits": 5000, "hitsMax": 5000, "notifyWhenAttacked": true,
        "store": {"energy": 120}, "storeCapacityResource": {"energy": 300},
        "spawning": {"name": "w200", "needTime": 9, "spawnTime": 103}
    }));
    let inspected = inspect(&spawn, &context);
    assert_eq!(inspected.title, "Spawn1");
    assert_eq!(
        field(&inspected.fields, "Energy"),
        &Value::Meter {
            value: 120,
            max: 300,
            tone: Tone::Energy
        }
    );
    assert_eq!(field(&inspected.fields, "Spawning"), &text("w200"));
    assert_eq!(
        field(&inspected.fields, "Progress"),
        &Value::Meter {
            value: 6,
            max: 9,
            tone: Tone::Progress
        }
    );
    let extension = object(json!({
        "_id": "e1", "type": "extension", "room": "W1N1", "x": 1, "y": 2, "user": "u1",
        "hits": 1000, "hitsMax": 1000, "notifyWhenAttacked": true,
        "store": {"energy": 50}, "storeCapacityResource": {"energy": 50}, "off": true
    }));
    let inspected = inspect(&extension, &context);
    assert_eq!(inspected.title, "Extension");
    assert_eq!(field(&inspected.fields, "State"), &text("Inactive"));
}

#[test]
fn controller_fields() {
    let users = users();
    let context = Context {
        game_time: Some(5000),
        me: Some("u1"),
        users: &users,
    };
    let controller = object(json!({
        "_id": "k", "type": "controller", "room": "W1N1", "x": 21, "y": 39, "user": "u1",
        "level": 3, "progress": 1500, "progressTotal": 45000, "downgradeTime": 25000,
        "safeMode": 5100, "safeModeAvailable": 2,
        "sign": {"user": "u2", "text": "hello", "time": 1, "datetime": 1}
    }));
    let inspected = inspect(&controller, &context);
    assert_eq!(inspected.title, "Controller · level 3");
    let fields = &inspected.fields;
    assert_eq!(field(fields, "Level"), &text("3"));
    assert_eq!(
        field(fields, "Progress"),
        &Value::Meter {
            value: 1500,
            max: 45000,
            tone: Tone::Progress
        }
    );
    assert_eq!(field(fields, "Downgrade"), &text("in 20\u{202f}000 ticks"));
    assert_eq!(
        field(fields, "Safe mode"),
        &text("Active, ends in 100 ticks")
    );
    assert_eq!(field(fields, "Safe modes available"), &text("2"));
    assert_eq!(field(fields, "Sign"), &text("“hello” — bob"));

    let reserved = object(json!({
        "_id": "k2", "type": "controller", "room": "W2N1", "x": 1, "y": 1, "level": 0,
        "reservation": {"user": "u2", "endTime": 5001}
    }));
    let fields = inspect(&reserved, &context).fields;
    assert_eq!(field(&fields, "Reserved by"), &text("bob, 1 tick left"));
    assert!(
        fields.iter().all(|field| field.label != "Safe mode"),
        "unowned: no safe mode"
    );
}

#[test]
fn sources_minerals_and_sites() {
    let users = users();
    let context = Context {
        game_time: Some(10),
        me: None,
        users: &users,
    };
    let source = object(json!({
        "_id": "src", "type": "source", "room": "W1N1", "x": 11, "y": 44, "energy": 1200,
        "energyCapacity": 3000, "ticksToRegeneration": 300, "nextRegenerationTime": 40
    }));
    let inspected = inspect(&source, &context);
    assert_eq!(inspected.owner, None);
    assert_eq!(
        field(&inspected.fields, "Energy"),
        &Value::Meter {
            value: 1200,
            max: 3000,
            tone: Tone::Energy
        }
    );
    assert_eq!(
        field(&inspected.fields, "Regenerates"),
        &text("in 30 ticks")
    );
    let mineral = object(json!({
        "_id": "m", "type": "mineral", "room": "W1N1", "x": 6, "y": 45, "density": 3,
        "mineralAmount": 35000.0, "mineralType": "O"
    }));
    let inspected = inspect(&mineral, &context);
    assert_eq!(inspected.title, "Mineral O");
    assert_eq!(field(&inspected.fields, "Amount"), &text("35\u{202f}000"));
    assert_eq!(field(&inspected.fields, "Density"), &text("3"));
    let site = object(json!({
        "_id": "cs", "type": "constructionSite", "room": "W1N1", "x": 2, "y": 2,
        "user": "u1", "structureType": "extension", "progress": 600, "progressTotal": 3000
    }));
    let inspected = inspect(&site, &context);
    assert_eq!(inspected.title, "Extension (site)");
    assert_eq!(inspected.kind, "Construction site");
    assert!(!inspected.own, "no signed-in user");
    assert_eq!(
        field(&inspected.fields, "Progress"),
        &Value::Meter {
            value: 600,
            max: 3000,
            tone: Tone::Progress
        }
    );
}

#[test]
fn stores_dropped_resources_and_tombstones() {
    let users = users();
    let context = Context {
        game_time: Some(100),
        me: Some("u1"),
        users: &users,
    };
    let storage = object(json!({
        "_id": "st", "type": "storage", "room": "W1N1", "x": 3, "y": 3, "user": "u1",
        "hits": 10000, "hitsMax": 10000, "notifyWhenAttacked": true,
        "store": {"energy": 1000, "UH2O": 3000, "K": 0}, "storeCapacity": 1_000_000
    }));
    assert_eq!(
        field(&inspect(&storage, &context).fields, "Store"),
        &Value::Store {
            resources: vec![("UH2O".to_owned(), 3000), ("energy".to_owned(), 1000)],
            used: 4000,
            capacity: Some(1_000_000)
        }
    );
    let dropped = object(json!({
        "_id": "r", "type": "energy", "resourceType": "energy", "room": "W1N1", "x": 4,
        "y": 4, "energy": 75
    }));
    let inspected = inspect(&dropped, &context);
    assert_eq!(inspected.title, "75 energy");
    assert_eq!(inspected.kind, "Dropped resource");
    assert_eq!(field(&inspected.fields, "Amount"), &text("75"));
    let tombstone = object(json!({
        "_id": "t", "type": "tombstone", "room": "W1N1", "x": 5, "y": 5, "user": "u2",
        "creepName": "old", "deathTime": 90, "decayTime": 150, "store": {"energy": 10}
    }));
    let inspected = inspect(&tombstone, &context);
    assert_eq!(inspected.title, "Tombstone of old");
    assert_eq!(field(&inspected.fields, "Decays"), &text("in 50 ticks"));
    assert_eq!(field(&inspected.fields, "Died"), &text("tick 90"));
}

#[test]
fn structures_with_cooldowns_and_decay() {
    let users = users();
    let context = Context {
        game_time: Some(100),
        me: Some("u1"),
        users: &users,
    };
    let link = object(json!({
        "_id": "l", "type": "link", "room": "W1N1", "x": 1, "y": 1, "user": "u1", "hits": 1000,
        "hitsMax": 1000, "notifyWhenAttacked": true, "cooldown": 3, "store": {"energy": 400},
        "storeCapacityResource": {"energy": 800}
    }));
    let fields = inspect(&link, &context).fields;
    assert_eq!(field(&fields, "Cooldown"), &text("3 ticks"));
    assert_eq!(
        field(&fields, "Energy"),
        &Value::Meter {
            value: 400,
            max: 800,
            tone: Tone::Energy
        }
    );
    let road = object(json!({
        "_id": "rd", "type": "road", "room": "W1N1", "x": 1, "y": 2, "hits": 2500,
        "hitsMax": 5000, "nextDecayTime": 1100, "notifyWhenAttacked": true
    }));
    let fields = inspect(&road, &context).fields;
    assert_eq!(field(&fields, "Decays"), &text("in 1\u{202f}000 ticks"));
    let rampart = object(json!({
        "_id": "rp", "type": "rampart", "room": "W1N1", "x": 1, "y": 3, "user": "u1",
        "hits": 1, "hitsMax": 300_000, "nextDecayTime": 99, "isPublic": true
    }));
    let fields = inspect(&rampart, &context).fields;
    assert_eq!(field(&fields, "Public"), &text("Yes"));
    assert!(
        fields.iter().all(|field| field.label != "Decays"),
        "past ticks are not shown"
    );
    let other =
        object(json!({"_id": "o", "type": "scoreContainer", "room": "W1N1", "x": 1, "y": 4}));
    let inspected = inspect(&other, &context);
    assert_eq!(inspected.kind, "Score container");
    assert_eq!(inspected.fields, []);
}

#[test]
fn portals_lead_to_their_destination_on_this_or_another_shard() {
    let users = users();
    let context = Context {
        game_time: Some(1000),
        me: None,
        users: &users,
    };
    let within = object(json!({
        "_id": "p1", "type": "portal", "room": "W5N5", "x": 20, "y": 21,
        "destination": {"room": "W0N0", "x": 25, "y": 26}, "decayTime": 1500
    }));
    let fields = inspect(&within, &context).fields;
    let Value::Destination { text, to } = field(&fields, "Leads to") else {
        panic!("no destination link in {fields:?}");
    };
    assert_eq!(text, "W0N0");
    assert_eq!(to.room.to_string(), "W0N0");
    assert_eq!(
        (to.shard.as_deref(), to.x, to.y),
        (None, Some(25), Some(26))
    );
    assert_eq!(
        field(&fields, "Decays"),
        &Value::Text("in 500 ticks".to_owned())
    );

    let across = object(json!({
        "_id": "p2", "type": "portal", "room": "W0N0", "x": 10, "y": 10,
        "destination": {"room": "E10S20", "shard": "shard2"}
    }));
    let fields = inspect(&across, &context).fields;
    let Value::Destination { text, to } = field(&fields, "Leads to") else {
        panic!("no destination link in {fields:?}");
    };
    assert_eq!(text, "E10S20 on shard2");
    assert_eq!((to.shard.as_deref(), to.x), (Some("shard2"), None));

    // A destination that does not parse stays plain text.
    let broken = object(json!({
        "_id": "p3", "type": "portal", "room": "W0N0", "x": 10, "y": 10,
        "destination": {"room": "nowhere"}
    }));
    let fields = inspect(&broken, &context).fields;
    assert_eq!(
        field(&fields, "Leads to"),
        &Value::Text("nowhere".to_owned())
    );
}

#[test]
fn labels_and_titles() {
    assert_eq!(kind_label("constructionSite"), "Construction site");
    assert_eq!(kind_label("powerSpawn"), "Power spawn");
    assert_eq!(kind_label("constructedWall"), "Wall");
    assert_eq!(kind_label("keeperLair"), "Keeper lair");
    let wall =
        object(json!({"_id": "w", "type": "constructedWall", "room": "W1N1", "x": 1, "y": 1}));
    assert_eq!(title(&wall), "Wall");
}

#[test]
fn resource_colours() {
    let swatch = |colour: &str| format!("flex-none size-[0.7rem] rounded-xs {colour}");
    assert_eq!(resource_class("energy"), swatch("bg-res-energy"));
    assert_eq!(resource_class("power"), swatch("bg-res-power"));
    assert_eq!(resource_class("ops"), swatch("bg-res-ops"));
    assert_eq!(resource_class("UH2O"), swatch("bg-res-u"));
    assert_eq!(resource_class("LO"), swatch("bg-res-l"));
    assert_eq!(resource_class("XKHO2"), swatch("bg-res-k"));
    assert_eq!(resource_class("ZK"), swatch("bg-res-z"));
    assert_eq!(resource_class("OH"), swatch("bg-res-ho"));
    assert_eq!(resource_class("X"), swatch("bg-res-x"));
    assert_eq!(resource_class("G"), swatch("bg-res-g"));
    assert_eq!(resource_class("metal"), swatch("bg-muted"));
}

#[test]
fn part_colours() {
    assert_eq!(
        PART,
        "size-[0.85rem] rounded-xs data-dead:opacity-25 \
         data-boosted:shadow-[0_0_0_2px_var(--color-sunken),0_0_0_3px_var(--color-fg)]"
    );
    let part = |colour: &str| format!("{PART} {colour}");
    assert_eq!(part_class("move"), part("bg-part-move"));
    assert_eq!(part_class("work"), part("bg-part-work"));
    assert_eq!(part_class("carry"), part("bg-part-carry"));
    assert_eq!(part_class("attack"), part("bg-part-attack"));
    assert_eq!(part_class("ranged_attack"), part("bg-part-ranged-attack"));
    assert_eq!(part_class("heal"), part("bg-part-heal"));
    assert_eq!(part_class("claim"), part("bg-part-claim"));
    assert_eq!(part_class("tough"), part("bg-part-tough"));
    assert_eq!(part_class("wing"), part("bg-muted"));
}
