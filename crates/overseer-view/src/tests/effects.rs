//! Action effects: scheduling from action logs, their timing within the tick, and the
//! effect instances of frames.

use glam::Vec2;
use overseer::core::api::types::GameRoomObjectActionLog;
use serde_json::json;

use super::{ME, object, show, snapshot, terrain};
use crate::scene::effects::{Anchor, Effect, Look, schedule};
use crate::scene::{Eye, Features, Scene};

fn log(value: serde_json::Value) -> GameRoomObjectActionLog {
    serde_json::from_value(value).unwrap()
}

fn scheduled(value: serde_json::Value, tick_ms: f64) -> Vec<Effect> {
    let mut effects = Vec::new();
    schedule(
        &log(value),
        3,
        Vec2::new(10.0, 10.0),
        500.0,
        tick_ms,
        &mut effects,
    );
    effects
}

#[test]
fn action_logs_become_effects_from_the_snapshot_on() {
    let effects = scheduled(
        json!({
            "harvest": {"x": 11, "y": 10},
            "upgradeController": {"x": 12, "y": 14},
            "attacked": {"x": 8, "y": 8},
            "heal": null,
            "unknownAction": {"x": 1, "y": 1},
        }),
        3000.0,
    );
    // Action logs are ordered maps: attacked, harvest, upgradeController.
    assert_eq!(effects.len(), 3, "{effects:?}");
    let [hit, harvest, upgrade] = [effects[0], effects[1], effects[2]];
    assert_eq!(
        (hit.look, hit.to),
        (Look::Flash, Vec2::new(10.0, 10.0)),
        "on the actor"
    );
    assert_eq!(harvest.look, Look::Beam);
    assert_eq!(
        (harvest.from, harvest.to),
        (Anchor::Fixed(Vec2::new(11.0, 10.0)), Vec2::new(10.0, 10.0)),
        "energy flows from the source to the harvester"
    );
    assert_eq!(
        upgrade.from,
        Anchor::Object(3),
        "follows the (moving) actor"
    );
    assert_eq!(upgrade.to, Vec2::new(12.0, 14.0));
    for effect in &effects {
        assert_eq!(effect.start, 500.0, "starts on arrival");
        assert!(!effect.done(500.0));
        assert!(effect.done(effect.start + effect.duration));
    }
    assert_eq!(upgrade.progress(500.0 + upgrade.duration / 2.0), 0.5);
}

#[test]
fn combat_has_bolts_bursts_and_pulses() {
    let effects = scheduled(
        json!({
            "attack": {"x": 11, "y": 11},
            "rangedAttack": {"x": 13, "y": 10},
            "rangedMassAttack": {},
            "rangedHeal": {"x": 9, "y": 9},
        }),
        1000.0,
    );
    let looks: Vec<Look> = effects.iter().map(|effect| effect.look).collect();
    // In action order: attack, rangedAttack, rangedHeal, rangedMassAttack.
    assert_eq!(
        looks,
        [Look::Beam, Look::Burst, Look::Bolt, Look::Bolt, Look::Pulse],
        "{effects:?}"
    );
    assert_ne!(
        effects[2].color, effects[3].color,
        "attacks and heals differ"
    );
    let reactions = scheduled(
        json!({"runReaction": {"x1": 5, "y1": 5, "x2": 6, "y2": 4}}),
        1000.0,
    );
    assert_eq!(reactions.len(), 2, "a beam from each source lab");
    assert_eq!(reactions[0].to, Vec2::new(10.0, 10.0));
}

#[test]
fn effects_end_before_the_next_tick() {
    for tick in [500.0, 1000.0, 2000.0, 3000.0, 4000.0, 6000.0] {
        let effects = scheduled(
            json!({
                "build": {"x": 12, "y": 10}, "rangedAttack": {"x": 13, "y": 10},
                "attack": {"x": 11, "y": 10}, "attacked": {}, "rangedMassAttack": {},
            }),
            tick,
        );
        for effect in effects {
            assert!(
                effect.duration <= tick * 0.8 || effect.duration <= 700.0,
                "{tick} ms ticks: {effect:?}"
            );
            assert!(effect.duration >= 200.0, "visible: {effect:?}");
        }
    }
}

fn worker(x: u8, actions: &serde_json::Value) -> overseer::core::api::types::GameRoomObject {
    object(json!({
        "_id": "w", "type": "creep", "room": "W1N1", "x": x, "y": 10, "user": ME,
        "name": "w", "hits": 100, "hitsMax": 100, "store": {},
        "body": [{"type": "work", "hits": 100}], "actionLog": actions.clone(),
    }))
}

#[test]
fn scenes_play_effects_of_later_snapshots_only() {
    let terrain = terrain();
    let mut scene = Scene::default();
    let build = json!({"build": {"x": 20, "y": 20}});
    show(
        &mut scene,
        &snapshot(&terrain, vec![worker(10, &build)]),
        0.0,
    );
    assert!(
        scene.scheduled().is_empty(),
        "the first snapshot's log is old news"
    );
    show(
        &mut scene,
        &snapshot(&terrain, vec![worker(11, &build)]),
        1000.0,
    );
    assert_eq!(scene.scheduled().len(), 1);
    let frame = scene.frame(1000.0 + 100.0, &Eye::default());
    assert!(frame.animating);
    let effect = scene.effects()[0];
    assert_eq!(effect.to, [20.5, 0.38, 20.5]);
    assert!(
        effect.from[0] > 10.5 && effect.from[0] < 11.5,
        "from the gliding creep: {effect:?}"
    );
    assert!(effect.progress > 0.0 && effect.progress < 1.0);
    // Over: dropped, nothing animates.
    let frame = scene.frame(10_000.0, &Eye::default());
    assert!(scene.effects().is_empty(), "over");
    assert!(!frame.animating);
    // Switched off: none.
    scene.set_features(Features {
        effects: false,
        ..Features::default()
    });
    show(
        &mut scene,
        &snapshot(
            &terrain,
            vec![worker(11, &json!({"build": {"x": 20, "y": 20}}))],
        ),
        11_000.0,
    );
    assert!(scene.scheduled().is_empty(), "switched off");
}
