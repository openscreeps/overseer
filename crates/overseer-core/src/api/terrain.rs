//! Room terrain, decoded from [`GameRoomTerrainDocument`]s.

use super::types::GameRoomTerrainDocument;

/// Tiles per room side.
const SIZE: usize = 50;
/// Tiles per room.
const TILES: usize = SIZE * SIZE;

/// The terrain of one tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tile {
    /// Walkable at normal cost.
    Plain,
    /// Walkable at high cost.
    Swamp,
    /// Not walkable.
    Wall,
}

/// The 50×50 terrain of a room.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Terrain {
    tiles: Box<[Tile; TILES]>,
}

/// An encoded terrain that is not 2500 decimal digits.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TerrainError {
    /// The text does not have 2500 characters.
    #[error("terrain has {0} bytes, expected 2500 digits")]
    Length(usize),
    /// The character at `index` is not a decimal digit.
    #[error("terrain character {index} is not a decimal digit")]
    Digit {
        /// Byte offset, `y * 50 + x`.
        index: usize,
    },
}

impl Terrain {
    /// Decodes the 2500-digit form, indexed by `y * 50 + x`. Each digit is a bit mask: 1 wall,
    /// 2 swamp; a tile with both bits is a wall.
    ///
    /// # Errors
    ///
    /// Returns [`TerrainError`] unless `encoded` is exactly 2500 decimal digits.
    pub fn decode(encoded: &str) -> Result<Self, TerrainError> {
        let bytes = encoded.as_bytes();
        if bytes.len() != TILES {
            return Err(TerrainError::Length(bytes.len()));
        }
        let mut tiles = Box::new([Tile::Plain; TILES]);
        for (index, (&byte, tile)) in bytes.iter().zip(tiles.iter_mut()).enumerate() {
            if !byte.is_ascii_digit() {
                return Err(TerrainError::Digit { index });
            }
            let mask = byte - b'0';
            *tile = if mask & 1 != 0 {
                Tile::Wall
            } else if mask & 2 != 0 {
                Tile::Swamp
            } else {
                Tile::Plain
            };
        }
        Ok(Self { tiles })
    }

    /// The tile at `(x, y)`, or `None` outside `0..50`.
    #[must_use]
    pub fn get(&self, x: u8, y: u8) -> Option<Tile> {
        let (x, y) = (usize::from(x), usize::from(y));
        (x < SIZE && y < SIZE).then(|| self.tiles[y * SIZE + x])
    }
}

impl GameRoomTerrainDocument {
    /// Decodes [`Self::terrain`].
    ///
    /// # Errors
    ///
    /// Returns [`TerrainError`] unless the terrain is exactly 2500 decimal digits.
    pub fn decode(&self) -> Result<Terrain, TerrainError> {
        Terrain::decode(&self.terrain)
    }
}

#[cfg(test)]
mod tests {
    use super::{Terrain, TerrainError, Tile};

    fn encoded(tiles: &[(usize, u8)]) -> String {
        let mut digits = vec![b'0'; 2500];
        for &(index, digit) in tiles {
            digits[index] = digit;
        }
        String::from_utf8(digits).unwrap()
    }

    #[test]
    fn decodes_row_major_corners() {
        let terrain = Terrain::decode(&encoded(&[
            (0, b'1'),
            (49, b'2'),
            (49 * 50, b'2'),
            (2499, b'1'),
        ]))
        .unwrap();
        assert_eq!(terrain.get(0, 0), Some(Tile::Wall));
        assert_eq!(terrain.get(49, 0), Some(Tile::Swamp));
        assert_eq!(terrain.get(0, 49), Some(Tile::Swamp));
        assert_eq!(terrain.get(49, 49), Some(Tile::Wall));
        assert_eq!(terrain.get(1, 0), Some(Tile::Plain));
        assert_eq!(terrain.get(50, 0), None);
        assert_eq!(terrain.get(0, 50), None);
    }

    #[test]
    fn wall_bit_wins_over_swamp_bit() {
        let terrain = Terrain::decode(&encoded(&[(51, b'3')])).unwrap();
        assert_eq!(terrain.get(1, 1), Some(Tile::Wall));
    }

    #[test]
    fn rejects_wrong_length_and_non_digits() {
        assert_eq!(Terrain::decode("0"), Err(TerrainError::Length(1)));
        assert_eq!(
            Terrain::decode(&encoded(&[(7, b'x')])),
            Err(TerrainError::Digit { index: 7 })
        );
    }
}
