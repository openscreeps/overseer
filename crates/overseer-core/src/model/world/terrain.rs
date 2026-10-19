//! Room terrain (`GET /api/game/room-terrain`, `POST /api/game/rooms`), decoded into
//! [`Terrain`].
//!
//! A [`Terrain`] keeps its tiles packed, 2 bits per tile ([`Terrain::packed`]): 625 bytes per room,
//! so a whole shard of 182×182 rooms takes about 20 MB. [`Terrain::encoded`] writes the standard
//! 2500-digit form again.

use std::fmt;

use crate::api::community::types as community;
use crate::api::official::types as official;
use crate::api::official::types::{GameTerrainTile, GameTerrainTileType};

/// Tiles per room side.
const SIZE: usize = 50;
/// Tiles per room.
const TILES: usize = SIZE * SIZE;
/// Tiles per packed byte.
const PER_BYTE: usize = 4;

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

impl Tile {
    /// The 2-bit code of the packed form: 0 plain, 1 wall, 2 swamp.
    const fn code(self) -> u8 {
        match self {
            Self::Plain => 0,
            Self::Wall => 1,
            Self::Swamp => 2,
        }
    }
}

/// The 50×50 terrain of a room, packed 2 bits per tile.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Terrain {
    packed: Box<[u8; Self::PACKED_LEN]>,
}

/// A terrain reply that does not decode.
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
    /// The packed tile at `index` has the unused code 3.
    #[error("packed terrain tile {index} has the invalid code 3")]
    Packed {
        /// Tile index, `y * 50 + x`.
        index: usize,
    },
    /// A listed tile lies outside the room.
    #[error("terrain tile ({x}, {y}) is outside the room")]
    Outside {
        /// The tile's x coordinate.
        x: u64,
        /// The tile's y coordinate.
        y: u64,
    },
    /// The reply holds no terrain document.
    #[error("the reply holds no terrain")]
    Missing,
}

/// Why the terrain does not let a structure stand on a tile ([`Terrain::check_structure_spot`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SpotError {
    /// The tile is on the room edge or outside the room.
    #[error("on or beyond the room edge")]
    Edge,
    /// The tile is a wall.
    #[error("a wall")]
    Wall,
    /// The tile is next to an exit.
    #[error("next to an exit")]
    Exit,
}

impl Terrain {
    /// Bytes of the packed form ([`Self::packed`]).
    pub const PACKED_LEN: usize = TILES / PER_BYTE;

    /// Decodes the 2500-digit form, indexed by `y * 50 + x`. Each digit is a bit mask: 1 wall,
    /// 2 swamp; a tile with both bits is a wall. [`Self::encoded`] writes this form again.
    ///
    /// # Errors
    ///
    /// Returns [`TerrainError`] unless `encoded` is exactly 2500 decimal digits.
    pub fn decode(encoded: &str) -> Result<Self, TerrainError> {
        let bytes = encoded.as_bytes();
        if bytes.len() != TILES {
            return Err(TerrainError::Length(bytes.len()));
        }
        let mut packed = Box::new([0; Self::PACKED_LEN]);
        for (index, &byte) in bytes.iter().enumerate() {
            if !byte.is_ascii_digit() {
                return Err(TerrainError::Digit { index });
            }
            let mask = byte - b'0';
            let tile = if mask & 1 != 0 {
                Tile::Wall
            } else if mask & 2 != 0 {
                Tile::Swamp
            } else {
                Tile::Plain
            };
            packed[index / PER_BYTE] |= tile.code() << shift(index);
        }
        Ok(Self { packed })
    }

    /// The terrain of the tile form of `GET /api/game/room-terrain` (without `encoded`): every
    /// wall and swamp tile, every other tile plain. A tile listed twice keeps the wall.
    ///
    /// # Errors
    ///
    /// Returns [`TerrainError::Outside`] for a tile outside `0..50`.
    pub fn from_tiles(tiles: &[GameTerrainTile]) -> Result<Self, TerrainError> {
        let mut packed = Box::new([0; Self::PACKED_LEN]);
        for tile in tiles {
            let (x, y) = (tile.x, tile.y);
            let outside = TerrainError::Outside { x, y };
            let column = usize::try_from(x).map_err(|_| outside.clone())?;
            let row = usize::try_from(y).map_err(|_| outside.clone())?;
            if column >= SIZE || row >= SIZE {
                return Err(outside);
            }
            let index = row * SIZE + column;
            let code = match tile.r#type {
                GameTerrainTileType::Wall => Tile::Wall,
                GameTerrainTileType::Swamp => Tile::Swamp,
            }
            .code();
            let byte = &mut packed[index / PER_BYTE];
            let current = (*byte >> shift(index)) & 0b11;
            // A wall stays a wall (a tile with both bits is a wall in the encoded form too).
            if current != Tile::Wall.code() {
                *byte &= !(0b11 << shift(index));
                *byte |= code << shift(index);
            }
        }
        Ok(Self { packed })
    }

    /// Reads the packed form ([`Self::packed`]).
    ///
    /// # Errors
    ///
    /// Returns [`TerrainError::Packed`] for the first tile with the unused code 3.
    pub fn from_packed(packed: &[u8; Self::PACKED_LEN]) -> Result<Self, TerrainError> {
        for (byte_index, &byte) in packed.iter().enumerate() {
            // A tile is 3 exactly when both of its bits are set.
            let both = byte & (byte >> 1) & 0b0101_0101;
            if both != 0 {
                let within = both.trailing_zeros() as usize / 2;
                return Err(TerrainError::Packed {
                    index: byte_index * PER_BYTE + within,
                });
            }
        }
        Ok(Self {
            packed: Box::new(*packed),
        })
    }

    /// The packed form: tile `y * 50 + x` sits in byte `index / 4` at bits `2 * (index % 4)`
    /// (low bits first); code 0 is plain, 1 wall, 2 swamp, 3 never occurs.
    #[must_use]
    pub fn packed(&self) -> &[u8; Self::PACKED_LEN] {
        &self.packed
    }

    /// The standard 2500-digit form, indexed by `y * 50 + x`: `0` plain, `1` wall, `2` swamp.
    /// [`Self::decode`] reads it back to the same terrain. The digits are the normalized
    /// tiles, so a wall that [`Self::decode`] read with both bits (`3`) comes back as `1`.
    ///
    /// The result writes the digits through `Display` without an allocation: use `write!`,
    /// serde's `Serializer::collect_str`, or `to_string` for a `String`.
    #[must_use]
    pub const fn encoded(&self) -> EncodedTerrain<'_> {
        EncodedTerrain(self)
    }

    /// The tile at `(x, y)`, or `None` outside `0..50`.
    #[must_use]
    pub fn get(&self, x: u8, y: u8) -> Option<Tile> {
        let (x, y) = (usize::from(x), usize::from(y));
        if x >= SIZE || y >= SIZE {
            return None;
        }
        let index = y * SIZE + x;
        Some(
            match (self.packed[index / PER_BYTE] >> shift(index)) & 0b11 {
                1 => Tile::Wall,
                2 => Tile::Swamp,
                _ => Tile::Plain,
            },
        )
    }

    /// Checks that the terrain lets a structure stand on the tile `(x, y)`, as the game checks a
    /// new construction site and the first spawn (`POST /api/game/place-spawn`). The tile must be
    /// inside the room edge (1 to 48). It must not be a wall. It must not be next to an exit: an
    /// edge tile that is not a wall. Roads, containers and extractors have other rules. This
    /// check does not look at the objects on the tile.
    ///
    /// # Errors
    ///
    /// Returns the [`SpotError`] of the first rule that the tile breaks.
    pub fn check_structure_spot(&self, x: u8, y: u8) -> Result<(), SpotError> {
        const LAST: u8 = 49;
        if x == 0 || y == 0 || x >= LAST || y >= LAST {
            return Err(SpotError::Edge);
        }
        if self.get(x, y) == Some(Tile::Wall) {
            return Err(SpotError::Wall);
        }
        for ny in y - 1..=y + 1 {
            for nx in x - 1..=x + 1 {
                let edge = nx == 0 || ny == 0 || nx == LAST || ny == LAST;
                if edge && self.get(nx, ny) != Some(Tile::Wall) {
                    return Err(SpotError::Exit);
                }
            }
        }
        Ok(())
    }
}

/// The 2500-digit form of a [`Terrain`], written by `Display` (see [`Terrain::encoded`]).
#[derive(Clone, Copy, Debug)]
pub struct EncodedTerrain<'a>(&'a Terrain);

impl fmt::Display for EncodedTerrain<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut row = [0; SIZE];
        for y in 0..SIZE {
            for (x, digit) in row.iter_mut().enumerate() {
                let index = y * SIZE + x;
                // The packed code is the digit: 0 plain, 1 wall, 2 swamp.
                *digit = b'0' + ((self.0.packed[index / PER_BYTE] >> shift(index)) & 0b11);
            }
            f.write_str(std::str::from_utf8(&row).map_err(|_| fmt::Error)?)?;
        }
        Ok(())
    }
}

/// Bit offset of tile `index` within its packed byte.
const fn shift(index: usize) -> usize {
    (index % PER_BYTE) * 2
}

/// The terrain document (`rooms.terrain`, the item of `POST /api/game/rooms`) and the
/// `GET /api/game/room-terrain` reply in either form (the encoded document with `encoded=1`,
/// or the wall and swamp tiles) of the contract `$module`. `OpenScreeps` shares the official
/// types; the community contract has its own (its documents can lack `type`).
macro_rules! terrain_from {
    ($($module:ident),+) => {$(
        impl TryFrom<&$module::GameRoomTerrainDocument> for Terrain {
            type Error = TerrainError;

            fn try_from(document: &$module::GameRoomTerrainDocument) -> Result<Self, TerrainError> {
                Self::decode(&document.terrain)
            }
        }

        impl TryFrom<$module::GameRoomTerrainResponse> for Terrain {
            type Error = TerrainError;

            fn try_from(reply: $module::GameRoomTerrainResponse) -> Result<Self, TerrainError> {
                match reply {
                    $module::GameRoomTerrainResponse::Encoded(encoded) => encoded
                        .terrain
                        .first()
                        .ok_or(TerrainError::Missing)
                        .and_then(Self::try_from),
                    $module::GameRoomTerrainResponse::Tiles(tiles) => {
                        Self::from_tiles(&tiles.terrain)
                    }
                }
            }
        }
    )+};
}

terrain_from!(official, community);

#[cfg(test)]
mod tests {
    use super::{SpotError, Terrain, TerrainError, Tile};
    use crate::api::official::types::GameRoomTerrainResponse;

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

    #[test]
    fn packed_round_trip_keeps_every_tile() {
        let digits: String = (0..2500)
            .map(|i: usize| match (i * 7 + i / 50) % 3 {
                0 => '0',
                1 => '1',
                _ => '2',
            })
            .collect();
        let terrain = Terrain::decode(&digits).unwrap();
        let packed = terrain.packed();
        assert_eq!(packed.len(), Terrain::PACKED_LEN);
        assert_eq!(packed.len(), 625);
        let back = Terrain::from_packed(packed).unwrap();
        assert_eq!(back, terrain);
        for y in 0..50 {
            for x in 0..50 {
                assert_eq!(back.get(x, y), terrain.get(x, y), "{x},{y}");
            }
        }
        assert_eq!(terrain.encoded().to_string(), digits);
    }

    #[test]
    fn encoded_form_is_the_normalized_digits_and_decodes_back() {
        // Room corners, the packed-byte boundary 3/4, the row boundary 49/50 and a tile with
        // both bits (a wall).
        let tiles = [
            (0, b'1'),
            (3, b'2'),
            (4, b'1'),
            (49, b'2'),
            (50, b'3'),
            (51, b'2'),
            (49 * 50, b'2'),
            (2499, b'1'),
        ];
        let terrain = Terrain::decode(&encoded(&tiles)).unwrap();
        let normalized =
            tiles.map(|(index, digit)| (index, if digit == b'3' { b'1' } else { digit }));
        let text = terrain.encoded().to_string();
        assert_eq!(text, encoded(&normalized));
        assert_eq!(Terrain::decode(&text).unwrap(), terrain);
    }

    #[test]
    fn packed_layout_is_row_major_low_bits_first() {
        let terrain =
            Terrain::decode(&encoded(&[(0, b'1'), (1, b'2'), (3, b'3'), (51, b'2')])).unwrap();
        let packed = terrain.packed();
        // Tiles 0..4: wall (1), swamp (2 << 2), plain, wall (1 << 6).
        assert_eq!(packed[0], 0b0100_1001);
        // Tile 51 = (1, 1): byte 12, slot 3.
        assert_eq!(packed[12], 2 << 6);
        assert!(packed.iter().skip(13).all(|&b| b == 0));
    }

    #[test]
    fn from_packed_rejects_code_three() {
        let mut packed = [0_u8; 625];
        packed[10] = 0b0011_0000;
        assert_eq!(
            Terrain::from_packed(&packed),
            Err(TerrainError::Packed { index: 42 })
        );
        packed[10] = 0b0010_0100;
        assert!(Terrain::from_packed(&packed).is_ok());
        let mut last = [0_u8; 625];
        last[624] = 0b1100_0000;
        assert_eq!(
            Terrain::from_packed(&last),
            Err(TerrainError::Packed { index: 2499 })
        );
    }

    /// The encoded reply (`encoded=1`), as every server sends it.
    fn encoded_reply() -> String {
        let terrain = encoded(&[(0, b'1'), (51, b'2'), (2499, b'3')]);
        format!(
            r#"{{"terrain":[{{"_id":"5873bd","room":"W1N1","terrain":"{terrain}","type":"terrain"}}]}}"#
        )
    }

    /// The tile reply (no `encoded`): walls and swamps only, as every server sends it.
    const TILES_REPLY: &str = r#"{"terrain":[
        {"room":"W1N1","x":0,"y":0,"type":"wall"},
        {"room":"W1N1","x":1,"y":1,"type":"swamp"},
        {"room":"W1N1","x":49,"y":49,"type":"wall"},
        {"room":"W1N1","x":49,"y":49,"type":"swamp"}]}"#;

    fn check(terrain: &Terrain) {
        assert_eq!(terrain.get(0, 0), Some(Tile::Wall));
        assert_eq!(terrain.get(1, 1), Some(Tile::Swamp));
        assert_eq!(terrain.get(49, 49), Some(Tile::Wall));
        assert_eq!(terrain.get(2, 0), Some(Tile::Plain));
    }

    #[test]
    fn both_reply_forms_decode_alike() {
        let encoded: GameRoomTerrainResponse = serde_json::from_str(&encoded_reply()).unwrap();
        let tiles: GameRoomTerrainResponse = serde_json::from_str(TILES_REPLY).unwrap();
        assert!(matches!(tiles, GameRoomTerrainResponse::Tiles(_)));
        let terrain = Terrain::try_from(encoded).unwrap();
        check(&terrain);
        assert_eq!(Terrain::try_from(tiles).unwrap(), terrain);
    }

    #[test]
    fn empty_and_outside_replies_are_rejected() {
        let empty: GameRoomTerrainResponse = serde_json::from_str(r#"{"terrain":[]}"#).unwrap();
        // An empty list reads as the encoded form first: no document.
        assert_eq!(Terrain::try_from(empty), Err(TerrainError::Missing));
        let outside: GameRoomTerrainResponse =
            serde_json::from_str(r#"{"terrain":[{"room":"W1N1","x":50,"y":0,"type":"wall"}]}"#)
                .unwrap();
        assert_eq!(
            Terrain::try_from(outside),
            Err(TerrainError::Outside { x: 50, y: 0 })
        );
    }

    #[test]
    fn structures_stand_inside_the_edge_off_walls_and_away_from_exits() {
        // Walls on the whole edge but one exit tile at (10, 0); a wall at (30, 25) and a swamp
        // at (31, 25).
        let mut tiles: Vec<(usize, u8)> = (0..50)
            .flat_map(|at| [at, 49 * 50 + at, at * 50, at * 50 + 49])
            .filter(|&index| index != 10)
            .map(|index| (index, b'1'))
            .collect();
        tiles.extend([(25 * 50 + 30, b'3'), (25 * 50 + 31, b'2')]);
        let terrain = Terrain::decode(&encoded(&tiles)).unwrap();

        assert_eq!(terrain.check_structure_spot(25, 25), Ok(()));
        assert_eq!(terrain.check_structure_spot(31, 25), Ok(()));
        assert_eq!(terrain.check_structure_spot(20, 1), Ok(()));
        assert_eq!(terrain.check_structure_spot(0, 25), Err(SpotError::Edge));
        assert_eq!(terrain.check_structure_spot(25, 49), Err(SpotError::Edge));
        assert_eq!(terrain.check_structure_spot(25, 255), Err(SpotError::Edge));
        assert_eq!(terrain.check_structure_spot(30, 25), Err(SpotError::Wall));
        assert_eq!(terrain.check_structure_spot(11, 1), Err(SpotError::Exit));
        assert_eq!(terrain.check_structure_spot(9, 1), Err(SpotError::Exit));
    }
}
