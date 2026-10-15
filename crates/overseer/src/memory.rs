//! The signed-in user's Memory and Memory segments.

use crate::Overseer;

/// The signed-in user's Memory and Memory segments.
///
/// Created by [`Overseer::memory`]; cheap to clone.
#[derive(Clone, Debug)]
pub struct Memory {
    #[expect(dead_code, reason = "read by the memory features")]
    overseer: Overseer,
}

impl Memory {
    pub(crate) fn new(overseer: Overseer) -> Self {
        Self { overseer }
    }
}
