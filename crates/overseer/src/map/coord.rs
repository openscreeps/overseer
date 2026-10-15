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
}
