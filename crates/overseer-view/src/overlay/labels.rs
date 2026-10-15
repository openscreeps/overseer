//! Room coordinates as labels on the map, in CSS pixels at ground points (they keep their
//! size at every zoom), drawn from the glyph atlas with a dark halo so that they read over
//! any terrain.
//!
//! - **Sector labels** name the highway crossroads (`W10N20`, both numbers multiples of
//!   ten: the corners of the sectors), centred just above the crossroad's centre, while
//!   zoomed out ([`Lod::Overview`](crate::Lod::Overview)).
//! - **Shard labels** name the shards a room's portals lead to, under its portal mark.
//! - **Room labels** name each room in its north-west corner from
//!   [`Lod::Terrain`](crate::Lod::Terrain).
//!
//! Labels never overlap: each has the room size (CSS pixels) from which it shows
//! ([`Label::appear`]). Sector and shard labels are placed greedily, most important first
//! (shard names, then crossroads on coarser grids: `E0S0`, then every 160, 80, 40, 20
//! rooms…): a label shows from the zoom at which it clears every label placed before it.
//! Room labels form a grid, so the same rule is a tier: a room on every `2^t`-th row and
//! column (`t` ≤ [`ROOM_TIERS`]) shows once `2^t` rooms are wider than its label, so
//! zoomed out every second or fourth room is named, and every room once they fit.

use glam::Vec2;
use overseer::map::{RoomCoord, RoomRect};

use super::portals::{MARK_MIN_PX, Portals, SHARD_PORTAL};
use crate::camera::{room_centre, room_origin};
use crate::mesh::{LabelGlyph, rgba};
use crate::text::{Atlas, FONT_PX, SPREAD};

/// Label classes (`labels` in `overlay.wgsl` fades each).
pub mod class {
    /// Highway crossroads.
    pub const SECTOR: u32 = 0;
    /// Room names.
    pub const ROOM: u32 = 1;
    /// Shards portals lead to.
    pub const SHARD: u32 = 2;
}

/// Least room between two labels shown together, in CSS pixels.
pub const GAP: f32 = 6.0;
/// Height of capitals above the baseline, in ems.
const CAP: f32 = 0.74;
/// Depth below the baseline, in ems.
const DESCENT: f32 = 0.22;
/// Font sizes, CSS pixels per em.
const SECTOR_PX: f32 = 11.0;
const ROOM_PX: f32 = 10.0;
const SHARD_PX: f32 = 10.0;
/// Inset of a room label from its room's north-west corner, in CSS pixels.
pub const ROOM_INSET: f32 = 3.0;
/// Coarsest grid of room labels: every `2^ROOM_TIERS`-th room.
pub const ROOM_TIERS: u32 = 3;
/// Width of the halo outside the glyphs, in CSS pixels.
const HALO_PX: f32 = 2.4;
/// Colours (sRGB) and opacities.
const SECTOR_COLOR: [u8; 4] = rgba(0xe8_ec_f2, 235);
const ROOM_COLOR: [u8; 4] = rgba(0xd2_d7_df, 220);
const SHARD_COLOR: [u8; 4] = rgba(SHARD_PORTAL, 255);
const HALO: [u8; 4] = rgba(0x05_06_08, 210);

/// A label: one line of text at a ground point.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    /// The text.
    pub text: String,
    /// The ground point (world X, Z) it is placed from.
    pub anchor: Vec2,
    /// Font size, CSS pixels per em.
    pub size: f32,
    /// Where the line starts and its baseline, CSS pixels from the anchor (y down).
    pub pen: Vec2,
    /// Width of the line, CSS pixels.
    pub width: f32,
    /// One of [`class`].
    pub class: u32,
    /// Room size (CSS pixels) from which it shows.
    pub appear: f32,
}

impl Label {
    fn new(text: String, anchor: Vec2, size: f32, class: u32, measure: &mut Atlas) -> Self {
        let width = measure.measure(&text) * size;
        Self {
            text,
            anchor,
            size,
            pen: Vec2::ZERO,
            width,
            class,
            appear: 0.0,
        }
    }

    /// Centres the line horizontally with its baseline at `baseline`.
    fn centred(mut self, baseline: f32) -> Self {
        self.pen = Vec2::new(-self.width / 2.0, baseline);
        self
    }

    /// The box the line takes, CSS pixels from the anchor: left, top, right, bottom.
    pub fn bounds(&self) -> [f32; 4] {
        [
            self.pen.x,
            self.pen.y - CAP * self.size,
            self.pen.x + self.width,
            self.pen.y + DESCENT * self.size,
        ]
    }

    /// The room size (CSS pixels) from which this label and `other` (both shown) are
    /// [`GAP`] apart; infinite when they never are (same place).
    pub fn clearance(&self, other: &Self) -> f32 {
        // Anchors apart in rooms: at `p` pixels per room they are `delta × p` pixels apart.
        let delta = (other.anchor - self.anchor) / 50.0;
        let (a, b) = (self.bounds(), other.bounds());
        let axis = |delta: f32, (a_low, a_high): (f32, f32), (b_low, b_high): (f32, f32)| {
            if delta > 0.0 {
                ((a_high - b_low + GAP) / delta).max(0.0)
            } else if delta < 0.0 {
                ((b_high - a_low + GAP) / -delta).max(0.0)
            } else if a_high + GAP <= b_low || b_high + GAP <= a_low {
                0.0
            } else {
                f32::INFINITY
            }
        };
        let x = axis(delta.x, (a[0], a[2]), (b[0], b[2]));
        let y = axis(delta.y, (a[1], a[3]), (b[1], b[3]));
        x.min(y)
    }
}

/// The number of a room coordinate as in its name (`W0` and `E0` are both 0).
const fn number(coordinate: i32) -> i32 {
    if coordinate >= 0 {
        coordinate
    } else {
        -coordinate - 1
    }
}

/// Trailing zero bits of `value`, at most `cap` (`cap` for 0).
fn zeros(value: i32, cap: u32) -> u32 {
    if value == 0 {
        cap
    } else {
        value.trailing_zeros().min(cap)
    }
}

/// Whether a room is a highway crossroad (both numbers multiples of ten): a sector label.
pub const fn is_crossroad(room: RoomCoord) -> bool {
    number(room.x) % 10 == 0 && number(room.y) % 10 == 0
}

/// The sector labels (highway crossroads) and shard labels (portals to other shards) of
/// `bounds`, placed (see the module documentation); rooms `absent` from the world get
/// none.
pub fn overview(
    bounds: RoomRect,
    absent: impl Fn(RoomCoord) -> bool,
    portals: &Portals,
    atlas: &mut Atlas,
) -> Vec<Label> {
    let mut labels: Vec<Label> = portals
        .shard_names()
        .filter(|(room, ..)| bounds.contains(*room))
        .map(|(_, edge, names)| {
            Label::new(names, edge, SHARD_PX, class::SHARD, atlas).centred(3.0 + CAP * SHARD_PX)
        })
        .collect();
    let crossroads = |min: i32, max: i32| (min..=max).filter(|value| number(*value) % 10 == 0);
    let mut sectors: Vec<(u32, i32, RoomCoord)> = crossroads(bounds.min.y, bounds.max.y)
        .flat_map(|y| crossroads(bounds.min.x, bounds.max.x).map(move |x| RoomCoord { x, y }))
        .filter(|room| !absent(*room))
        .map(|room| {
            let (x, y) = (number(room.x) / 10, number(room.y) / 10);
            (zeros(x, 8).min(zeros(y, 8)), x + y, room)
        })
        .collect();
    // Coarser grids first, then nearer the centre lines, then south-east (`E0S0` before
    // its neighbours across the double highways).
    sectors.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(b.2.cmp(&a.2)));
    let baseline = -(MARK_MIN_PX + 3.0);
    labels.extend(sectors.into_iter().map(|(_, _, room)| {
        Label::new(
            room.to_string(),
            room_centre(room),
            SECTOR_PX,
            class::SECTOR,
            atlas,
        )
        .centred(baseline)
    }));
    for index in 0..labels.len() {
        let (placed, rest) = labels.split_at_mut(index);
        let label = &mut rest[0];
        label.appear = placed
            .iter()
            .map(|other| other.clearance(label))
            .fold(0.0, f32::max);
    }
    labels
}

/// The coarsest grid of room labels that may show at `px_per_room` (with some zoom to
/// spare) when labels are `nominal` CSS pixels wide, coarser still while `rooms` would get
/// more than `limit` labels.
pub fn room_tier(px_per_room: f32, nominal: f32, rooms: usize, limit: usize) -> u32 {
    let mut tier = (0..ROOM_TIERS)
        .find(|tier| (nominal + GAP) / f32::from(1_u16 << tier) <= px_per_room * 1.5)
        .unwrap_or(ROOM_TIERS);
    while tier < ROOM_TIERS && rooms >> (2 * tier) > limit {
        tier += 1;
    }
    tier
}

/// The width (CSS pixels) of a typical room label.
pub fn nominal(atlas: &mut Atlas) -> f32 {
    atlas.measure("W00N00") * ROOM_PX
}

/// The room labels of `rect` on grids of every `2^tier`-th room or coarser; rooms
/// `absent` from the world get none.
pub fn rooms(
    rect: RoomRect,
    tier: u32,
    absent: impl Fn(RoomCoord) -> bool,
    atlas: &mut Atlas,
) -> Vec<Label> {
    let step = 1_i32 << tier.min(ROOM_TIERS);
    let first = |min: i32| min + (-min).rem_euclid(step);
    let mut labels = Vec::new();
    for y in (first(rect.min.y)..=rect.max.y).step_by(step.unsigned_abs() as usize) {
        for x in (first(rect.min.x)..=rect.max.x).step_by(step.unsigned_abs() as usize) {
            let room = RoomCoord { x, y };
            if absent(room) {
                continue;
            }
            let grid = zeros(x, ROOM_TIERS).min(zeros(y, ROOM_TIERS));
            let mut label = Label::new(
                room.to_string(),
                room_origin(room),
                ROOM_PX,
                class::ROOM,
                atlas,
            );
            label.pen = Vec2::new(ROOM_INSET, ROOM_INSET + CAP * ROOM_PX);
            label.appear = (label.width + ROOM_INSET + GAP) / f32::from(1_u16 << grid);
            labels.push(label);
        }
    }
    labels
}

/// Appends the glyphs of `labels`.
pub fn glyphs(labels: &[Label], atlas: &mut Atlas, out: &mut Vec<LabelGlyph>) {
    for label in labels {
        let color = match label.class {
            class::SECTOR => SECTOR_COLOR,
            class::ROOM => ROOM_COLOR,
            _ => SHARD_COLOR,
        };
        // The halo in distance-field units: the field spans `2 × SPREAD` atlas pixels,
        // an atlas pixel is `size / FONT_PX` CSS pixels.
        let halo_width = (HALO_PX / (label.size / FONT_PX) / (2.0 * SPREAD)).clamp(0.0, 0.5);
        atlas.shape(&label.text, label.size, |rect, uv| {
            out.push(LabelGlyph {
                anchor: label.anchor.to_array(),
                rect: [
                    label.pen.x + rect[0],
                    label.pen.y + rect[1],
                    rect[2],
                    rect[3],
                ],
                uv,
                color,
                halo: HALO,
                halo_width,
                appear: label.appear,
                class: label.class,
            });
        });
    }
}
