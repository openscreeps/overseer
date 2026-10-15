//! Room history playback.

use crate::Overseer;

/// Room history playback.
///
/// Created by [`Overseer::history`]; cheap to clone.
#[derive(Clone, Debug)]
pub struct History {
    #[expect(dead_code, reason = "read by the history features")]
    overseer: Overseer,
}

impl History {
    pub(crate) fn new(overseer: Overseer) -> Self {
        Self { overseer }
    }
}
