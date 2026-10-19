//! World coordinates of rooms.

use std::fmt;
use std::str::FromStr;

use crate::Error;

/// The position of a world room on the map, in rooms: `x` grows eastwards and `y` southwards,
/// `E0S0` is `(0, 0)` and `W0N0` is `(-1, -1)`.
///
/// Parse a name with [`str::parse`]; [`Display`](fmt::Display) gives the name back.
///
/// ```
/// use overseer::map::RoomCoord;
///
/// let center: RoomCoord = "W0N0".parse()?;
/// assert_eq!(center, RoomCoord { x: -1, y: -1 });
/// let rooms: Vec<String> = center.around(1).map(|room| room.to_string()).collect();
/// assert_eq!(rooms[0], "W1N1");
/// assert_eq!(rooms[8], "E0S0");
/// # Ok::<(), overseer::Error>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RoomCoord {
    /// Rooms east of `E0` (negative: west, `W0` is -1).
    pub x: i32,
    /// Rooms south of `S0` (negative: north, `N0` is -1).
    pub y: i32,
}

impl RoomCoord {
    /// The rooms of the square of `radius` rooms around this one (`(2 radius + 1)²` rooms),
    /// row by row from the north-west corner.
    pub fn around(self, radius: u16) -> impl Iterator<Item = Self> {
        let radius = i32::from(radius);
        (-radius..=radius).flat_map(move |dy| {
            (-radius..=radius).map(move |dx| Self {
                x: self.x + dx,
                y: self.y + dy,
            })
        })
    }

    /// The distance to `other` in rooms: the larger of the two axis distances, so that a
    /// diagonal step counts as one room.
    #[must_use]
    pub const fn distance(self, other: Self) -> u32 {
        let (dx, dy) = (self.x.abs_diff(other.x), self.y.abs_diff(other.y));
        if dx > dy { dx } else { dy }
    }

    /// Whether the room can have a controller in a standard world. A highway room (a number of
    /// its name ends in 0) has no controller. The 3×3 rooms at the centre of a sector (both
    /// numbers end in 4 to 6) have no controller: they hold Source Keepers or the sector centre.
    #[must_use]
    pub const fn may_have_controller(self) -> bool {
        let (x, y) = (name_number(self.x) % 10, name_number(self.y) % 10);
        let centre = 4 <= x && x <= 6 && 4 <= y && y <= 6;
        x != 0 && y != 0 && !centre
    }
}

/// The number in a room name of the coordinate `value` (`W0` is -1, `E0` is 0).
pub(crate) const fn name_number(value: i32) -> i32 {
    if value < 0 { -value - 1 } else { value }
}

impl FromStr for RoomCoord {
    type Err = Error;

    /// Parses a world room name such as `W8N3`.
    fn from_str(name: &str) -> Result<Self, Error> {
        let invalid = || Error::InvalidInput(format!("`{name}` is not a world room name"));
        let (horizontal, rest) = name.split_at_checked(1).ok_or_else(invalid)?;
        let split = rest.find(['N', 'S']).ok_or_else(invalid)?;
        let (x, rest) = rest.split_at(split);
        let (vertical, y) = rest.split_at(1);
        let number = |digits: &str| {
            if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            digits.parse::<i32>().ok()
        };
        let (x, y) = (
            number(x).ok_or_else(invalid)?,
            number(y).ok_or_else(invalid)?,
        );
        let x = match horizontal {
            "E" => x,
            "W" => -x - 1,
            _ => return Err(invalid()),
        };
        let y = if vertical == "S" { y } else { -y - 1 };
        Ok(Self { x, y })
    }
}

impl fmt::Display for RoomCoord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (horizontal, x) = if self.x >= 0 {
            ('E', self.x)
        } else {
            ('W', -self.x - 1)
        };
        let (vertical, y) = if self.y >= 0 {
            ('S', self.y)
        } else {
            ('N', -self.y - 1)
        };
        write!(f, "{horizontal}{x}{vertical}{y}")
    }
}

#[cfg(test)]
mod tests {
    use super::RoomCoord;

    #[test]
    fn room_names_round_trip() {
        for (name, x, y) in [
            ("E0S0", 0, 0),
            ("W0N0", -1, -1),
            ("W8N3", -9, -4),
            ("E12S34", 12, 34),
        ] {
            let coord: RoomCoord = name.parse().unwrap();
            assert_eq!(coord, RoomCoord { x, y });
            assert_eq!(coord.to_string(), name);
        }
        for invalid in [
            "", "sim", "W", "WN1", "W1N", "X1N1", "W1X1", "W-1N1", "W1N1a",
        ] {
            assert!(invalid.parse::<RoomCoord>().is_err(), "{invalid}");
        }
    }

    #[test]
    fn around_lists_the_square_row_by_row() {
        let center: RoomCoord = "W1N1".parse().unwrap();
        let rooms = center
            .around(1)
            .map(|room| room.to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            rooms,
            [
                "W2N2", "W1N2", "W0N2", "W2N1", "W1N1", "W0N1", "W2N0", "W1N0", "W0N0"
            ]
        );
        assert_eq!(center.around(0).count(), 1);
    }

    #[test]
    fn distances_count_a_diagonal_step_as_one_room() {
        let room = |name: &str| name.parse::<RoomCoord>().unwrap();
        assert_eq!(room("W0N0").distance(room("E0N0")), 1);
        assert_eq!(room("W0N0").distance(room("E1S1")), 2);
        assert_eq!(room("W8N3").distance(room("W8N3")), 0);
    }

    #[test]
    fn highways_and_sector_centres_have_no_controller() {
        for name in ["W1N1", "W3N7", "E9S9", "W14N13", "E4S3"] {
            let room: RoomCoord = name.parse().unwrap();
            assert!(room.may_have_controller(), "{name}");
        }
        for name in [
            "W0N0", "W0N1", "E10S5", "W15S5", "W14N16", "E5S5", "W4N6", "E6S4",
        ] {
            let room: RoomCoord = name.parse().unwrap();
            assert!(!room.may_have_controller(), "{name}");
        }
    }
}
