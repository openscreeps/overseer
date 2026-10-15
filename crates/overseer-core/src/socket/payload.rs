//! Helpers on the generated payload types of channel events.

use super::types::{Position, RoomMap};

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
    use crate::socket::types::{RoomMap, UserConsole, UserCpu, UserCpuCpu, UserResources};

    #[test]
    fn cpu_is_a_number_or_the_limit_marker() {
        let used: UserCpu = serde_json::from_str(r#"{"cpu":12.4,"memory":754}"#).unwrap();
        assert_eq!(used.cpu, UserCpuCpu::Number(12.4));
        assert_eq!(used.memory, Some(754));
        let start: UserCpu = serde_json::from_str(r#"{"cpu":0}"#).unwrap();
        assert_eq!(start.cpu, UserCpuCpu::Number(0.0));
        let limit: UserCpu = serde_json::from_str(r#"{"cpu":"error"}"#).unwrap();
        assert_eq!(limit.cpu, UserCpuCpu::Error);
        assert!(serde_json::from_str::<UserCpu>(r#"{"cpu":"fail"}"#).is_err());
        assert_eq!(serde_json::to_string(&limit).unwrap(), r#"{"cpu":"error"}"#);
    }

    #[test]
    fn console_output_and_errors_with_and_without_shard() {
        let output: UserConsole =
            serde_json::from_str(r#"{"messages":{"log":["tick"],"results":[]},"shard":"shard3"}"#)
                .unwrap();
        let UserConsole::Output(output) = output else {
            panic!("not output: {output:?}");
        };
        assert_eq!(output.messages.log, ["tick"]);
        assert_eq!(output.messages.results, [] as [String; 0]);
        assert_eq!(output.shard.as_deref(), Some("shard3"));
        let error: UserConsole = serde_json::from_str(r#"{"error":"Error: boom"}"#).unwrap();
        let UserConsole::Error(error) = error else {
            panic!("not an error: {error:?}");
        };
        assert_eq!((error.error.as_str(), error.shard), ("Error: boom", None));
        assert!(serde_json::from_str::<UserConsole>(r#"{"shard":"shard3"}"#).is_err());
    }

    #[test]
    fn resources_split_credits_from_the_rest() {
        let resources: UserResources =
            serde_json::from_str(r#"{"pixel":120,"credits":15000.5}"#).unwrap();
        assert!((resources.credits - 15_000.5).abs() < f64::EPSILON);
        assert_eq!(resources.other.len(), 1);
        assert!((resources.other["pixel"] - 120.0).abs() < f64::EPSILON);
        assert!(serde_json::from_str::<UserResources>(r#"{"pixel":1}"#).is_err());
    }

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
