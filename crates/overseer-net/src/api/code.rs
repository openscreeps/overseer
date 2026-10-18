//! Code area: the signed-in user's branches and their modules.
//!
//! Every contract defines these operations identically (same paths, requests and replies), so
//! the per-contract impls differ only in the generated module they send through.

use std::borrow::Cow;

use overseer_core::api::{community, official, openscreeps};
use overseer_core::model::code::{BranchCode, CodeTarget, Modules, UserCodeBranch};

use super::{Api, Community, Official, OpenScreeps, delegate};
use crate::Error;

/// The signed-in user's code branches and modules. Wherever a `branch` names one whose modules
/// are read or written, [`CodeTarget::selector`] selects the branch active in that target.
pub trait CodeApi {
    /// Every branch, without module contents (`GET /api/user/branches`). The server creates a
    /// `default` branch for a user without any.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn branches(&self) -> impl Future<Output = Result<Vec<UserCodeBranch>, Error>>;

    /// The modules of `branch` and its resolved name (`GET /api/user/code`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send); the server answers
    /// `no code` for an unknown branch.
    fn code(&self, branch: &str) -> impl Future<Output = Result<BranchCode, Error>>;

    /// Replaces every module of the existing `branch` with `modules` (`POST /api/user/code`).
    /// Code of the world branch runs from a later tick on, and a code change is announced on the
    /// socket. The request borrows `modules`, so one set of modules can go to many branches.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send); the server rejects unknown
    /// branches (`branch does not exist`) and modules over 5 MiB in total.
    fn set_code(&self, branch: &str, modules: &Modules) -> impl Future<Output = Result<(), Error>>;

    /// Copies the modules of `source` into the branch `new_name` (`POST /api/user/clone-branch`),
    /// creating it inactive or overwriting an existing branch of that name. The server copies
    /// its default modules (`{main: ""}`) when `source` does not exist.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send); the server rejects names
    /// longer than 30 characters and a 31st branch.
    fn clone_branch(&self, source: &str, new_name: &str)
    -> impl Future<Output = Result<(), Error>>;

    /// Creates the branch `name` with `modules` (`POST /api/user/clone-branch` with
    /// `defaultModules`), inactive, or overwrites an existing branch of that name. The request
    /// borrows `modules`.
    ///
    /// # Errors
    ///
    /// As [`Self::clone_branch`].
    fn create_branch(
        &self,
        name: &str,
        modules: &Modules,
    ) -> impl Future<Output = Result<(), Error>>;

    /// Deletes `branch` (`POST /api/user/delete-branch`). The server silently keeps missing
    /// branches and branches active in the world or the simulator.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn delete_branch(&self, branch: &str) -> impl Future<Output = Result<(), Error>>;

    /// Makes `branch` the one running in `target` (`POST /api/user/set-active-branch`); the
    /// activation is announced on the socket. Activating for the world loads the branch's code
    /// into the runtime.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send); the server answers
    /// `no branch` for an unknown branch.
    fn set_active_branch(
        &self,
        target: CodeTarget,
        branch: &str,
    ) -> impl Future<Output = Result<(), Error>>;
}

impl CodeApi for Api {
    async fn branches(&self) -> Result<Vec<UserCodeBranch>, Error> {
        delegate!(self, api => api.branches().await)
    }

    async fn code(&self, branch: &str) -> Result<BranchCode, Error> {
        delegate!(self, api => api.code(branch).await)
    }

    async fn set_code(&self, branch: &str, modules: &Modules) -> Result<(), Error> {
        delegate!(self, api => api.set_code(branch, modules).await)
    }

    async fn clone_branch(&self, source: &str, new_name: &str) -> Result<(), Error> {
        delegate!(self, api => api.clone_branch(source, new_name).await)
    }

    async fn create_branch(&self, name: &str, modules: &Modules) -> Result<(), Error> {
        delegate!(self, api => api.create_branch(name, modules).await)
    }

    async fn delete_branch(&self, branch: &str) -> Result<(), Error> {
        delegate!(self, api => api.delete_branch(branch).await)
    }

    async fn set_active_branch(&self, target: CodeTarget, branch: &str) -> Result<(), Error> {
        delegate!(self, api => api.set_active_branch(target, branch).await)
    }
}

/// `impl CodeApi for $api`, sending the endpoints of the contract module `$contract`.
macro_rules! code_api {
    ($($api:ty => $contract:ident),+ $(,)?) => {$(
        impl CodeApi for $api {
            async fn branches(&self) -> Result<Vec<UserCodeBranch>, Error> {
                let branches = $contract::endpoints::UserBranches {
                    with_code: Some(false),
                };
                Ok(self.client.send(&branches).await?.body.list)
            }

            async fn code(&self, branch: &str) -> Result<BranchCode, Error> {
                let code = $contract::endpoints::UserCodeGet {
                    branch: Some(branch.to_owned()),
                };
                Ok(self.client.send(&code).await?.body.into())
            }

            async fn set_code(&self, branch: &str, modules: &Modules) -> Result<(), Error> {
                let upload = $contract::endpoints::UserCodeSet {
                    body: $contract::types::UserCodeSetRequest {
                        hash: None,
                        branch: Some(branch.into()),
                        modules: Cow::Borrowed(modules.as_map()),
                    },
                };
                self.client.send(&upload).await?;
                Ok(())
            }

            async fn clone_branch(&self, source: &str, new_name: &str) -> Result<(), Error> {
                let body = $contract::types::UserCloneBranchRequest {
                    branch: Some(source.into()),
                    default_modules: None,
                    new_name: new_name.into(),
                };
                self.client
                    .send(&$contract::endpoints::UserCloneBranch { body })
                    .await?;
                Ok(())
            }

            async fn create_branch(&self, name: &str, modules: &Modules) -> Result<(), Error> {
                let body = $contract::types::UserCloneBranchRequest {
                    branch: None,
                    default_modules: Some(Cow::Borrowed(modules.as_map())),
                    new_name: name.into(),
                };
                self.client
                    .send(&$contract::endpoints::UserCloneBranch { body })
                    .await?;
                Ok(())
            }

            async fn delete_branch(&self, branch: &str) -> Result<(), Error> {
                let body = $contract::types::UserDeleteBranchRequest {
                    branch: branch.into(),
                };
                self.client
                    .send(&$contract::endpoints::UserDeleteBranch { body })
                    .await?;
                Ok(())
            }

            async fn set_active_branch(
                &self,
                target: CodeTarget,
                branch: &str,
            ) -> Result<(), Error> {
                let active_name = match target {
                    CodeTarget::World => {
                        $contract::types::UserSetActiveBranchRequestActiveName::ActiveWorld
                    }
                    CodeTarget::Simulation => {
                        $contract::types::UserSetActiveBranchRequestActiveName::ActiveSim
                    }
                };
                let body = $contract::types::UserSetActiveBranchRequest {
                    active_name,
                    branch: branch.into(),
                };
                self.client
                    .send(&$contract::endpoints::UserSetActiveBranch { body })
                    .await?;
                Ok(())
            }
        }
    )+};
}

code_api!(Official => official, OpenScreeps => openscreeps, Community => community);
