//! Power creeps area: the signed-in user's power creep records, creation, renaming, upgrades,
//! deletion and power experimentation.

use std::collections::BTreeMap;

use overseer_core::api::{community, official, openscreeps};
use overseer_core::model::power_creeps::{PowerCreep, PowerCreepClassName};

use super::{Api, Community, Official, OpenScreeps, delegate};
use crate::Error;

/// The signed-in user's power creeps. The server's refusals (for example `not enough power
/// level`, `spawned`, `no power resets`) are [`ApiError::Server`] errors.
///
/// [`ApiError::Server`]: overseer_core::api::ApiError::Server
pub trait PowerCreepsApi {
    /// Every power creep of the user (`GET /api/game/power-creeps/list`); a spawned creep
    /// carries its `shard` (and its room object's state).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn power_creeps(&self) -> impl Future<Output = Result<Vec<PowerCreep>, Error>>;

    /// Creates an unspawned level-0 power creep named `name` of `class`
    /// (`POST /api/game/power-creeps/create`); needs a free Global Power Level. Returns the new
    /// record when the server answers it (`OpenScreeps`); screeps.com and community servers
    /// only acknowledge, so list the power creeps to see it there.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn create_power_creep(
        &self,
        name: String,
        class: PowerCreepClassName,
    ) -> impl Future<Output = Result<Option<PowerCreep>, Error>>;

    /// Renames the unspawned power creep `id` (`POST /api/game/power-creeps/rename`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn rename_power_creep(
        &self,
        id: String,
        name: String,
    ) -> impl Future<Output = Result<(), Error>>;

    /// Deletes the unspawned power creep `id` (`POST /api/game/power-creeps/delete`): at once
    /// during power experimentation, otherwise after 24 hours (`deleteTime`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn delete_power_creep(&self, id: String) -> impl Future<Output = Result<(), Error>>;

    /// Cancels the scheduled deletion of power creep `id`
    /// (`POST /api/game/power-creeps/cancel-delete`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn cancel_power_creep_delete(&self, id: String) -> impl Future<Output = Result<(), Error>>;

    /// Sets the complete power levels of power creep `id`, keyed by power id (`"1"`–`"19"`,
    /// the `PWR_*` constants), via `POST /api/game/power-creeps/upgrade`.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn upgrade_power_creep(
        &self,
        id: String,
        powers: BTreeMap<String, u64>,
    ) -> impl Future<Output = Result<(), Error>>;

    /// Spends one power experimentation to start a 24-hour period in which deleting power
    /// creeps is immediate (`POST /api/game/power-creeps/experimentation`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn start_power_experimentation(&self) -> impl Future<Output = Result<(), Error>>;
}

impl PowerCreepsApi for Api {
    async fn power_creeps(&self) -> Result<Vec<PowerCreep>, Error> {
        delegate!(self, api => api.power_creeps().await)
    }

    async fn create_power_creep(
        &self,
        name: String,
        class: PowerCreepClassName,
    ) -> Result<Option<PowerCreep>, Error> {
        delegate!(self, api => api.create_power_creep(name, class).await)
    }

    async fn rename_power_creep(&self, id: String, name: String) -> Result<(), Error> {
        delegate!(self, api => api.rename_power_creep(id, name).await)
    }

    async fn delete_power_creep(&self, id: String) -> Result<(), Error> {
        delegate!(self, api => api.delete_power_creep(id).await)
    }

    async fn cancel_power_creep_delete(&self, id: String) -> Result<(), Error> {
        delegate!(self, api => api.cancel_power_creep_delete(id).await)
    }

    async fn upgrade_power_creep(
        &self,
        id: String,
        powers: BTreeMap<String, u64>,
    ) -> Result<(), Error> {
        delegate!(self, api => api.upgrade_power_creep(id, powers).await)
    }

    async fn start_power_experimentation(&self) -> Result<(), Error> {
        delegate!(self, api => api.start_power_experimentation().await)
    }
}

/// The new record a contract's creation reply holds, if any.
trait Created {
    /// The new power creep, or `None` from a mere acknowledgement.
    fn record(self) -> Option<PowerCreep>;
}

/// The official and community contracts only acknowledge (`{ok: 1}`).
impl Created for official::types::PowerCreepCreateResponse {
    fn record(self) -> Option<PowerCreep> {
        None
    }
}

/// `OpenScreeps` answers the new record.
impl Created for openscreeps::types::PowerCreepCreateResponse {
    fn record(self) -> Option<PowerCreep> {
        Some(self.into())
    }
}

/// `impl PowerCreepsApi` for the per-contract API `$api` sending the endpoints of `$contract`:
/// every contract has the same power creep endpoints and requests, only the creation replies
/// differ ([`Created`]).
macro_rules! power_creeps_api {
    ($api:ty, $contract:ident) => {
        impl PowerCreepsApi for $api {
            async fn power_creeps(&self) -> Result<Vec<PowerCreep>, Error> {
                let list = $contract::endpoints::PowerCreepsList;
                Ok(self.client.send(&list).await?.body.list)
            }

            async fn create_power_creep(
                &self,
                name: String,
                class: PowerCreepClassName,
            ) -> Result<Option<PowerCreep>, Error> {
                let body = $contract::types::PowerCreepCreateRequest {
                    class_name: class.into(),
                    name,
                };
                let create = $contract::endpoints::PowerCreepsCreate { body };
                Ok(self.client.send(&create).await?.body.record())
            }

            async fn rename_power_creep(&self, id: String, name: String) -> Result<(), Error> {
                let body = $contract::types::PowerCreepRenameRequest { id, name };
                let rename = $contract::endpoints::PowerCreepsRename { body };
                self.client.send(&rename).await?;
                Ok(())
            }

            async fn delete_power_creep(&self, id: String) -> Result<(), Error> {
                let body = $contract::types::PowerCreepIdRequest { id };
                let delete = $contract::endpoints::PowerCreepsDelete { body };
                self.client.send(&delete).await?;
                Ok(())
            }

            async fn cancel_power_creep_delete(&self, id: String) -> Result<(), Error> {
                let body = $contract::types::PowerCreepIdRequest { id };
                let cancel = $contract::endpoints::PowerCreepsCancelDelete { body };
                self.client.send(&cancel).await?;
                Ok(())
            }

            async fn upgrade_power_creep(
                &self,
                id: String,
                powers: BTreeMap<String, u64>,
            ) -> Result<(), Error> {
                let body = $contract::types::PowerCreepUpgradeRequest { id, powers };
                let upgrade = $contract::endpoints::PowerCreepsUpgrade { body };
                self.client.send(&upgrade).await?;
                Ok(())
            }

            async fn start_power_experimentation(&self) -> Result<(), Error> {
                let start = $contract::endpoints::PowerCreepsExperimentation::default();
                self.client.send(&start).await?;
                Ok(())
            }
        }
    };
}

power_creeps_api!(Official, official);
power_creeps_api!(OpenScreeps, openscreeps);
power_creeps_api!(Community, community);
