//! Other players: profiles, rooms, badges and leaderboards.

use crate::Overseer;

/// Other players: profiles, rooms, badges and leaderboards.
///
/// Created by [`Overseer::users`]; cheap to clone.
#[derive(Clone, Debug)]
pub struct Users {
    #[expect(dead_code, reason = "read by the users features")]
    overseer: Overseer,
}

impl Users {
    pub(crate) fn new(overseer: Overseer) -> Self {
        Self { overseer }
    }
}
