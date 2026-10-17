//! Action effects: what an object's action log of the last tick shows (beams, bolts,
//! bursts and hit flashes), scheduled from the snapshot's arrival over a share of the tick.
//!
//! | action | effect |
//! |---|---|
//! | `attack` | short red beam and a burst on the target |
//! | `rangedAttack` | blue bolt flying to the target |
//! | `rangedMassAttack` | blue ring spreading from the actor (range 3) |
//! | `heal` | short green beam and a flash on the target |
//! | `rangedHeal` | green bolt |
//! | `harvest` | amber beam flowing from the source to the actor |
//! | `build` | pale amber beam flowing to the site |
//! | `repair` | teal beam |
//! | `upgradeController` | amber beam flowing to the controller |
//! | `reserveController` | violet beam |
//! | `attackController` | magenta beam and a burst |
//! | `transferEnergy` (links) | amber bolt to the receiving link |
//! | `runReaction`, `reverseReaction` (labs) | pale blue beams between the labs |
//! | `power` (power creeps) | violet burst on the target |
//! | `attacked`, `healed` | red or green flash on the object itself |
//!
//! Beams last [`BEAM_SHARE`] of a tick, bolts [`BOLT_SHARE`], within fixed bounds, so that
//! effects end before the next tick's begin.

use glam::{Vec2, Vec3};
use overseer::model::world::{
    GameRoomObjectActionLog, GameRoomObjectActionLogEntry, GameRoomObjectActionLogValue,
};

/// How an effect is drawn (`EffectInstance::look`, `effects.wgsl`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Look {
    /// A beam with dashes flowing from `from` to `to`.
    Beam = 0,
    /// A bolt flying from `from` to `to`.
    Bolt = 1,
    /// A burst at `to`.
    Burst = 2,
    /// A flash ring at `to`.
    Flash = 3,
    /// A ring spreading from `from` to radius `width`.
    Pulse = 4,
}

/// Share of a tick a beam lasts.
pub const BEAM_SHARE: f64 = 0.6;
/// Share of a tick a bolt flies.
pub const BOLT_SHARE: f64 = 0.4;
/// Bounds of a beam's duration (ms).
const BEAM_MS: (f64, f64) = (300.0, 1100.0);
/// Bounds of a bolt's flight (ms).
const BOLT_MS: (f64, f64) = (220.0, 650.0);
/// Duration of bursts and flashes (ms).
const FLASH_MS: f64 = 380.0;
/// Duration of a mass attack's ring (ms).
const PULSE_MS: f64 = 650.0;

/// Where an effect starts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    /// The object at this index of the scene (it may move meanwhile).
    Object(usize),
    /// A fixed room-local tile position (an object that is gone or replaced).
    Fixed(Vec2),
}

/// A scheduled effect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Effect {
    /// How it is drawn.
    pub look: Look,
    /// Colour (sRGB).
    pub color: u32,
    /// Start.
    pub from: Anchor,
    /// End: a room-local tile (whole tiles; drawn at the tile's centre).
    pub to: Vec2,
    /// When it starts (ms).
    pub start: f64,
    /// How long it lasts (ms).
    pub duration: f64,
    /// Width of a beam or bolt, radius of a burst, flash or pulse (tiles).
    pub width: f32,
}

impl Effect {
    /// Whether the effect is over at `now`.
    pub fn done(&self, now: f64) -> bool {
        now >= self.start + self.duration
    }

    /// The effect's progress at `now`, `0..=1`.
    pub fn progress(&self, now: f64) -> f32 {
        super::motion::progress(now, self.start, self.duration)
    }
}

/// Colours (sRGB).
const RED: u32 = 0xff_4a_4a;
const BLUE: u32 = 0x5c_9d_ff;
const GREEN: u32 = 0x5f_e0_7a;
const AMBER: u32 = 0xff_c8_3a;
const PALE_AMBER: u32 = 0xff_e0_8a;
const TEAL: u32 = 0x5f_e0_c8;
const VIOLET: u32 = 0xb3_88_ff;
const MAGENTA: u32 = 0xe0_5c_d8;
const PALE_BLUE: u32 = 0x9c_d8_ff;

/// The target tile of an entry.
fn target(entry: &GameRoomObjectActionLogEntry) -> Option<Vec2> {
    let coordinate = |value: Option<u64>| {
        value
            .and_then(|value| u8::try_from(value).ok())
            .filter(|value| *value < 50)
            .map(f32::from)
    };
    Some(Vec2::new(coordinate(entry.x)?, coordinate(entry.y)?))
}

/// The effects of `log`, the action log of the object at index `actor` standing on `tile`,
/// for a snapshot that arrived at `now` with ticks of `tick_ms`; appended to `out` in
/// action order.
pub fn schedule(
    log: &GameRoomObjectActionLog,
    actor: usize,
    tile: Vec2,
    now: f64,
    tick_ms: f64,
    out: &mut Vec<Effect>,
) {
    let beam_ms = (tick_ms * BEAM_SHARE).clamp(BEAM_MS.0, BEAM_MS.1);
    let bolt_ms = (tick_ms * BOLT_SHARE).clamp(BOLT_MS.0, BOLT_MS.1);
    let from = Anchor::Object(actor);
    for (action, value) in log {
        let Some(value) = value else {
            continue;
        };
        let entry = match value {
            GameRoomObjectActionLogValue::Entry(entry) => entry,
            GameRoomObjectActionLogValue::Bool(_) => continue,
        };
        let mut push = |look, color, to: Vec2, duration, width| {
            out.push(Effect {
                look,
                color,
                from,
                to,
                start: now,
                duration,
                width,
            });
        };
        let to = target(entry);
        match (action.as_str(), to) {
            ("attack", Some(to)) => {
                push(Look::Beam, RED, to, (beam_ms * 0.6).max(BOLT_MS.0), 0.16);
                push(Look::Burst, RED, to, FLASH_MS, 0.45);
            }
            ("rangedAttack", Some(to)) => push(Look::Bolt, BLUE, to, bolt_ms, 0.14),
            ("rangedMassAttack", _) => push(Look::Pulse, BLUE, tile, PULSE_MS, 3.5),
            ("heal", Some(to)) => {
                push(Look::Beam, GREEN, to, (beam_ms * 0.6).max(BOLT_MS.0), 0.12);
                push(Look::Flash, GREEN, to, FLASH_MS, 0.5);
            }
            ("rangedHeal", Some(to)) => push(Look::Bolt, GREEN, to, bolt_ms, 0.12),
            ("harvest", Some(to)) => out.push(Effect {
                look: Look::Beam,
                color: AMBER,
                // Energy flows from the source to the harvester.
                from: Anchor::Fixed(to),
                to: tile,
                start: now,
                duration: beam_ms,
                width: 0.1,
            }),
            ("build", Some(to)) => push(Look::Beam, PALE_AMBER, to, beam_ms, 0.1),
            ("repair", Some(to)) => push(Look::Beam, TEAL, to, beam_ms, 0.1),
            ("upgradeController", Some(to)) => push(Look::Beam, AMBER, to, beam_ms, 0.1),
            ("reserveController", Some(to)) => push(Look::Beam, VIOLET, to, beam_ms, 0.12),
            ("attackController", Some(to)) => {
                push(Look::Beam, MAGENTA, to, beam_ms, 0.14);
                push(Look::Burst, MAGENTA, to, FLASH_MS, 0.6);
            }
            ("transferEnergy", Some(to)) => push(Look::Bolt, AMBER, to, bolt_ms, 0.16),
            ("runReaction" | "reverseReaction", _) => {
                let sources = [(entry.x1, entry.y1), (entry.x2, entry.y2)];
                for (x, y) in sources {
                    let source = GameRoomObjectActionLogEntry {
                        x,
                        y,
                        ..GameRoomObjectActionLogEntry::default()
                    };
                    if let Some(source) = target(&source) {
                        let (from, to) = if action == "runReaction" {
                            (source, tile)
                        } else {
                            (tile, source)
                        };
                        out.push(Effect {
                            look: Look::Beam,
                            color: PALE_BLUE,
                            from: Anchor::Fixed(from),
                            to,
                            start: now,
                            duration: beam_ms,
                            width: 0.09,
                        });
                    }
                }
            }
            ("power", Some(to)) => push(Look::Burst, VIOLET, to, FLASH_MS * 1.5, 0.7),
            ("attacked", _) => push(Look::Flash, RED, tile, FLASH_MS, 0.48),
            ("healed", _) => push(Look::Flash, GREEN, tile, FLASH_MS, 0.48),
            _ => {}
        }
    }
}

/// The room-local 3D point of an effect end on tile `tile` whose ground is at `base`.
pub fn point(tile: Vec2, base: f32, lift: f32) -> Vec3 {
    Vec3::new(tile.x + 0.5, base + lift, tile.y + 0.5)
}
