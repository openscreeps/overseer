//! Code branches and modules of the signed-in user, and their sync with local directories.
//!
//! A branch is a named set of [`Modules`]; one branch runs in the world and one in the
//! simulator ([`CodeTarget`]). Code directories use the official desktop client's layout (one
//! `<module>.js` or `<module>.wasm` file per module): [`read_dir`] and [`write_dir`] convert
//! between a directory and [`Modules`], [`Code::pull_dir`] and [`Code::push_dir`] sync a
//! directory with a branch.

mod dir;
mod modules;

use std::path::Path;

use overseer_net::core::api::endpoints;
use overseer_net::core::api::types::{
    UserCloneBranchRequest, UserCodeBranch, UserCodeSetRequest, UserDeleteBranchRequest,
    UserSetActiveBranchRequest, UserSetActiveBranchRequestActiveName,
};
use overseer_net::core::socket::types::{UserActiveBranchActiveName, UserCodeHash};
use overseer_net::core::socket::{Channel, Payload};

pub use dir::{read_dir, write_dir};
pub use modules::{InvalidWasm, ModuleKind, Modules};

use crate::{Error, Feed, Overseer};

/// Where a branch runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CodeTarget {
    /// The world: the branch the player's creeps run.
    World,
    /// The simulator.
    Simulation,
}

impl CodeTarget {
    /// The branch name selecting the branch active in this target (`$activeWorld` or
    /// `$activeSim`), accepted wherever a method of [`Code`] reads or writes the modules of a
    /// branch.
    #[must_use]
    pub const fn selector(self) -> &'static str {
        match self {
            Self::World => "$activeWorld",
            Self::Simulation => "$activeSim",
        }
    }
}

/// The names of the branches active in the world and in the simulator.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActiveBranches {
    /// The branch running in the world; `None` when no branch is marked.
    pub world: Option<String>,
    /// The branch running in the simulator; `None` when no branch is marked.
    pub simulation: Option<String>,
}

impl ActiveBranches {
    /// The branch active in `target`.
    #[must_use]
    pub fn get(&self, target: CodeTarget) -> Option<&str> {
        match target {
            CodeTarget::World => self.world.as_deref(),
            CodeTarget::Simulation => self.simulation.as_deref(),
        }
    }
}

/// The saved modules of a branch, as announced on `user:<user>/code`.
#[derive(Clone, Debug, PartialEq)]
pub struct CodeChange {
    /// The branch name.
    pub branch: String,
    /// Every module of the branch after the change.
    pub modules: Modules,
    /// The `_hash` the uploading client sent, if any (the official web client sends a random
    /// number to recognize its own uploads).
    pub hash: Option<UserCodeHash>,
    /// Save time in milliseconds since the Unix epoch.
    pub timestamp: Option<u64>,
}

/// A branch was activated, as announced on `user:<user>/set-active-branch`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveBranchChange {
    /// Where the branch now runs.
    pub target: CodeTarget,
    /// The branch name.
    pub branch: String,
}

/// Code branches and modules of the signed-in user, and their sync with local directories.
///
/// Created by [`Overseer::code`]; cheap to clone. Every method needs a signed-in user and fails
/// with [`Error::Anonymous`] otherwise, and with [`Error::Closed`] after
/// [`Overseer::shutdown`]. Methods taking a `branch` to read or write modules also accept
/// [`CodeTarget::selector`] for the active branch.
#[derive(Clone, Debug)]
pub struct Code {
    overseer: Overseer,
}

impl Code {
    pub(crate) fn new(overseer: Overseer) -> Self {
        Self { overseer }
    }

    /// Every branch, without module contents (`GET /api/user/branches`). The server creates
    /// a `default` branch for a user without any.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Net`] when the request fails.
    pub async fn branches(&self) -> Result<Vec<UserCodeBranch>, Error> {
        self.ready()?;
        Ok(self.overseer.client().branches().await?)
    }

    /// The names of the branches active in the world and in the simulator, from
    /// [`Self::branches`].
    ///
    /// # Errors
    ///
    /// As [`Self::branches`].
    pub async fn active_branches(&self) -> Result<ActiveBranches, Error> {
        let mut active = ActiveBranches::default();
        for branch in self.branches().await? {
            if branch.active_world == Some(true) {
                active.world = Some(branch.branch.clone());
            }
            if branch.active_sim == Some(true) {
                active.simulation = Some(branch.branch);
            }
        }
        Ok(active)
    }

    /// The modules of `branch` (`GET /api/user/code`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Net`] when the request fails; the server answers `no code` for an
    /// unknown branch.
    pub async fn modules(&self, branch: &str) -> Result<Modules, Error> {
        Ok(self.fetch(branch).await?.1)
    }

    /// The name and modules of the branch active in `target` (`GET /api/user/code`).
    ///
    /// # Errors
    ///
    /// As [`Self::modules`].
    pub async fn active_modules(&self, target: CodeTarget) -> Result<(String, Modules), Error> {
        self.fetch(target.selector()).await
    }

    /// Replaces every module of the existing `branch` (`POST /api/user/code`). Code of the
    /// world branch runs from a later tick on, and a [`CodeChange`] is announced.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Net`] when the request fails; the server rejects unknown branches
    /// (`branch does not exist`) and modules over 5 MiB in total.
    pub async fn set_modules(&self, branch: &str, modules: Modules) -> Result<(), Error> {
        self.upload(branch, modules).await?;
        Ok(())
    }

    /// Copies the modules of `source` into the branch `new_name` (`POST /api/user/clone-branch`),
    /// creating it inactive or overwriting an existing branch of that name. The server copies
    /// its default modules (`{main: ""}`) when `source` does not exist.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Net`] when the request fails; the server rejects names longer than 30
    /// characters and a 31st branch.
    pub async fn clone_branch(&self, source: &str, new_name: &str) -> Result<(), Error> {
        self.clone_request(UserCloneBranchRequest {
            branch: Some(source.to_owned()),
            default_modules: None,
            new_name: new_name.to_owned(),
        })
        .await
    }

    /// Creates the branch `name` with `modules` (`POST /api/user/clone-branch` with
    /// `defaultModules`), inactive, or overwrites an existing branch of that name.
    ///
    /// # Errors
    ///
    /// As [`Self::clone_branch`].
    pub async fn create_branch(&self, name: &str, modules: Modules) -> Result<(), Error> {
        self.clone_request(UserCloneBranchRequest {
            branch: None,
            default_modules: Some(modules.into()),
            new_name: name.to_owned(),
        })
        .await
    }

    /// Deletes `branch` (`POST /api/user/delete-branch`). The server silently keeps missing
    /// branches and branches active in the world or the simulator.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Net`] when the request fails.
    pub async fn delete_branch(&self, branch: &str) -> Result<(), Error> {
        let body = UserDeleteBranchRequest {
            branch: branch.to_owned(),
        };
        self.overseer
            .send(&endpoints::UserDeleteBranch { body })
            .await?;
        Ok(())
    }

    /// Makes `branch` the one running in `target` (`POST /api/user/set-active-branch`); an
    /// [`ActiveBranchChange`] is announced. Activating for the world loads the branch's code
    /// into the runtime.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Net`] when the request fails; the server answers `no branch` for an
    /// unknown branch.
    pub async fn set_active_branch(&self, target: CodeTarget, branch: &str) -> Result<(), Error> {
        let active_name = match target {
            CodeTarget::World => UserSetActiveBranchRequestActiveName::ActiveWorld,
            CodeTarget::Simulation => UserSetActiveBranchRequestActiveName::ActiveSim,
        };
        let body = UserSetActiveBranchRequest {
            active_name,
            branch: branch.to_owned(),
        };
        self.overseer
            .send(&endpoints::UserSetActiveBranch { body })
            .await?;
        Ok(())
    }

    /// Every code change from now on: uploads and clones of any branch, by any client.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`] without a signed-in user and [`Error::Closed`] after
    /// [`Overseer::shutdown`].
    pub fn changes(&self) -> Result<Feed<CodeChange>, Error> {
        let user = self.ready()?.to_owned();
        Ok(self.overseer.feed(Channel::Code { user }, |payload| {
            let Payload::Code(code) = payload else {
                return None;
            };
            Some(CodeChange {
                branch: code.branch,
                modules: Modules::from_event(code.modules),
                hash: code.hash,
                timestamp: code.timestamp,
            })
        }))
    }

    /// Every branch activation from now on, by any client.
    ///
    /// # Errors
    ///
    /// As [`Self::changes`].
    pub fn active_branch_changes(&self) -> Result<Feed<ActiveBranchChange>, Error> {
        let user = self.ready()?.to_owned();
        Ok(self
            .overseer
            .feed(Channel::ActiveBranch { user }, |payload| {
                let Payload::ActiveBranch(change) = payload else {
                    return None;
                };
                let target = match change.active_name {
                    UserActiveBranchActiveName::ActiveWorld => CodeTarget::World,
                    UserActiveBranchActiveName::ActiveSim => CodeTarget::Simulation,
                };
                Some(ActiveBranchChange {
                    target,
                    branch: change.branch,
                })
            }))
    }

    /// Fetches the modules of `branch` and [writes](write_dir) them into the directory `dir`:
    /// stale `.js`/`.wasm` module files are deleted, other files are kept. Returns the
    /// modules written.
    ///
    /// # Errors
    ///
    /// The errors of [`Self::modules`] and [`write_dir`]; the directory is untouched when the
    /// branch cannot be fetched or holds a module [`write_dir`] rejects.
    pub async fn pull_dir(&self, branch: &str, dir: impl AsRef<Path>) -> Result<Modules, Error> {
        let modules = self.modules(branch).await?;
        write_dir(dir, &modules).await?;
        Ok(modules)
    }

    /// [Reads](read_dir) the modules of the directory `dir` and replaces every module of
    /// `branch` with them ([`Self::set_modules`]). Returns the modules sent.
    ///
    /// # Errors
    ///
    /// The errors of [`read_dir`] and [`Self::set_modules`].
    pub async fn push_dir(&self, branch: &str, dir: impl AsRef<Path>) -> Result<Modules, Error> {
        self.ready()?;
        let modules = read_dir(dir).await?;
        self.upload(branch, modules).await
    }

    /// The signed-in user's id once the session is open.
    fn ready(&self) -> Result<&str, Error> {
        self.overseer.ensure_open()?;
        self.overseer.require_user()
    }

    async fn fetch(&self, branch: &str) -> Result<(String, Modules), Error> {
        self.ready()?;
        let code = self.overseer.client().code(Some(branch)).await?;
        Ok((code.branch, code.modules.into()))
    }

    /// Sends `modules` to `branch` and hands them back.
    async fn upload(&self, branch: &str, modules: Modules) -> Result<Modules, Error> {
        let upload = endpoints::UserCodeSet {
            body: UserCodeSetRequest {
                hash: None,
                branch: Some(branch.to_owned()),
                modules: modules.into(),
            },
        };
        self.overseer.send(&upload).await?;
        Ok(upload.body.modules.into())
    }

    async fn clone_request(&self, body: UserCloneBranchRequest) -> Result<(), Error> {
        self.overseer
            .send(&endpoints::UserCloneBranch { body })
            .await?;
        Ok(())
    }
}
