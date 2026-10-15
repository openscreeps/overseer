//! The market: orders, statistics and money history.

use crate::Overseer;

/// The market: orders, statistics and money history.
///
/// Created by [`Overseer::market`]; cheap to clone.
#[derive(Clone, Debug)]
pub struct Market {
    #[expect(dead_code, reason = "read by the market features")]
    overseer: Overseer,
}

impl Market {
    pub(crate) fn new(overseer: Overseer) -> Self {
        Self { overseer }
    }
}
