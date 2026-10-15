//! Code branches and modules of the signed-in user, and their sync with local directories.

use crate::Overseer;

/// Code branches and modules of the signed-in user, and their sync with local directories.
///
/// Created by [`Overseer::code`]; cheap to clone.
#[derive(Clone, Debug)]
pub struct Code {
    #[expect(dead_code, reason = "read by the code features")]
    overseer: Overseer,
}

impl Code {
    pub(crate) fn new(overseer: Overseer) -> Self {
        Self { overseer }
    }
}
