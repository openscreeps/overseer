//! Links between neighbouring objects of one kind (roads, constructed walls, ramparts): one
//! bit per direction, `Token::kind`'s second byte, which `objects.wgsl` draws.

use super::style::Join;

/// Tile offsets of the link directions, clockwise from north (x east, y south): bit `i` of a
/// link mask is direction `i`.
pub const OFFSETS: [(i8, i8); 8] = [
    (0, -1),
    (1, -1),
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
];

/// A 50×50 set of tiles holding an object of one kind.
#[derive(Clone, Debug)]
pub struct Grid {
    tiles: [bool; 2500],
}

impl Default for Grid {
    fn default() -> Self {
        Self {
            tiles: [false; 2500],
        }
    }
}

impl Grid {
    /// Forgets every tile.
    pub fn clear(&mut self) {
        self.tiles.fill(false);
    }

    /// Marks a tile.
    pub fn insert(&mut self, x: u8, y: u8) {
        if let Some(tile) = self.tiles.get_mut(usize::from(y) * 50 + usize::from(x)) {
            *tile = true;
        }
    }

    /// Whether a tile is marked (tiles outside the room are not).
    pub fn contains(&self, x: i16, y: i16) -> bool {
        (0..50).contains(&x)
            && (0..50).contains(&y)
            && self
                .tiles
                .get(usize::try_from(y * 50 + x).unwrap_or(usize::MAX))
                .copied()
                .unwrap_or(false)
    }

    /// The links of the tile `(x, y)` to its marked neighbours. Joined by
    /// [`Join::Link`]: a bit per marked orthogonal neighbour, and per marked diagonal
    /// neighbour unless one of the two tiles beside both is marked (the path exists
    /// already). Joined by [`Join::Fill`]: a bit per marked orthogonal neighbour, and a
    /// diagonal bit when the diagonal and both orthogonal neighbours beside it are marked
    /// (the corner between the four is filled). [`Join::None`] has no links.
    pub fn links(&self, x: u8, y: u8, join: Join) -> u8 {
        if join == Join::None {
            return 0;
        }
        let at = |direction: usize| {
            let (dx, dy) = OFFSETS[direction % 8];
            self.contains(i16::from(x) + i16::from(dx), i16::from(y) + i16::from(dy))
        };
        let mut links = 0;
        for direction in 0..8 {
            let bit = 1 << direction;
            let linked = if direction % 2 == 0 {
                at(direction)
            } else {
                let (before, after) = (at(direction + 7), at(direction + 1));
                at(direction)
                    && match join {
                        Join::Link => !before && !after,
                        Join::Fill | Join::None => before && after,
                    }
            };
            if linked {
                links |= bit;
            }
        }
        links
    }
}
