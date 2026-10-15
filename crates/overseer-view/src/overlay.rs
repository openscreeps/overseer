//! The overlay over the world map: owners' badges, portals (marks, arcs between linked
//! rooms, the names of the shards they lead to), room coordinates as labels and the
//! hovered room's outline. Sizes are CSS pixels (within tile sizes for marks), so the
//! overlay reads the same at every zoom; each part fades by level of detail
//! ([`Overlay::fades`]) in the shader, so zooming changes no instance.
//!
//! Instances are rebuilt only when their data changes: badges and portal marks when an
//! owner, reservation, badge or portal changes; sector labels with the bounds and portals;
//! room labels when the view leaves the rectangle they were made for (the view and a
//! margin around it) or zooms to another grid ([`labels::room_tier`]).

pub mod badges;
pub mod labels;
pub mod portals;

use overseer::map::{Portal, RoomCoord, RoomRect};

use crate::camera::room_centre;
use crate::mesh::{ArcInstance, LabelGlyph, MarkInstance, mark, rgba};
use crate::text::Atlas;
use crate::world::Cells;
use crate::{BADGE_PX_PER_ROOM, MINIMAP_PX_PER_ROOM, TERRAIN_PX_PER_ROOM};
pub use badges::Badges;
pub use labels::Label;
pub use portals::Portals;

/// Room sizes (CSS pixels) over which sector labels fade out: room labels take over.
const SECTOR_LABELS_PX: (f32, f32) = (TERRAIN_PX_PER_ROOM * 0.75, TERRAIN_PX_PER_ROOM);
/// Room sizes (CSS pixels) over which room labels fade in.
const ROOM_LABELS_PX: (f32, f32) = (TERRAIN_PX_PER_ROOM, TERRAIN_PX_PER_ROOM * 1.25);
/// Room sizes (CSS pixels) over which badges fade in (smaller rooms leave no room).
const BADGES_PX: (f32, f32) = (BADGE_PX_PER_ROOM, BADGE_PX_PER_ROOM * 1.5);
/// Room sizes (CSS pixels) over which the arcs between linked portals fade out.
const ARCS_PX: (f32, f32) = (TERRAIN_PX_PER_ROOM, MINIMAP_PX_PER_ROOM);
/// Most room labels made at once.
const ROOM_LABELS: usize = 6000;
/// Badge of an owned room: radius in tiles (60 % of the room wide) and in CSS pixels at
/// least and at most.
const BADGE_RADIUS: [f32; 3] = [15.0, 5.0, 20.0];
/// Badge of a reserved room: smaller, and faded.
const RESERVED_RADIUS: [f32; 3] = [7.0, 3.0, 9.0];
const RESERVED_ALPHA: u8 = 140;
/// Hovered room outline: the room, at least 3 CSS pixels wide.
const HOVER_RADIUS: [f32; 3] = [25.0, 3.0, f32::MAX];
const HOVER_COLOR: [u8; 4] = rgba(0xff_ff_ff, 200);

/// Opacity of each overlay part at the current zoom, `0..=1`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fades {
    /// Sector, room and shard labels, by [`labels::class`] (`w` unused).
    pub labels: [f32; 4],
    /// Badges, portal arcs, portal marks and the hover outline.
    pub marks: [f32; 4],
}

impl Fades {
    /// The fades of a world view whose rooms are `px_per_room` CSS pixels wide, its
    /// detail layer at `detail` (`view::Fades::detail`).
    pub fn at(px_per_room: f32, detail: f32) -> Self {
        let ramp = |(from, to): (f32, f32)| smoothstep(from, to, px_per_room);
        Self {
            labels: [1.0 - ramp(SECTOR_LABELS_PX), ramp(ROOM_LABELS_PX), 1.0, 0.0],
            marks: [
                ramp(BADGES_PX) * (1.0 - detail),
                1.0 - ramp(ARCS_PX),
                1.0 - 0.5 * detail,
                1.0,
            ],
        }
    }

    /// A room scope: only the hover outline.
    pub const ROOM: Self = Self {
        labels: [0.0; 4],
        marks: [0.0, 0.0, 0.0, 1.0],
    };
}

fn smoothstep(from: f32, to: f32, value: f32) -> f32 {
    let t = ((value - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Where the camera looks, for the room labels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// The scope's rooms.
    pub bounds: RoomRect,
    /// The rooms in view.
    pub visible: RoomRect,
    /// CSS pixels per room.
    pub px_per_room: f32,
    /// A world scope (a room scope draws no labels).
    pub world: bool,
}

/// GPU data that changes as a whole: the items and a version bumped on every change.
#[derive(Debug, Default)]
pub struct Layer<T> {
    /// The instances.
    pub items: Vec<T>,
    /// Bumped whenever `items` changed.
    pub version: u64,
}

impl<T> Layer<T> {
    fn replace(&mut self, fill: impl FnOnce(&mut Vec<T>)) {
        self.items.clear();
        fill(&mut self.items);
        self.version += 1;
    }
}

/// The overlay's data.
#[derive(Debug)]
pub struct Overlay {
    /// The badge atlas.
    pub badges: Badges,
    /// Every room's portals.
    pub portals: Portals,
    hover: Option<RoomCoord>,
    /// Owners' and reservers' badges.
    pub marks: Layer<MarkInstance>,
    /// Portal marks.
    pub portal_marks: Layer<MarkInstance>,
    /// Arcs between linked portals.
    pub arcs: Layer<ArcInstance>,
    /// Sector and shard labels and their glyphs.
    overview: Vec<Label>,
    pub overview_glyphs: Layer<LabelGlyph>,
    /// Room labels, the rectangle and grid they were made for, and their glyphs.
    rooms: Vec<Label>,
    room_key: Option<(RoomRect, u32)>,
    pub room_glyphs: Layer<LabelGlyph>,
    /// Badges need placing again.
    marks_stale: bool,
    /// Portal marks and arcs need rebuilding.
    portals_stale: bool,
    /// Sector and shard labels need rebuilding (the room labels too).
    labels_stale: bool,
    /// The bounds the labels were made for.
    bounds: Option<RoomRect>,
}

impl Overlay {
    /// An empty overlay whose badge texture may have up to `max_layers` layers.
    pub fn new(max_layers: u32) -> Self {
        Self {
            badges: Badges::new(max_layers),
            portals: Portals::default(),
            hover: None,
            marks: Layer::default(),
            portal_marks: Layer::default(),
            arcs: Layer::default(),
            overview: Vec::new(),
            overview_glyphs: Layer::default(),
            rooms: Vec::new(),
            room_key: None,
            room_glyphs: Layer::default(),
            marks_stale: true,
            portals_stale: true,
            labels_stale: true,
            bounds: None,
        }
    }

    /// Owners or reservations changed: the badges are placed again.
    pub const fn owners_changed(&mut self) {
        self.marks_stale = true;
    }

    /// A room's terrain state changed (none on the server): labels that may name it are
    /// made again.
    pub fn room_changed(&mut self, room: RoomCoord) {
        if labels::is_crossroad(room) {
            self.labels_stale = true;
        }
        if self.room_key.is_some_and(|(rect, _)| rect.contains(room)) {
            self.room_key = None;
        }
    }

    /// Stores a user's badge (see [`Badges::set`]).
    pub fn set_badge(&mut self, user: &str, rgba: &[u8], size: u32) {
        if self.badges.set(user, rgba, size) {
            self.marks_stale = true;
        }
    }

    /// Sets a room's portals.
    pub fn set_portals(&mut self, room: RoomCoord, portals: &[Portal]) {
        let (changed, names_changed) = self.portals.set(room, portals);
        self.portals_stale |= changed;
        self.labels_stale |= names_changed;
    }

    /// Sets the hovered room.
    pub const fn set_hover(&mut self, room: Option<RoomCoord>) {
        self.hover = room;
    }

    /// The outline of the hovered room, when it is in `bounds`.
    pub fn hover(&self, bounds: RoomRect) -> Option<MarkInstance> {
        let room = self.hover.filter(|room| bounds.contains(*room))?;
        Some(MarkInstance {
            centre: room_centre(room).to_array(),
            radius: HOVER_RADIUS,
            uv: [0.0; 4],
            color: HOVER_COLOR,
            kind: mark::HOVER,
        })
    }

    /// The sector and shard labels.
    #[cfg(test)]
    pub fn overview_labels(&self) -> &[Label] {
        &self.overview
    }

    /// The room labels.
    #[cfg(test)]
    pub fn room_labels(&self) -> &[Label] {
        &self.rooms
    }

    /// Rebuilds what changed for `frame`: badges from `cells`' owners and reservations,
    /// portal marks and arcs, labels (rooms `absent` from the world get none) with glyphs
    /// from `atlas`.
    pub fn update(
        &mut self,
        frame: &Frame,
        cells: &Cells,
        absent: impl Fn(RoomCoord) -> bool,
        atlas: &mut Atlas,
    ) {
        if self.bounds != Some(frame.bounds) {
            self.bounds = Some(frame.bounds);
            self.labels_stale = true;
            self.marks_stale = true;
        }
        if std::mem::take(&mut self.marks_stale) {
            self.rebuild_badges(frame.bounds, cells);
        }
        if std::mem::take(&mut self.portals_stale) {
            let portals = &self.portals;
            self.portal_marks.replace(|out| portals.marks(out));
            self.arcs.replace(|out| portals.arcs(out));
        }
        if !frame.world {
            return;
        }
        if std::mem::take(&mut self.labels_stale) {
            self.overview = labels::overview(frame.bounds, &absent, &self.portals, atlas);
            let overview = &self.overview;
            self.overview_glyphs
                .replace(|out| labels::glyphs(overview, atlas, out));
            self.room_key = None;
        }
        let Some(tier) = Self::room_tier(frame, atlas) else {
            if self.room_key.is_some() || !self.room_glyphs.items.is_empty() {
                self.rooms.clear();
                self.room_key = None;
                self.room_glyphs.replace(|_| {});
            }
            return;
        };
        let covered = self.room_key.is_some_and(|(rect, made)| {
            made == tier && rect.contains(frame.visible.min) && rect.contains(frame.visible.max)
        });
        if covered {
            return;
        }
        let rect = margin(frame.visible, frame.bounds);
        self.rooms = labels::rooms(rect, tier, &absent, atlas);
        self.room_key = Some((rect, tier));
        let rooms = &self.rooms;
        self.room_glyphs
            .replace(|out| labels::glyphs(rooms, atlas, out));
    }

    /// The grid of room labels wanted at `frame` (`None`: none, zoomed out too far).
    fn room_tier(frame: &Frame, atlas: &mut Atlas) -> Option<u32> {
        if frame.px_per_room < ROOM_LABELS_PX.0 * 0.8 {
            return None;
        }
        let rect = margin(frame.visible, frame.bounds);
        Some(labels::room_tier(
            frame.px_per_room,
            labels::nominal(atlas),
            rect.len(),
            ROOM_LABELS,
        ))
    }

    fn rebuild_badges(&mut self, bounds: RoomRect, cells: &Cells) {
        let badges = &self.badges;
        self.marks.replace(|out| {
            for (room, owner, reserver) in cells.owners() {
                if !bounds.contains(room) {
                    continue;
                }
                let (user, radius, alpha, kind) = match (owner, reserver) {
                    (Some(owner), _) => (owner, BADGE_RADIUS, 255, mark::BADGE),
                    (None, Some(reserver)) => {
                        (reserver, RESERVED_RADIUS, RESERVED_ALPHA, mark::RESERVED)
                    }
                    (None, None) => continue,
                };
                let Some((uv, layer)) = badges.cell(user) else {
                    continue;
                };
                out.push(MarkInstance {
                    centre: room_centre(room).to_array(),
                    radius,
                    uv,
                    color: rgba(0xff_ff_ff, alpha),
                    kind: kind | (layer << 8),
                });
            }
        });
    }
}

/// `visible` grown by half its size each way, within `bounds`.
fn margin(visible: RoomRect, bounds: RoomRect) -> RoomRect {
    let grow_x = i32::try_from(visible.width() / 2).unwrap_or(0) + 1;
    let grow_y = i32::try_from(visible.height() / 2).unwrap_or(0) + 1;
    let min = RoomCoord {
        x: (visible.min.x - grow_x).max(bounds.min.x),
        y: (visible.min.y - grow_y).max(bounds.min.y),
    };
    let max = RoomCoord {
        x: (visible.max.x + grow_x).min(bounds.max.x),
        y: (visible.max.y + grow_y).min(bounds.max.y),
    };
    RoomRect::new(min, max)
}
