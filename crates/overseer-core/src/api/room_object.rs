//! The amount of a dropped resource ([`GameRoomObjectResource`]).

use super::types::GameRoomObjectResource;

impl GameRoomObjectResource {
    /// The amount of the dropped resource: the value stored under its resource type (the
    /// `resourceType` property, or `type` for a pile created without one); 0 when absent.
    #[must_use]
    pub fn amount(&self) -> u64 {
        let key = self
            .resource_type
            .map_or(self.r#type.as_str(), |resource| resource.as_str());
        self.other.get(key).copied().unwrap_or(0)
    }
}
