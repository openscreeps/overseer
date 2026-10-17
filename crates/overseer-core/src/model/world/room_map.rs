//! Reading a room's map view (`roomMap2:` channel) by object kind and by user.

use super::{Position, RoomMap};

impl RoomMap {
    /// Keys of the documented object kinds; every other key is a user id.
    pub const KINDS: [&'static str; 8] = ["w", "r", "pb", "p", "s", "c", "m", "k"];

    /// Positions listed under `key` (a kind or a user id); empty when absent.
    #[must_use]
    pub fn get(&self, key: &str) -> &[Position] {
        let positions = match key {
            "w" => self.w.as_ref(),
            "r" => self.r.as_ref(),
            "pb" => self.pb.as_ref(),
            "p" => self.p.as_ref(),
            "s" => self.s.as_ref(),
            "c" => self.c.as_ref(),
            "m" => self.m.as_ref(),
            "k" => self.k.as_ref(),
            user => self.other.get(user),
        };
        positions.map_or(&[], Vec::as_slice)
    }

    /// Constructed walls (`w`).
    #[must_use]
    pub fn walls(&self) -> &[Position] {
        self.get("w")
    }

    /// Roads (`r`).
    #[must_use]
    pub fn roads(&self) -> &[Position] {
        self.get("r")
    }

    /// Power banks (`pb`).
    #[must_use]
    pub fn power_banks(&self) -> &[Position] {
        self.get("pb")
    }

    /// Portals (`p`).
    #[must_use]
    pub fn portals(&self) -> &[Position] {
        self.get("p")
    }

    /// Sources (`s`).
    #[must_use]
    pub fn sources(&self) -> &[Position] {
        self.get("s")
    }

    /// Controllers (`c`).
    #[must_use]
    pub fn controllers(&self) -> &[Position] {
        self.get("c")
    }

    /// Minerals (`m`).
    #[must_use]
    pub fn minerals(&self) -> &[Position] {
        self.get("m")
    }

    /// Keeper lairs (`k`).
    #[must_use]
    pub fn keeper_lairs(&self) -> &[Position] {
        self.get("k")
    }

    /// Creeps and structures of each user: `(user id, positions)` for every key that is not
    /// one of [`Self::KINDS`], ordered by user id.
    pub fn users(&self) -> impl Iterator<Item = (&str, &[Position])> {
        self.other
            .iter()
            .map(|(user, positions)| (user.as_str(), positions.as_slice()))
    }

    /// Whether the view lists nothing (no view is stored for the room).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        let kinds = [
            &self.w, &self.r, &self.pb, &self.p, &self.s, &self.c, &self.m, &self.k,
        ];
        kinds.iter().all(|positions| positions.is_none()) && self.other.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::RoomMap;

    #[test]
    fn room_map_kinds_and_user_layers() {
        let view: RoomMap = serde_json::from_str(
            r#"{"w":[[10,10]],"r":[[11,10],[12,10]],"pb":[],"5a1b":[[24,25]]}"#,
        )
        .unwrap();
        assert_eq!(view.walls(), &[[10, 10]]);
        assert_eq!(view.roads().len(), 2);
        assert_eq!(view.get("5a1b"), &[[24, 25]]);
        assert_eq!(view.sources(), [] as [[u64; 2]; 0]);
        assert_eq!(
            view.users().collect::<Vec<_>>(),
            vec![("5a1b", &[[24, 25]][..])]
        );
        assert!(!view.is_empty());
        assert!(serde_json::from_str::<RoomMap>("{}").unwrap().is_empty());
    }
}
