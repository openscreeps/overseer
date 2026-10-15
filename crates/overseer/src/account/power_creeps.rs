//! The signed-in user's power creeps.

use std::collections::BTreeMap;

use overseer_net::core::api::{endpoints, types};

use super::forget_me;
use crate::{Error, Overseer};

/// The signed-in user's power creeps: the account records behind the power creep page.
///
/// Created by [`Account::power_creeps`](crate::Account::power_creeps); cheap to clone. Every
/// operation fails with [`Error::Anonymous`] without a signed-in user, with [`Error::Closed`]
/// after [`Overseer::shutdown`] and with [`Error::Net`] when the request fails or the server
/// refuses (its error text, for example `not enough power level` or `spawned`).
#[derive(Clone, Debug)]
pub struct PowerCreeps {
    overseer: Overseer,
}

impl PowerCreeps {
    pub(crate) fn new(overseer: Overseer) -> Self {
        Self { overseer }
    }

    /// Every power creep of the user (`GET /api/game/power-creeps/list`); a spawned creep
    /// carries its `shard` (and its room object's state).
    ///
    /// # Errors
    ///
    /// See [`PowerCreeps`].
    pub async fn list(&self) -> Result<Vec<types::PowerCreep>, Error> {
        Ok(self.overseer.send(&endpoints::PowerCreepsList).await?.list)
    }

    /// Creates an unspawned level-0 power creep named `name` (`POST
    /// /api/game/power-creeps/create`); needs a free Global Power Level.
    ///
    /// # Errors
    ///
    /// See [`PowerCreeps`].
    pub async fn create(
        &self,
        name: &str,
        class: types::PowerCreepCreateRequestClassName,
    ) -> Result<types::PowerCreepCreateResponse, Error> {
        let body = types::PowerCreepCreateRequest {
            class_name: class,
            name: name.to_owned(),
        };
        self.overseer
            .send(&endpoints::PowerCreepsCreate { body })
            .await
    }

    /// Renames the unspawned power creep `id` (`POST /api/game/power-creeps/rename`).
    ///
    /// # Errors
    ///
    /// See [`PowerCreeps`].
    pub async fn rename(&self, id: &str, name: &str) -> Result<(), Error> {
        let body = types::PowerCreepRenameRequest {
            id: id.to_owned(),
            name: name.to_owned(),
        };
        self.overseer
            .send(&endpoints::PowerCreepsRename { body })
            .await?;
        Ok(())
    }

    /// Deletes the unspawned power creep `id` (`POST /api/game/power-creeps/delete`): at
    /// once during [experimentation](Self::start_experimentation), otherwise after 24 hours
    /// (`deleteTime`), until then [cancellable](Self::cancel_delete).
    ///
    /// # Errors
    ///
    /// See [`PowerCreeps`].
    pub async fn delete(&self, id: &str) -> Result<(), Error> {
        let body = types::PowerCreepIdRequest { id: id.to_owned() };
        self.overseer
            .send(&endpoints::PowerCreepsDelete { body })
            .await?;
        Ok(())
    }

    /// Cancels the scheduled deletion of power creep `id`
    /// (`POST /api/game/power-creeps/cancel-delete`).
    ///
    /// # Errors
    ///
    /// See [`PowerCreeps`].
    pub async fn cancel_delete(&self, id: &str) -> Result<(), Error> {
        let body = types::PowerCreepIdRequest { id: id.to_owned() };
        self.overseer
            .send(&endpoints::PowerCreepsCancelDelete { body })
            .await?;
        Ok(())
    }

    /// Sets the complete power levels of power creep `id`, keyed by power id (`"1"`–`"19"`,
    /// the `PWR_*` constants), via `POST /api/game/power-creeps/upgrade`. Learned powers left
    /// out count as level 0, which the server refuses as a downgrade.
    ///
    /// # Errors
    ///
    /// See [`PowerCreeps`].
    pub async fn upgrade(&self, id: &str, powers: BTreeMap<String, u64>) -> Result<(), Error> {
        let body = types::PowerCreepUpgradeRequest {
            id: id.to_owned(),
            powers,
        };
        self.overseer
            .send(&endpoints::PowerCreepsUpgrade { body })
            .await?;
        Ok(())
    }

    /// Spends one power experimentation (`powerExperimentations` of
    /// [`Account::me`](crate::Account::me)) to start a 24-hour period in which deleting power
    /// creeps is immediate (`POST /api/game/power-creeps/experimentation`). Clears the cached
    /// profile.
    ///
    /// # Errors
    ///
    /// See [`PowerCreeps`]; the server answers `no power resets` when none are left.
    pub async fn start_experimentation(&self) -> Result<(), Error> {
        self.overseer
            .send(&endpoints::PowerCreepsExperimentation::default())
            .await?;
        forget_me(&self.overseer).await;
        Ok(())
    }
}
