//! The signed-in user's account: profile, overview statistics, world status, start room, respawn, CPU and resources, badge, notification preferences and power creeps.

use crate::Overseer;

/// The signed-in user's account: profile, overview statistics, world status, start room, respawn, CPU and resources, badge, notification preferences and power creeps.
///
/// Created by [`Overseer::account`]; cheap to clone.
#[derive(Clone, Debug)]
pub struct Account {
    #[expect(dead_code, reason = "read by the account features")]
    overseer: Overseer,
}

impl Account {
    pub(crate) fn new(overseer: Overseer) -> Self {
        Self { overseer }
    }
}
