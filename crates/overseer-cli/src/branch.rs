//! The branches of a Screeps account: the branch names, `auto` (the current
//! git branch) and where a branch runs.

use std::fmt;
use std::process::Command;
use std::str::FromStr;

use overseer::model::code::{CodeTarget, UserCodeBranch};
use serde::Deserialize;

/// The branch name that selects the current git branch.
const AUTO: &str = "auto";
/// The branch of a profile that does not set `branch`. Each Screeps account
/// starts with this branch.
const DEFAULT: &str = "default";

/// The name of a branch. It is not empty and has no white space around it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct BranchName(String);

impl BranchName {
    /// The name as the library takes it.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for BranchName {
    type Error = InvalidBranch;

    fn try_from(name: String) -> Result<Self, InvalidBranch> {
        if name.is_empty() || name.trim() != name {
            return Err(InvalidBranch(name));
        }
        Ok(Self(name))
    }
}

impl FromStr for BranchName {
    type Err = InvalidBranch;

    fn from_str(name: &str) -> Result<Self, InvalidBranch> {
        Self::try_from(name.to_owned())
    }
}

impl fmt::Display for BranchName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.0)
    }
}

/// A branch name that is empty or has white space around it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("branch {0:?}: a branch name is not empty and has no white space around it")]
pub(crate) struct InvalidBranch(String);

/// The branch of a profile or of `--branch`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub(crate) enum Branch {
    /// The current branch of the git repository of the working directory.
    Auto,
    /// The branch of this name.
    Named(BranchName),
}

impl Branch {
    /// Returns the name of the branch. For `auto`, returns the current git
    /// branch and keeps it in `current` for the next call.
    ///
    /// # Errors
    ///
    /// For `auto`, returns an error when git gives no current branch.
    pub(crate) fn resolve(&self, current: &mut Option<BranchName>) -> Result<BranchName, GitError> {
        match self {
            Self::Named(name) => Ok(name.clone()),
            Self::Auto => {
                if let Some(name) = current {
                    return Ok(name.clone());
                }
                let name = git_branch()?;
                *current = Some(name.clone());
                Ok(name)
            }
        }
    }
}

impl Default for Branch {
    fn default() -> Self {
        Self::Named(BranchName(DEFAULT.to_owned()))
    }
}

impl TryFrom<String> for Branch {
    type Error = InvalidBranch;

    fn try_from(name: String) -> Result<Self, InvalidBranch> {
        if name == AUTO {
            Ok(Self::Auto)
        } else {
            BranchName::try_from(name).map(Self::Named)
        }
    }
}

impl FromStr for Branch {
    type Err = InvalidBranch;

    fn from_str(name: &str) -> Result<Self, InvalidBranch> {
        Self::try_from(name.to_owned())
    }
}

impl fmt::Display for Branch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.pad(AUTO),
            Self::Named(name) => name.fmt(f),
        }
    }
}

/// Where a branch runs. Each account has one branch that runs in the world
/// and one that runs in the simulator.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Active {
    /// The world: the shards of the server.
    World,
    /// The simulator.
    Sim,
}

impl Active {
    /// Each place, in the order of the output.
    pub(crate) const ALL: [Self; 2] = [Self::World, Self::Sim];

    /// Whether the account marks `branch` as the branch that runs in this
    /// place ([`CodeTarget::is_active`]).
    pub(crate) fn runs(self, branch: &UserCodeBranch) -> bool {
        CodeTarget::from(self).is_active(branch)
    }
}

impl From<Active> for CodeTarget {
    fn from(active: Active) -> Self {
        match active {
            Active::World => Self::World,
            Active::Sim => Self::Simulation,
        }
    }
}

impl fmt::Display for Active {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::World => "world",
            Self::Sim => "sim",
        })
    }
}

/// Why git gives no branch for `auto`.
#[derive(Debug, thiserror::Error)]
pub(crate) enum GitError {
    /// Git does not start.
    #[error("git: {0}")]
    Spawn(#[source] std::io::Error),
    /// Git fails. The value is the error output of git.
    #[error("git: {0}")]
    Failed(String),
    /// HEAD is not a branch.
    #[error("HEAD is detached: no current git branch")]
    Detached,
    /// The branch name is not UTF-8.
    #[error("git: the branch name is not UTF-8")]
    NotUtf8,
    /// The branch name is not a valid branch name.
    #[error(transparent)]
    Invalid(#[from] InvalidBranch),
}

/// The current branch of the git repository of the working directory.
fn git_branch() -> Result<BranchName, GitError> {
    let output = Command::new("git")
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .output()
        .map_err(GitError::Spawn)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(match stderr.trim() {
            "" => GitError::Detached,
            error => GitError::Failed(error.to_owned()),
        });
    }
    let mut name = String::from_utf8(output.stdout).map_err(|_| GitError::NotUtf8)?;
    name.truncate(name.trim_end().len());
    Ok(BranchName::try_from(name)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `auto` is the git branch. Another name is a branch name if it is not
    /// empty and has no white space around it.
    #[test]
    fn branches() {
        assert_eq!("auto".parse(), Ok(Branch::Auto));
        assert_eq!(
            "feature-x"
                .parse::<Branch>()
                .map(|branch| branch.to_string()),
            Ok("feature-x".to_owned())
        );
        for invalid in ["", " main", "main\n"] {
            assert!(invalid.parse::<Branch>().is_err(), "{invalid:?}");
        }
        assert_eq!(Branch::default().to_string(), DEFAULT);
    }
}
