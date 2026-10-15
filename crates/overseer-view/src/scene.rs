//! The drawn room of a detail scene, in room-local tile coordinates: raised walls, objects
//! with their motion and fades, action effects, bot visuals and the per-frame instance
//! lists (tokens sorted far to near, prisms grouped by silhouette, effects).

pub mod color;
pub mod connect;
pub mod effects;
pub mod motion;
pub mod style;
pub mod visuals;

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use glam::{Vec2, Vec3};
use overseer::RoomSnapshot;
use overseer::core::api::types::{GameRoomObject, GameRoomObjectActionLogValue};
use overseer::core::api::{Terrain, Tile};

use crate::mesh::{self, EffectInstance, Instance, Mesh, Shape, TerrainVertex, Token, rgba};
use crate::text::Atlas;
use connect::Grid;
use effects::{Anchor, Effect};
use motion::{ease, progress};
use style::{Gauges, Glyph, Join, Look, flag};
use visuals::Visuals;

/// How long new objects fade in and removed ones fade out, in milliseconds.
pub const FADE_MS: f64 = 350.0;
/// Height of effect ends above the ground under them.
const EFFECT_LIFT: f32 = 0.38;
/// `Token::kind` silhouette byte of objects without a prism.
const NO_SHAPE: u32 = 0xff;

/// An object of the room as drawn.
#[derive(Clone, Debug)]
struct Object {
    id: String,
    user: Option<String>,
    look: Look,
    gauges: Gauges,
    /// Links to neighbours of the same kind (`connect`).
    links: u8,
    /// The tile the object is on.
    tile: (u8, u8),
    /// Glide from `from` (tile coordinates) to `tile`, started at `start` over `duration`
    /// (ms).
    from: Vec2,
    start: f64,
    duration: f64,
    /// Fade-in start (ms; `-∞`: shown at once).
    born: f64,
    /// Fade-out start of a removed object (ms).
    dying: Option<f64>,
    /// Owner colour (sRGB, alpha 0 when unowned).
    accent: [u8; 4],
}

impl Object {
    /// Tile coordinates of the object at `now` (ms): eased from `from` to `tile`.
    fn position(&self, now: f64) -> Vec2 {
        let to = tile_vec(self.tile);
        let progress = progress(now, self.start, self.duration);
        if progress >= 1.0 {
            return to;
        }
        self.from.lerp(to, ease(progress))
    }

    /// Opacity and scale at `now`: fading in after birth, out after removal.
    fn fade(&self, now: f64) -> (f32, f32) {
        if let Some(dying) = self.dying {
            let left = 1.0 - progress(now, dying, FADE_MS);
            return (left, 0.7 + 0.3 * left);
        }
        let shown = ease(progress(now, self.born, FADE_MS));
        (shown, 0.6 + 0.4 * shown)
    }

    fn animating(&self, now: f64) -> bool {
        progress(now, self.start, self.duration) < 1.0
            || progress(now, self.born, FADE_MS) < 1.0
            || self.dying.is_some()
    }
}

fn tile_vec(tile: (u8, u8)) -> Vec2 {
    Vec2::new(f32::from(tile.0), f32::from(tile.1))
}

/// Timing of a snapshot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timing {
    /// Arrival (ms).
    pub now: f64,
    /// Estimated tick duration (ms).
    pub tick_ms: f64,
    /// How long moves glide (ms).
    pub glide_ms: f64,
}

impl Timing {
    /// The timing of a snapshot arriving at `now` before ticks are measured.
    #[cfg(test)]
    pub fn at(now: f64) -> Self {
        Self {
            now,
            tick_ms: motion::DEFAULT_TICK_MS,
            glide_ms: crate::MOVE_DURATION_MS,
        }
    }
}

/// Where the camera is, room-local, for drawing order and prisms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Eye {
    /// The eye position.
    pub position: Vec3,
    /// The unit view direction.
    pub forward: Vec3,
    /// Whether prisms and raised walls show (perspective).
    pub perspective: bool,
}

impl Default for Eye {
    fn default() -> Self {
        Self {
            position: Vec3::new(25.0, 100.0, 25.0),
            forward: Vec3::NEG_Y,
            perspective: false,
        }
    }
}

/// Feature switches of a scene ([`crate::ViewOptions`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Features {
    /// Objects glide between tiles (else they snap).
    pub motion: bool,
    /// Action effects.
    pub effects: bool,
    /// Bot room visuals.
    pub visuals: bool,
}

impl Default for Features {
    fn default() -> Self {
        Self {
            motion: true,
            effects: true,
            visuals: true,
        }
    }
}

/// The room as drawn.
#[derive(Debug, Default)]
pub struct Scene {
    /// `(shard, room)` of the objects shown.
    room: Option<(Option<String>, String)>,
    terrain: Option<Arc<Terrain>>,
    /// Bumped whenever the wall mesh is rebuilt.
    terrain_version: u64,
    terrain_mesh: Mesh<TerrainVertex>,
    /// Terrain height per tile, indexed by `y * 50 + x`.
    heights: Vec<f32>,
    user: Option<String>,
    features: Features,
    objects: Vec<Object>,
    /// Removed objects fading out.
    ghosts: Vec<Object>,
    effects: Vec<Effect>,
    visuals: Visuals,
    /// Tiles of roads, walls and ramparts, for their links.
    grids: [Grid; 3],
    /// The instances of the latest frame (reused: no allocation per frame).
    tokens: Vec<Token>,
    sides: Vec<Instance>,
    effect_instances: Vec<EffectInstance>,
}

/// What [`Scene::frame`] produced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    /// Prism instance range per [`Shape`] (in [`Shape::ALL`] order).
    pub sides: [Range<u32>; 8],
    /// Whether an object still moves or fades, or an effect plays.
    pub animating: bool,
}

impl Scene {
    /// Shows `snapshot` arriving at `timing.now`, laying out its text with `atlas`. Returns
    /// whether the wall mesh was rebuilt.
    pub fn set_room(&mut self, snapshot: &RoomSnapshot, timing: Timing, atlas: &mut Atlas) -> bool {
        let now = timing.now;
        let room = (snapshot.shard.clone(), snapshot.room.clone());
        let same_room = self.room.as_ref() == Some(&room);
        self.room = Some(room);
        let rebuilt = self.set_terrain(&snapshot.terrain);
        // Effects of earlier ticks still playing keep where their actor is drawn now.
        if same_room {
            for effect in &mut self.effects {
                if let Anchor::Object(index) = effect.from {
                    let at = self
                        .objects
                        .get(index)
                        .map_or(effect.to, |object| object.position(now));
                    effect.from = Anchor::Fixed(at);
                }
            }
            self.effects.retain(|effect| !effect.done(now));
            self.ghosts.retain(|ghost| ghost.fade(now).0 > 0.0);
        } else {
            self.effects.clear();
            self.ghosts.clear();
        }
        let previous: HashMap<&str, &Object> = if same_room {
            self.objects
                .iter()
                .map(|object| (object.id.as_str(), object))
                .collect()
        } else {
            HashMap::new()
        };
        for grid in &mut self.grids {
            grid.clear();
        }
        let mut objects = Vec::with_capacity(snapshot.objects.len());
        let mut says = Vec::new();
        for object in &snapshot.objects {
            let (Some(x), Some(y)) = (coordinate(object.x()), coordinate(object.y())) else {
                continue;
            };
            let tile = (x, y);
            let look = Look::of(object);
            let gauges = Gauges::of(object, snapshot.game_time);
            let (from, start, duration, born) = match previous.get(object.id()) {
                // Unmoved: keep any movement in progress; a creep leaving its spawn fades
                // in.
                Some(old) if old.tile == tile => {
                    let born = if old.gauges.hidden && !gauges.hidden {
                        now
                    } else {
                        old.born
                    };
                    (old.from, old.start, old.duration, born)
                }
                // Moved: from where it is drawn now.
                Some(old) if self.features.motion => {
                    (old.position(now), now, timing.glide_ms, old.born)
                }
                Some(old) => (tile_vec(tile), f64::NEG_INFINITY, 0.0, old.born),
                // New in a room already shown: fades in; the first snapshot snaps.
                None if same_room => (tile_vec(tile), f64::NEG_INFINITY, 0.0, now),
                None => (tile_vec(tile), f64::NEG_INFINITY, 0.0, f64::NEG_INFINITY),
            };
            let grid = match (look.glyph, look.join) {
                (_, Join::None) => None,
                (Glyph::Road, _) => Some(0),
                (Glyph::Wall, _) => Some(1),
                _ => Some(2),
            };
            if let Some(grid) = grid {
                self.grids[grid].insert(x, y);
            }
            if self.features.effects
                && same_room
                && let Some(log) = action_log(object)
            {
                effects::schedule(
                    log,
                    objects.len(),
                    tile_vec(tile),
                    now,
                    timing.tick_ms,
                    &mut self.effects,
                );
            }
            if let Some(log) = action_log(object)
                && let Some(Some(GameRoomObjectActionLogValue::Entry(entry))) = log.get("say")
                && let Some(message) = &entry.message
            {
                says.push((message.clone(), tile_vec(tile)));
            }
            objects.push(Object {
                id: object.id().to_owned(),
                user: object.user().map(str::to_owned),
                look,
                gauges,
                links: 0,
                tile,
                from,
                start,
                duration,
                born,
                dying: None,
                accent: [0; 4],
            });
        }
        // Removed objects fade out where they are drawn.
        if same_room {
            let kept: std::collections::HashSet<&str> =
                snapshot.objects.iter().map(GameRoomObject::id).collect();
            for old in &self.objects {
                if !kept.contains(old.id.as_str()) && !old.gauges.hidden {
                    // Keeps gliding where it went while it fades.
                    let mut ghost = old.clone();
                    ghost.dying = Some(now);
                    ghost.gauges.flags |= flag::GHOST;
                    self.ghosts.push(ghost);
                }
            }
        }
        for object in &mut objects {
            let grid = match object.look.glyph {
                Glyph::Road => 0,
                Glyph::Wall => 1,
                _ => 2,
            };
            object.links = self.grids[grid].links(object.tile.0, object.tile.1, object.look.join);
        }
        drop(previous);
        self.objects = objects;
        self.recolor();
        if self.features.visuals {
            self.visuals.build(&snapshot.visuals, atlas);
            for (message, tile) in &says {
                self.visuals.say(message, *tile, atlas);
            }
        } else {
            self.visuals.build(&[], atlas);
        }
        self.reserve();
        rebuilt
    }

    /// Replaces the terrain when it differs; returns whether the wall mesh was rebuilt.
    fn set_terrain(&mut self, terrain: &Arc<Terrain>) -> bool {
        if let Some(current) = &self.terrain
            && (Arc::ptr_eq(current, terrain) || current == terrain)
        {
            return false;
        }
        self.terrain_mesh = mesh::terrain(terrain);
        self.heights = (0..2500_u16)
            .map(|index| {
                let (x, y) = (index % 50, index / 50);
                let tile = u8::try_from(x)
                    .ok()
                    .zip(u8::try_from(y).ok())
                    .and_then(|(x, y)| terrain.get(x, y));
                mesh::tile_height(tile.unwrap_or(Tile::Plain))
            })
            .collect();
        self.terrain = Some(Arc::clone(terrain));
        self.terrain_version += 1;
        true
    }

    /// Sets the signed-in user (own objects are coloured differently).
    pub fn set_user(&mut self, user: Option<String>) {
        if self.user != user {
            self.user = user;
            self.recolor();
        }
    }

    /// Sets the feature switches (they apply from the next snapshot; effects and visuals
    /// switched off disappear at once).
    pub fn set_features(&mut self, features: Features) {
        self.features = features;
        if !features.effects {
            self.effects.clear();
        }
        if !features.visuals && !self.visuals.is_empty() {
            self.visuals.build(&[], &mut Atlas::default());
        }
    }

    /// Recomputes the owner colours.
    fn recolor(&mut self) {
        let me = self.user.as_deref();
        for object in self.objects.iter_mut().chain(&mut self.ghosts) {
            let user = object
                .gauges
                .accent_user
                .as_deref()
                .or(object.user.as_deref());
            object.accent = match object.look.owner(user, me).color() {
                Some(color) => rgba(color, 255),
                None => [0; 4],
            };
        }
    }

    /// Reserves the instances of a frame: drawing never allocates.
    fn reserve(&mut self) {
        let count = self.objects.len() + self.ghosts.len();
        self.tokens.reserve(count.saturating_sub(self.tokens.len()));
        self.sides.reserve(count.saturating_sub(self.sides.len()));
        self.effect_instances.reserve(
            self.effects
                .len()
                .saturating_sub(self.effect_instances.len()),
        );
    }

    /// Fills the instances of the frame at `now` (ms) seen from `eye`.
    pub fn frame(&mut self, now: f64, eye: &Eye) -> Frame {
        self.tokens.clear();
        self.sides.clear();
        self.effect_instances.clear();
        self.ghosts.retain(|ghost| ghost.fade(now).0 > 0.0);
        self.effects.retain(|effect| !effect.done(now));
        let mut animating = !self.effects.is_empty() || !self.ghosts.is_empty();
        for object in self.objects.iter().chain(&self.ghosts) {
            animating |= object.animating(now);
            if object.gauges.hidden {
                continue;
            }
            let (opacity, scale) = object.fade(now);
            if opacity <= 0.0 {
                continue;
            }
            let position = object.position(now);
            let base = self.height_at(position);
            self.tokens
                .push(token(object, position, base, opacity, scale));
        }
        // Far to near: the blended tops overlap correctly.
        let depth = |token: &Token| (Vec3::from(token.position) - eye.position).dot(eye.forward);
        self.tokens
            .sort_unstable_by(|a, b| depth(b).total_cmp(&depth(a)));
        let mut sides: [Range<u32>; 8] = Default::default();
        if eye.perspective {
            for (shape, range) in Shape::ALL.into_iter().zip(&mut sides) {
                let start = count(self.sides.len());
                for object in self.objects.iter().chain(&self.ghosts) {
                    if object.look.shape != Some(shape) || object.gauges.hidden {
                        continue;
                    }
                    let (opacity, scale) = object.fade(now);
                    if opacity <= 0.0 {
                        continue;
                    }
                    let position = object.position(now);
                    let base = self.height_at(position);
                    let size = object.gauges.size.unwrap_or(object.look.size) * scale;
                    let mut color = mesh::linear(object.look.side);
                    color[3] = opacity;
                    self.sides.push(Instance {
                        offset: [position.x + 0.5, base, position.y + 0.5],
                        scale: [size, object.look.height * scale, size],
                        color,
                    });
                }
                *range = start..count(self.sides.len());
            }
        }
        for effect in &self.effects {
            if now < effect.start {
                continue;
            }
            let from = match effect.from {
                Anchor::Object(index) => self
                    .objects
                    .get(index)
                    .map_or(effect.to, |object| object.position(now)),
                Anchor::Fixed(at) => at,
            };
            let (from_base, to_base) = (self.height_at(from), self.height_at(effect.to));
            let color = rgba(effect.color, 255);
            self.effect_instances.push(EffectInstance {
                from: effects::point(from, from_base, EFFECT_LIFT).to_array(),
                progress: effect.progress(now),
                to: effects::point(effect.to, to_base, EFFECT_LIFT).to_array(),
                width: effect.width,
                color,
                look: effect.look as u32,
            });
        }
        Frame { sides, animating }
    }

    /// Terrain height under a position in tile coordinates (of its nearest tile).
    pub fn height_at(&self, position: Vec2) -> f32 {
        let tile = position.round();
        if !(0.0..50.0).contains(&tile.x) || !(0.0..50.0).contains(&tile.y) {
            return 0.0;
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "whole numbers in 0..50"
        )]
        let index = tile.y as usize * 50 + tile.x as usize;
        self.heights.get(index).copied().unwrap_or(0.0)
    }

    /// Terrain height of a tile.
    pub fn tile_height(&self, x: u8, y: u8) -> f32 {
        self.heights
            .get(usize::from(y) * 50 + usize::from(x))
            .copied()
            .unwrap_or(0.0)
    }

    /// Ids of the objects on a tile, topmost first (higher layer first, then by id).
    pub fn objects_at(&self, tile: (u8, u8)) -> Vec<String> {
        let mut on_tile: Vec<&Object> = self
            .objects
            .iter()
            .filter(|object| object.tile == tile)
            .collect();
        on_tile.sort_by(|a, b| b.look.layer.cmp(&a.look.layer).then(a.id.cmp(&b.id)));
        on_tile
            .into_iter()
            .map(|object| object.id.clone())
            .collect()
    }

    /// The tokens of the latest [`Self::frame`], far to near.
    pub fn tokens(&self) -> &[Token] {
        &self.tokens
    }

    /// The prisms of the latest [`Self::frame`], grouped by shape.
    pub fn sides(&self) -> &[Instance] {
        &self.sides
    }

    /// The effects of the latest [`Self::frame`].
    pub fn effects(&self) -> &[EffectInstance] {
        &self.effect_instances
    }

    /// The bot visuals of the latest snapshot.
    pub const fn visuals(&self) -> &Visuals {
        &self.visuals
    }

    /// Instances a frame may need: tokens and prisms, effects.
    pub fn capacity(&self) -> (usize, usize) {
        (
            self.objects.len() + self.ghosts.len(),
            self.effects.len().max(self.effect_instances.len()),
        )
    }

    /// Number of objects shown.
    pub const fn object_count(&self) -> usize {
        self.objects.len()
    }

    /// The wall mesh and its version (bumped on every rebuild; 0 before any terrain).
    pub const fn terrain(&self) -> (&Mesh<TerrainVertex>, u64) {
        (&self.terrain_mesh, self.terrain_version)
    }
}

/// The token of `object` drawn at `position` on ground `base`.
fn token(object: &Object, position: Vec2, base: f32, opacity: f32, scale: f32) -> Token {
    let gauges = &object.gauges;
    let shape = object.look.shape.map_or(NO_SHAPE, Shape::index);
    let kind = u32::from(object.look.glyph as u8)
        | u32::from(object.links) << 8
        | shape << 16
        | u32::from(gauges.flags) << 24;
    let fill = gauges
        .store
        .map_or([0; 4], |(fill, color)| rgba(color, gauge(fill)));
    let mut parts_a = [0; 4];
    let mut parts_b = [0; 4];
    parts_a.copy_from_slice(&gauges.parts[..4]);
    parts_b.copy_from_slice(&gauges.parts[4..]);
    let height = object.look.height * scale;
    Token {
        position: [position.x + 0.5, base + height, position.y + 0.5],
        size: gauges.size.unwrap_or(object.look.size) * scale,
        kind,
        accent: object.accent,
        fill,
        gauges: [
            gauges.hits.map_or(0, gauge),
            gauges.progress.map_or(0, gauge),
            gauges.level,
            0,
        ],
        parts_a,
        parts_b,
        fade: opacity,
        base,
    }
}

/// A gauge in `0..=1` as a byte: `1..=255` (0 means no gauge).
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "1..=255 after the clamp"
)]
fn gauge(value: f32) -> u8 {
    (1.0 + value.clamp(0.0, 1.0) * 254.0).round() as u8
}

/// The action log of an object, if its kind has one.
fn action_log(
    object: &GameRoomObject,
) -> Option<&overseer::core::api::types::GameRoomObjectActionLog> {
    use GameRoomObject as O;
    match object {
        O::Creep(creep) => creep.action_log.as_ref(),
        O::PowerCreep(creep) => Some(&creep.action_log),
        O::Tower(tower) => tower.action_log.as_ref(),
        O::Link(link) => link.action_log.as_ref(),
        O::Lab(lab) => lab.action_log.as_ref(),
        O::Factory(factory) => factory.action_log.as_ref(),
        O::InvaderCore(core) => core.action_log.as_ref(),
        _ => None,
    }
}

/// A room coordinate in `0..50`.
fn coordinate(value: u64) -> Option<u8> {
    u8::try_from(value).ok().filter(|value| *value < 50)
}

/// An instance index as `u32` (instance lists stay far below `u32::MAX`).
fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
impl Scene {
    /// Where an object (or a ghost) is drawn at `now`: position, ground height, owner
    /// colour and opacity.
    pub fn drawn(&self, id: &str, now: f64) -> Option<(Vec2, f32, [u8; 4], f32)> {
        let object = self
            .objects
            .iter()
            .chain(&self.ghosts)
            .find(|object| object.id == id)?;
        let position = object.position(now);
        Some((
            position,
            self.height_at(position),
            object.accent,
            object.fade(now).0,
        ))
    }

    /// Ghosts fading out.
    pub fn ghost_count(&self) -> usize {
        self.ghosts.len()
    }

    /// Scheduled effects.
    pub fn scheduled(&self) -> &[Effect] {
        &self.effects
    }
}
