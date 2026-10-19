//! Code branches and modules of the signed-in user, and their sync with local directories.
//!
//! A branch is a named set of [`Modules`]; one branch runs in the world and one in the
//! simulator ([`CodeTarget`]). Code directories use the official desktop client's layout (one
//! `<module>.js` or `<module>.wasm` file per module) and can also hold the source maps of a build
//! (`<module>.js.map`). [`read_dir`] and [`write_dir`] convert between a directory and
//! [`Modules`], [`module_of`] gives the module of a file name, and [`Code::pull_dir`] and
//! [`Code::push_dir`] sync a directory with a branch. Directory sync is native-only: browsers
//! (`wasm32-unknown-unknown`) have no file system, so a web app reads and writes [`Modules`]
//! itself (file pickers, downloads, browser storage). The values are those of
//! [`overseer_net::model::code`].

#[cfg(not(target_arch = "wasm32"))]
mod dir;

#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;

use overseer_net::api::CodeApi as _;
use overseer_net::core::socket::{Channel, Payload};

#[cfg(not(target_arch = "wasm32"))]
pub use dir::{module_of, read_dir, write_dir};
pub use overseer_net::model::code::{
    ActiveBranchChange, ActiveBranches, CodeChange, CodeTarget, InvalidWasm, ModuleKind, Modules,
    UserCodeBranch, UserCodeHash,
};

use crate::{Error, Feed, Overseer};

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
        Ok(self.overseer.user_api()?.branches().await?)
    }

    /// The names of the branches active in the world and in the simulator, from
    /// [`Self::branches`].
    ///
    /// # Errors
    ///
    /// As [`Self::branches`].
    pub async fn active_branches(&self) -> Result<ActiveBranches, Error> {
        Ok(self.branches().await?.into_iter().collect())
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

    /// Replaces every module of the existing `branch` with `modules` (`POST /api/user/code`).
    /// Code of the world branch runs from a later tick on, and a [`CodeChange`] is announced.
    /// The request borrows `modules`: upload the same modules to other branches or sessions
    /// without a copy.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Net`] when the request fails; the server rejects unknown branches
    /// (`branch does not exist`) and modules over 5 MiB in total.
    pub async fn set_modules(&self, branch: &str, modules: &Modules) -> Result<(), Error> {
        Ok(self.overseer.user_api()?.set_code(branch, modules).await?)
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
        Ok(self
            .overseer
            .user_api()?
            .clone_branch(source, new_name)
            .await?)
    }

    /// Creates the branch `name` with `modules` (`POST /api/user/clone-branch` with
    /// `defaultModules`), inactive, or overwrites an existing branch of that name. The request
    /// borrows `modules`.
    ///
    /// # Errors
    ///
    /// As [`Self::clone_branch`].
    pub async fn create_branch(&self, name: &str, modules: &Modules) -> Result<(), Error> {
        Ok(self
            .overseer
            .user_api()?
            .create_branch(name, modules)
            .await?)
    }

    /// Deletes `branch` (`POST /api/user/delete-branch`). The server silently keeps missing
    /// branches and branches active in the world or the simulator.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Net`] when the request fails.
    pub async fn delete_branch(&self, branch: &str) -> Result<(), Error> {
        Ok(self.overseer.user_api()?.delete_branch(branch).await?)
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
        Ok(self
            .overseer
            .user_api()?
            .set_active_branch(target, branch)
            .await?)
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
            let Payload::Code(change) = payload else {
                return None;
            };
            Some(change)
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
                Some(change)
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
    ///
    /// Native-only: browsers have no file system.
    #[cfg(not(target_arch = "wasm32"))]
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
    ///
    /// Native-only: browsers have no file system.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn push_dir(&self, branch: &str, dir: impl AsRef<Path>) -> Result<Modules, Error> {
        let api = self.overseer.user_api()?;
        let modules = read_dir(dir).await?;
        api.set_code(branch, &modules).await?;
        Ok(modules)
    }

    /// The signed-in user's id once the session is open.
    fn ready(&self) -> Result<&str, Error> {
        self.overseer.ensure_open()?;
        self.overseer.require_user()
    }

    async fn fetch(&self, branch: &str) -> Result<(String, Modules), Error> {
        let code = self.overseer.user_api()?.code(branch).await?;
        Ok((code.branch, code.modules))
    }
}
