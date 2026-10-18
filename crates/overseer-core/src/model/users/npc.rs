//! The game's non-player characters: the users that own Invaders and Source Keepers.

/// A non-player character of the game: the owner of NPC creeps and structures. Every
/// contract gives them the same user ids (`2` and `3` in the engine's `common/constants`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Npc {
    /// The Invader (user id `2`): invader creeps, Invader Cores and strongholds.
    Invader,
    /// The Source Keeper (user id `3`): Source Keeper creeps and Keeper Lairs.
    SourceKeeper,
}

impl Npc {
    /// Every NPC.
    pub const ALL: [Self; 2] = [Self::Invader, Self::SourceKeeper];

    /// The NPC with user id `id`, or `None` for a player.
    #[must_use]
    pub fn from_user_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|npc| npc.user_id() == id)
    }

    /// The user id.
    #[must_use]
    pub const fn user_id(self) -> &'static str {
        match self {
            Self::Invader => "2",
            Self::SourceKeeper => "3",
        }
    }

    /// The name that the game shows for the NPC.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Invader => "Invader",
            Self::SourceKeeper => "Source Keeper",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Npc;

    #[test]
    fn npcs_have_fixed_user_ids_and_names() {
        assert_eq!(Npc::from_user_id("2"), Some(Npc::Invader));
        assert_eq!(Npc::from_user_id("3"), Some(Npc::SourceKeeper));
        assert_eq!(Npc::from_user_id("5a1b"), None);
        assert_eq!(Npc::from_user_id(""), None);
        assert_eq!(Npc::Invader.name(), "Invader");
        assert_eq!(Npc::SourceKeeper.name(), "Source Keeper");
        for npc in Npc::ALL {
            assert_eq!(Npc::from_user_id(npc.user_id()), Some(npc));
        }
    }
}
