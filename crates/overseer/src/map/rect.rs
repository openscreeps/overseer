//! Rectangles of rooms and the bounds of a world.

use super::RoomCoord;

/// An inclusive rectangle of rooms, from its north-west corner `min` to its south-east corner
/// `max`. Empty when `min` lies east or south of `max`.
///
/// ```
/// use overseer::map::{RoomCoord, RoomRect};
///
/// let rect = RoomRect::new("W1N1".parse()?, "E1N0".parse()?);
/// assert_eq!((rect.width(), rect.height(), rect.len()), (4, 2, 8));
/// assert!(rect.contains("W0N0".parse()?));
/// assert_eq!(rect.iter().next(), Some("W1N1".parse::<RoomCoord>()?));
/// # Ok::<(), overseer::Error>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RoomRect {
    /// The north-west corner.
    pub min: RoomCoord,
    /// The south-east corner (inclusive).
    pub max: RoomCoord,
}

impl RoomRect {
    /// The rectangle spanned by two opposite corners, in any order.
    #[must_use]
    pub fn new(a: RoomCoord, b: RoomCoord) -> Self {
        Self {
            min: RoomCoord {
                x: a.x.min(b.x),
                y: a.y.min(b.y),
            },
            max: RoomCoord {
                x: a.x.max(b.x),
                y: a.y.max(b.y),
            },
        }
    }

    /// Whether `room` lies inside.
    #[must_use]
    pub const fn contains(&self, room: RoomCoord) -> bool {
        self.min.x <= room.x && room.x <= self.max.x && self.min.y <= room.y && room.y <= self.max.y
    }

    /// Columns (rooms from west to east); 0 when empty.
    #[must_use]
    pub fn width(&self) -> u32 {
        span(self.min.x, self.max.x)
    }

    /// Rows (rooms from north to south); 0 when empty.
    #[must_use]
    pub fn height(&self) -> u32 {
        span(self.min.y, self.max.y)
    }

    /// The number of rooms.
    #[must_use]
    pub fn len(&self) -> usize {
        usize::try_from(u64::from(self.width()) * u64::from(self.height())).unwrap_or(usize::MAX)
    }

    /// Whether no room lies inside.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y
    }

    /// The room at the centre (the north-western one of the central rooms when a side has an
    /// even length).
    #[must_use]
    pub const fn center(&self) -> RoomCoord {
        RoomCoord {
            x: self.min.x + (self.max.x - self.min.x).div_euclid(2),
            y: self.min.y + (self.max.y - self.min.y).div_euclid(2),
        }
    }

    /// The rooms inside both rectangles; `None` when they do not overlap.
    #[must_use]
    pub fn intersection(&self, other: &Self) -> Option<Self> {
        let rect = Self {
            min: RoomCoord {
                x: self.min.x.max(other.min.x),
                y: self.min.y.max(other.min.y),
            },
            max: RoomCoord {
                x: self.max.x.min(other.max.x),
                y: self.max.y.min(other.max.y),
            },
        };
        (!rect.is_empty()).then_some(rect)
    }

    /// The smallest rectangle that holds the rooms of the two rectangles. When one rectangle
    /// is empty, the result is the other rectangle. When the two are empty, the result is
    /// `other`.
    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        if self.is_empty() {
            return *other;
        }
        if other.is_empty() {
            return *self;
        }
        Self {
            min: RoomCoord {
                x: self.min.x.min(other.min.x),
                y: self.min.y.min(other.min.y),
            },
            max: RoomCoord {
                x: self.max.x.max(other.max.x),
                y: self.max.y.max(other.max.y),
            },
        }
    }

    /// The rooms row by row from the north-west corner.
    pub fn iter(&self) -> impl Iterator<Item = RoomCoord> + use<> {
        let (min, max) = (self.min, self.max);
        (min.y..=max.y).flat_map(move |y| (min.x..=max.x).map(move |x| RoomCoord { x, y }))
    }

    /// The rooms of a world reported as `width` × `height` rooms (`GET /api/game/world-size`)
    /// whose rooms lie on the given sides of the `W0`|`E0` and `N0`|`S0` lines.
    ///
    /// The server measures each axis from its westernmost (northernmost) room to its
    /// easternmost (southernmost) one, but always counts the `E0` (`S0`) line
    /// (`@screeps/common` `calcWorldSize` starts the maximum at 0). So:
    ///
    /// - rooms on both sides (screeps.com shards): the world is centred, `W(n/2-1)` to
    ///   `E(n/2-1)` for an even size `n`; an odd size has the extra room in the west (north);
    /// - west (north) only, like the upstream default world and `OpenScreeps` worlds, which
    ///   generate `W0N0` to `W(k-1)N(k-1)` and report `k + 1`: `W(n-2)` to `W0`;
    /// - east (south) only: `E0` to `E(n-1)`.
    #[must_use]
    pub(crate) fn world(width: u32, height: u32, sides: Sides) -> Self {
        let (min_x, max_x) = axis(width, sides.west, sides.east);
        let (min_y, max_y) = axis(height, sides.north, sides.south);
        Self {
            min: RoomCoord { x: min_x, y: min_y },
            max: RoomCoord { x: max_x, y: max_y },
        }
    }
}

/// Which sides of the `W0`|`E0` and `N0`|`S0` lines hold rooms.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "four independent observations, one per side"
)]
pub(crate) struct Sides {
    /// Rooms named `W…`.
    pub(crate) west: bool,
    /// Rooms named `E…`.
    pub(crate) east: bool,
    /// Rooms named `…N…`.
    pub(crate) north: bool,
    /// Rooms named `…S…`.
    pub(crate) south: bool,
}

/// The first and last coordinate of an axis of `size` rooms with rooms on the negative and/or
/// positive side (both when neither is known).
fn axis(size: u32, negative: bool, positive: bool) -> (i32, i32) {
    let size = i32::try_from(size).unwrap_or(i32::MAX);
    match (negative, positive) {
        (true, false) => (1 - size, -1),
        (false, true) => (0, size - 1),
        _ => (-(size + 1).div_euclid(2), size.div_euclid(2) - 1),
    }
}

fn span(min: i32, max: i32) -> u32 {
    u32::try_from(i64::from(max) - i64::from(min) + 1).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{RoomRect, Sides};
    use crate::map::RoomCoord;

    fn room(name: &str) -> RoomCoord {
        name.parse().unwrap()
    }

    fn names(rect: RoomRect) -> (String, String) {
        (rect.min.to_string(), rect.max.to_string())
    }

    const BOTH: Sides = Sides {
        west: true,
        east: true,
        north: true,
        south: true,
    };
    const NORTH_WEST: Sides = Sides {
        west: true,
        east: false,
        north: true,
        south: false,
    };
    const SOUTH_EAST: Sides = Sides {
        west: false,
        east: true,
        north: false,
        south: true,
    };

    #[test]
    fn centred_worlds_split_at_the_zero_lines() {
        // screeps.com shard0: 182 rooms, W90..E90.
        let rect = RoomRect::world(182, 182, BOTH);
        assert_eq!(names(rect), ("W90N90".into(), "E90S90".into()));
        assert_eq!(rect.len(), 182 * 182);
        // Odd sizes put the extra room in the west and north.
        let rect = RoomRect::world(11, 11, BOTH);
        assert_eq!(names(rect), ("W5N5".into(), "E4S4".into()));
        assert_eq!((rect.width(), rect.height()), (11, 11));
        // Unknown sides count as both.
        assert_eq!(
            RoomRect::world(12, 12, Sides::default()),
            RoomRect::world(12, 12, BOTH)
        );
    }

    #[test]
    fn one_sided_worlds_follow_the_server_measure() {
        // OpenScreeps `--world-size 11` (and the upstream default world): W10N10..W0N0, reported
        // as 12.
        let rect = RoomRect::world(12, 12, NORTH_WEST);
        assert_eq!(names(rect), ("W10N10".into(), "W0N0".into()));
        assert_eq!(rect.len(), 121);
        // `--world-size 10`: W9N9..W0N0, reported as 11.
        let rect = RoomRect::world(11, 11, NORTH_WEST);
        assert_eq!(names(rect), ("W9N9".into(), "W0N0".into()));
        assert_eq!(rect.len(), 100);
        let rect = RoomRect::world(5, 4, SOUTH_EAST);
        assert_eq!(names(rect), ("E0S0".into(), "E4S3".into()));
    }

    #[test]
    fn rectangles_measure_contain_and_iterate() {
        let rect = RoomRect::new(room("E1N0"), room("W1N1"));
        assert_eq!(names(rect), ("W1N1".into(), "E1N0".into()));
        assert_eq!((rect.width(), rect.height(), rect.len()), (4, 2, 8));
        assert_eq!(rect.center(), room("W0N1"));
        assert!(rect.contains(room("E1N0")) && !rect.contains(room("E1S0")));
        let rooms = rect.iter().map(|room| room.to_string()).collect::<Vec<_>>();
        assert_eq!(
            rooms,
            [
                "W1N1", "W0N1", "E0N1", "E1N1", "W1N0", "W0N0", "E0N0", "E1N0"
            ]
        );
        let other = RoomRect::new(room("E1N0"), room("E5S5"));
        assert_eq!(
            rect.intersection(&other).map(names),
            Some(("E1N0".into(), "E1N0".into()))
        );
        assert_eq!(
            rect.intersection(&RoomRect::new(room("E9S9"), room("E9S9"))),
            None
        );
        let empty = RoomRect {
            min: room("E1S1"),
            max: room("W1N1"),
        };
        assert!(empty.is_empty());
        assert_eq!((empty.len(), empty.iter().count()), (0, 0));
    }

    #[test]
    fn a_union_ignores_an_empty_rectangle() {
        let rect = RoomRect::new(room("W1N1"), room("W0N0"));
        let other = RoomRect::new(room("E2S2"), room("E2S2"));
        assert_eq!(names(rect.union(&other)), ("W1N1".into(), "E2S2".into()));
        let empty = RoomRect {
            min: room("E9S9"),
            max: room("W9N9"),
        };
        assert_eq!(rect.union(&empty), rect);
        assert_eq!(empty.union(&rect), rect);
    }
}
