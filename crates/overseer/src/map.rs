//! The world map: room map views, map statistics, terrain, room status and map visuals.

use crate::Overseer;

/// The world map: room map views, map statistics, terrain, room status and map visuals.
///
/// Created by [`Overseer::map`]; cheap to clone.
#[derive(Clone, Debug)]
pub struct Map {
    #[expect(dead_code, reason = "read by the map features")]
    overseer: Overseer,
}

impl Map {
    pub(crate) fn new(overseer: Overseer) -> Self {
        Self { overseer }
    }
}
