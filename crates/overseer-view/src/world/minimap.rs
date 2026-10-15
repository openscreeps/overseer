//! Room minimaps (`roomMap2`): one slot of [`POINTS`] points per room, up to
//! [`MINIMAP_ROOMS`](crate::MINIMAP_ROOMS) rooms.

use std::collections::HashMap;

use overseer::map::{RoomCoord, RoomMap};

use super::{Dirty, Focus};
use crate::MINIMAP_ROOMS;
use crate::camera::room_origin;
use crate::mesh::{PointInstance, rgba};
use crate::scene::style::Owner;

/// Points per room: a room has 2500 tiles; further points are dropped.
pub const POINTS: usize = 2500;

/// How each kind is drawn: key, sRGB colour, square side (tiles). In drawing order: later
/// kinds cover earlier ones; players' creeps and structures come last.
const KINDS: [(&str, u32, f32); 8] = [
    ("r", 0x8a_8a_8a, 0.8),
    ("w", 0x55_55_55, 1.0),
    ("p", 0x5a_d1_ff, 1.6),
    ("pb", 0xff_4f_a0, 2.0),
    ("k", 0xab_47_bc, 2.0),
    ("m", 0xc5_ca_e9, 2.0),
    ("s", 0xff_f2_46, 2.0),
    ("c", 0xf0_f0_f0, 2.0),
];
/// Square side of a player's point.
const USER_SIZE: f32 = 1.0;

/// A room's minimap and its point count.
#[derive(Debug)]
struct Slot {
    room: RoomCoord,
    map: RoomMap,
    count: u32,
}

/// The minimap slots and their points (slot-major, [`POINTS`] per slot).
#[derive(Debug)]
pub struct Minimaps {
    slots: Vec<Option<Slot>>,
    by_room: HashMap<RoomCoord, usize>,
    points: Vec<PointInstance>,
    /// Slots whose points changed since the GPU took them.
    pub dirty: Dirty,
}

impl Default for Minimaps {
    fn default() -> Self {
        Self {
            slots: (0..MINIMAP_ROOMS).map(|_| None).collect(),
            by_room: HashMap::new(),
            points: vec![PointInstance::default(); MINIMAP_ROOMS * POINTS],
            dirty: Dirty::default(),
        }
    }
}

impl Minimaps {
    /// Shows (`Some`) or removes (`None`) a room's minimap. When every slot is taken, the
    /// room farthest from the camera gives up its slot (if it is farther than `room`).
    pub fn set(&mut self, room: RoomCoord, map: Option<&RoomMap>, me: Option<&str>, focus: &Focus) {
        let Some(map) = map else {
            if let Some(slot) = self.by_room.remove(&room) {
                self.slots[slot] = None;
            }
            return;
        };
        let slot = if let Some(&slot) = self.by_room.get(&room) {
            slot
        } else {
            let free = self.slots.iter().position(Option::is_none);
            let slot = free.or_else(|| {
                let distance = focus.distance(room);
                self.slots
                    .iter()
                    .enumerate()
                    .filter_map(|(slot, entry)| Some((focus.distance(entry.as_ref()?.room), slot)))
                    .max_by(|a, b| a.0.total_cmp(&b.0))
                    .filter(|(far, _)| *far > distance)
                    .map(|(_, slot)| slot)
            });
            let Some(slot) = slot else {
                return;
            };
            if let Some(old) = self.slots[slot].take() {
                self.by_room.remove(&old.room);
            }
            self.by_room.insert(room, slot);
            slot
        };
        self.slots[slot] = Some(Slot {
            room,
            map: map.clone(),
            count: 0,
        });
        self.fill(slot, me);
    }

    /// Recolours every minimap (the signed-in user changed).
    pub fn recolor(&mut self, me: Option<&str>) {
        for slot in 0..self.slots.len() {
            if self.slots[slot].is_some() {
                self.fill(slot, me);
            }
        }
    }

    /// Writes the points of a slot.
    fn fill(&mut self, slot: usize, me: Option<&str>) {
        let Some(entry) = &mut self.slots[slot] else {
            return;
        };
        let origin = room_origin(entry.room);
        let points = &mut self.points[slot * POINTS..(slot + 1) * POINTS];
        let mut count = 0;
        let mut push = |positions: &[[u64; 2]], color: u32, size: f32| {
            for &[x, y] in positions {
                let (Some(x), Some(y)) = (tile(x), tile(y)) else {
                    continue;
                };
                let Some(point) = points.get_mut(count) else {
                    return;
                };
                *point = PointInstance {
                    position: [origin.x + f32::from(x), origin.y + f32::from(y)],
                    size,
                    color: rgba(color, 255),
                };
                count += 1;
            }
        };
        for (key, color, size) in KINDS {
            push(entry.map.get(key), color, size);
        }
        for (user, positions) in entry.map.users() {
            let color = Owner::of(Some(user), me).color().unwrap_or(0xbd_bd_bd);
            push(positions, color, USER_SIZE);
        }
        entry.count = u32::try_from(count).unwrap_or(u32::MAX);
        self.dirty.mark(u32::try_from(slot).unwrap_or(u32::MAX));
    }

    /// The occupied slots: index, room and point count.
    pub fn slots(&self) -> impl Iterator<Item = (usize, RoomCoord, u32)> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(slot, entry)| entry.as_ref().map(|entry| (slot, entry.room, entry.count)))
    }

    /// The points of a slot.
    pub fn points(&self, slot: usize) -> &[PointInstance] {
        let count = self.slots[slot]
            .as_ref()
            .map_or(0, |entry| entry.count as usize);
        &self.points[slot * POINTS..slot * POINTS + count]
    }
}

/// A tile coordinate in `0..50`.
fn tile(value: u64) -> Option<u8> {
    u8::try_from(value).ok().filter(|value| *value < 50)
}
