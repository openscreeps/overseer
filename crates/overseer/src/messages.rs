//! The signed-in user's private messages.

use crate::Overseer;

/// The signed-in user's private messages.
///
/// Created by [`Overseer::messages`]; cheap to clone.
#[derive(Clone, Debug)]
pub struct Messages {
    #[expect(dead_code, reason = "read by the messages features")]
    overseer: Overseer,
}

impl Messages {
    pub(crate) fn new(overseer: Overseer) -> Self {
        Self { overseer }
    }
}
