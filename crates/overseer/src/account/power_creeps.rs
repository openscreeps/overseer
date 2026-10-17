//! The signed-in user's power creeps.

use std::collections::BTreeMap;

use overseer_net::api::PowerCreepsApi as _;
use overseer_net::model::power_creeps::{PowerCreep, PowerCreepClassName};

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
    pub async fn list(&self) -> Result<Vec<PowerCreep>, Error> {
        Ok(self.overseer.user_api()?.power_creeps().await?)
    }

    /// Creates an unspawned level-0 power creep named `name` (`POST
    /// /api/game/power-creeps/create`); needs a free Global Power Level. Returns the new record
    /// when the server answers it (`OpenScreeps`); screeps.com and community servers only
    /// acknowledge, so [list](Self::list) the power creeps to see it there.
    ///
    /// # Errors
    ///
    /// See [`PowerCreeps`].
    pub async fn create(
        &self,
        name: &str,
        class: PowerCreepClassName,
    ) -> Result<Option<PowerCreep>, Error> {
        let api = self.overseer.user_api()?;
        Ok(api.create_power_creep(name.to_owned(), class).await?)
    }

    /// Renames the unspawned power creep `id` (`POST /api/game/power-creeps/rename`).
    ///
    /// # Errors
    ///
    /// See [`PowerCreeps`].
    pub async fn rename(&self, id: &str, name: &str) -> Result<(), Error> {
        let api = self.overseer.user_api()?;
        api.rename_power_creep(id.to_owned(), name.to_owned())
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
        let api = self.overseer.user_api()?;
        api.delete_power_creep(id.to_owned()).await?;
        Ok(())
    }

    /// Cancels the scheduled deletion of power creep `id`
    /// (`POST /api/game/power-creeps/cancel-delete`).
    ///
    /// # Errors
    ///
    /// See [`PowerCreeps`].
    pub async fn cancel_delete(&self, id: &str) -> Result<(), Error> {
        let api = self.overseer.user_api()?;
        api.cancel_power_creep_delete(id.to_owned()).await?;
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
        let api = self.overseer.user_api()?;
        api.upgrade_power_creep(id.to_owned(), powers).await?;
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
            .user_api()?
            .start_power_experimentation()
            .await?;
        forget_me(&self.overseer).await;
        Ok(())
    }
}
