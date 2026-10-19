//! The safety checks for a placement on one shard and for an account-wide
//! reset.

use crate::heartbeat::{self, Heartbeat};
use overseer::Overseer;
use overseer::model::account::{Profile, UserWorldStatus};
use overseer::model::memory::MemoryPath;
use overseer::model::users::RoomHoldings;

use super::{Failure, label};

/// Reads the owned and reserved room lists of the signed-in account.
/// Returns `Failure::Identity` when the account has no identity. Thus the
/// CLI never reads the rooms of another user or an unknown user.
pub(super) async fn user_rooms(api: &Overseer, me: &Profile) -> Result<RoomHoldings, Failure> {
    if me.id.trim().is_empty() {
        return Err(Failure::Identity);
    }
    Ok(api.users().room_holdings(&me.id, true).await?)
}

/// Returns the reason that the room lists stop a placement on `shard`. A
/// missing entry for `shard` stops it. An owned or reserved room stops it.
/// Empty lists permit only a try: they do not prove that no creeps exist.
pub(super) fn target_rooms(rooms: &RoomHoldings, shard: &str) -> Option<String> {
    let RoomHoldings::Sharded { owned, reserved } = rooms else {
        return Some(format!(
            "{shard}: single-world evidence does not report named shards"
        ));
    };
    let Some(owned) = owned.get(shard) else {
        return Some(format!("{shard}: owned-room evidence is missing"));
    };
    let Some(reserved) = reserved.as_ref().and_then(|rooms| rooms.get(shard)) else {
        return Some(format!("{shard}: reserved-room evidence is missing"));
    };
    if !owned.is_empty() || !reserved.is_empty() {
        return Some(format!("{shard}: the account has owned or reserved rooms"));
    }
    None
}

/// Returns the reason that the room lists stop a reset. A reset can erase
/// all shards. Thus the lists must be complete and empty for each shard in
/// `all`.
pub(super) fn room_footholds(rooms: &RoomHoldings, all: &[Option<String>]) -> Option<String> {
    let RoomHoldings::Sharded { owned, reserved } = rooms else {
        return Some("single-world evidence does not report all shards".to_owned());
    };
    if let Some((shard, _)) = owned
        .iter()
        .chain(reserved.iter().flatten())
        .find(|(_, names)| !names.is_empty())
    {
        return Some(format!("{shard}: the account has owned or reserved rooms"));
    }
    all.iter()
        .filter_map(|shard| shard.as_deref())
        .find_map(|shard| target_rooms(rooms, shard))
}

/// Returns the reason that the heartbeats stop a reset. The heartbeat of
/// each shard in `lost` must be fresh and count no footholds.
pub(super) async fn footholds(
    client: &Overseer,
    lost: &[Option<String>],
) -> Result<Option<String>, Failure> {
    let path = MemoryPath::parse(heartbeat::PATH)
        .map_err(|error| overseer::Error::InvalidInput(error.to_string()))?;
    for shard in lost {
        let shard = shard.as_deref();
        let data = client.memory().get(&path, shard).await?;
        let time = client.map().game_time(shard).await?;
        let checked = Heartbeat::decode(data).and_then(|beat| beat.check(shard, time));
        if let Err(reason) = checked {
            return Ok(Some(format!("{}: {reason}", label(shard))));
        }
    }
    Ok(None)
}

/// Revalidates a reset immediately before the CLI sends it. Returns the
/// reason that stops the reset, or `None`. The checks are:
///
/// - The world status is still `lost`.
/// - The room lists are complete and empty for all shards.
/// - Each shard in `lost` has a fresh heartbeat without footholds.
pub(super) async fn recheck_lost(
    client: &Overseer,
    all: &[Option<String>],
    lost: &[Option<String>],
) -> Result<Option<String>, Failure> {
    if client.account().world_status().await? != UserWorldStatus::Lost {
        return Ok(Some("the account is no longer lost".to_owned()));
    }
    if all.iter().any(Option::is_some) {
        let me = client.account().refresh().await?;
        if let Some(blocker) = room_footholds(&user_rooms(client, &me).await?, all) {
            return Ok(Some(blocker));
        }
    }
    footholds(client, lost).await
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    /// Only complete and empty room lists permit a try on the target shard.
    #[test]
    fn placement_requires_owned_and_reserved_absence() {
        let empty = BTreeMap::from([("shard0".to_owned(), Vec::new())]);
        let occupied = BTreeMap::from([("shard0".to_owned(), vec!["W1N1".to_owned()])]);
        let rooms = RoomHoldings::Sharded {
            owned: empty.clone(),
            reserved: Some(empty.clone()),
        };
        assert!(target_rooms(&rooms, "shard0").is_none());
        assert!(target_rooms(&rooms, "shard3").is_some());
        for rooms in [
            RoomHoldings::Sharded {
                owned: empty.clone(),
                reserved: None,
            },
            RoomHoldings::Sharded {
                owned: empty.clone(),
                reserved: Some(BTreeMap::new()),
            },
            RoomHoldings::Sharded {
                owned: BTreeMap::new(),
                reserved: Some(empty.clone()),
            },
            RoomHoldings::Sharded {
                owned: occupied.clone(),
                reserved: Some(empty.clone()),
            },
            RoomHoldings::Sharded {
                owned: empty,
                reserved: Some(occupied),
            },
            RoomHoldings::SingleWorld { owned: Vec::new() },
        ] {
            assert!(target_rooms(&rooms, "shard0").is_some(), "{rooms:?}");
        }
    }

    /// A reset is account-wide. Missing shards and footholds on unlisted
    /// shards stop it.
    #[test]
    fn reset_requires_every_shard_empty() {
        let all = vec![Some("shard0".to_owned()), Some("shard3".to_owned())];
        let empty = BTreeMap::from([
            ("shard0".to_owned(), Vec::new()),
            ("shard3".to_owned(), Vec::new()),
        ]);
        let mut owned = empty.clone();
        owned.remove("shard3");
        let mut reserved = empty.clone();
        reserved.insert("unlisted".to_owned(), vec!["W1N1".to_owned()]);
        for (rooms, blocked) in [
            (
                RoomHoldings::Sharded {
                    owned: empty.clone(),
                    reserved: Some(empty.clone()),
                },
                false,
            ),
            (
                RoomHoldings::Sharded {
                    owned,
                    reserved: Some(empty.clone()),
                },
                true,
            ),
            (
                RoomHoldings::Sharded {
                    owned: empty.clone(),
                    reserved: Some(reserved),
                },
                true,
            ),
            (
                RoomHoldings::Sharded {
                    owned: empty,
                    reserved: None,
                },
                true,
            ),
            (RoomHoldings::SingleWorld { owned: Vec::new() }, true),
        ] {
            assert_eq!(room_footholds(&rooms, &all).is_some(), blocked, "{rooms:?}");
        }
    }
}
