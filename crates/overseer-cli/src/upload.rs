//! The `upload` command. It reads the modules of the build directory one
//! time. Then it uploads them to the branch of each selected profile, in
//! order. The profiles of one server share one Overseer session. The first
//! failure stops the upload. The command shuts down each session before it
//! returns.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::io::Write;
use std::path::{Path, PathBuf};

use overseer::{Modules, Overseer};

use crate::Error;
use crate::branch::{Active, Branch, BranchName};
use crate::config::{Config, ProfileName, Selected, ServerName};
use crate::connection;
use crate::envfile::EnvFile;
use crate::modules::{self, Build};

/// The arguments of `upload`.
#[derive(Debug, clap::Args)]
pub(crate) struct Upload {
    /// The build directory [default: the `dir` of the configuration, or dist
    /// next to the configuration file]
    dir: Option<PathBuf>,
    /// A profile to upload to (repeatable, or comma-separated) [default: the
    /// `default` profiles of the configuration, or its only profile]
    #[arg(
        long = "profile",
        short,
        value_name = "NAME",
        env = "OVERSEER_PROFILE",
        value_delimiter = ','
    )]
    profiles: Vec<ProfileName>,
    /// The branch for all profiles, instead of the branch of each profile.
    /// The value auto selects the current git branch
    #[arg(long, short, value_name = "BRANCH")]
    branch: Option<Branch>,
    /// Also make the branch run in the world or in the simulator
    /// (repeatable, or comma-separated)
    #[arg(long, value_enum, value_name = "WHERE", value_delimiter = ',')]
    activate: Vec<Active>,
    /// Print the plan, but upload nothing. The CLI reads the configuration,
    /// the secrets and the modules
    #[arg(long)]
    dry_run: bool,
}

impl Upload {
    /// Runs the upload with `config` and the `--env-file` variables `env`.
    /// Prints each step to `out`. Shuts down each session that the upload
    /// opened, also when the upload fails.
    pub(crate) async fn run(
        self,
        config: &Config,
        env: &EnvFile,
        out: &mut dyn Write,
    ) -> Result<(), Error> {
        let targets = config.select(&self.profiles)?;
        let dir = self.dir.as_deref().unwrap_or(&config.dir);
        let build = modules::read(dir).await?;
        print_build(dir, &build, out)?;
        let mut sessions = BTreeMap::new();
        let result = self
            .upload_all(&targets, env, &build.modules, &mut sessions, out)
            .await;
        for session in sessions.values() {
            session.shutdown().await;
        }
        result
    }

    /// Uploads `modules` to each of `targets`, in order. Connects one time to
    /// each server and keeps the sessions in `sessions`. The caller must shut
    /// down the sessions. A dry run does not connect.
    async fn upload_all<'a>(
        &self,
        targets: &[Selected<'a>],
        env: &EnvFile,
        modules: &Modules,
        sessions: &mut BTreeMap<&'a ServerName, Overseer>,
        out: &mut dyn Write,
    ) -> Result<(), Error> {
        let mut git_branch = None;
        for &target in targets {
            let branch = self
                .branch
                .as_ref()
                .unwrap_or(&target.profile.branch)
                .resolve(&mut git_branch)?;
            let activate: Vec<Active> = Active::ALL
                .into_iter()
                .filter(|active| {
                    self.activate.contains(active) || target.profile.activate.contains(active)
                })
                .collect();
            write!(
                out,
                "profile {}: branch {branch} on {}",
                target.name, target.server.url
            )?;
            for (at, active) in activate.iter().enumerate() {
                write!(
                    out,
                    "{} {active}",
                    if at == 0 { ", run in" } else { " and" }
                )?;
            }
            writeln!(out)?;
            let credentials = target
                .server
                .auth
                .resolve(env)
                .map_err(|source| Error::Secret {
                    server: target.profile.server.clone(),
                    source,
                })?;
            if self.dry_run {
                writeln!(out, "  dry run: nothing uploaded")?;
                continue;
            }
            let session = match sessions.entry(&target.profile.server) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => entry.insert(
                    connection::connect(&target.server.url, credentials)
                        .await
                        .map_err(|source| api_error(&target, source))?,
                ),
            };
            upload(session, &target, &branch, modules, &activate, out).await?;
        }
        Ok(())
    }
}

/// Prints the modules of `build` from `dir`, and the entries that the read
/// skipped.
fn print_build(dir: &Path, build: &Build, out: &mut dyn Write) -> Result<(), Error> {
    writeln!(
        out,
        "build {}: {} modules",
        dir.display(),
        build.files.len()
    )?;
    let width = build
        .files
        .iter()
        .map(|file| file.name.len())
        .max()
        .unwrap_or_default();
    for file in &build.files {
        writeln!(
            out,
            "  {:<width$}  {:<10}  {:>9} bytes  {}",
            file.name, file.kind, file.len, file.file_name
        )?;
    }
    for skipped in &build.skipped {
        writeln!(
            out,
            "  skipped {}: {}",
            file_name(&skipped.path).display(),
            skipped.reason
        )?;
    }
    Ok(())
}

/// Uploads `modules` to the branch `branch` of the profile `target` with
/// `session`. Then makes the branch run in each place of `activate`. If the
/// account does not have the branch, creates the branch with `modules`.
async fn upload(
    session: &Overseer,
    target: &Selected<'_>,
    branch: &BranchName,
    modules: &Modules,
    activate: &[Active],
    out: &mut dyn Write,
) -> Result<(), Error> {
    let failed = |source| api_error(target, source);
    let code = session.code();
    let branches = code.branches().await.map_err(failed)?;
    let info = branches.iter().find(|info| info.branch == branch.as_str());
    if info.is_some() {
        code.set_modules(branch.as_str(), modules)
            .await
            .map_err(failed)?;
    } else {
        code.create_branch(branch.as_str(), modules)
            .await
            .map_err(failed)?;
        writeln!(out, "  created branch {branch}")?;
    }
    writeln!(out, "  uploaded {} modules", modules.len())?;
    for &active in activate {
        if info.is_some_and(|info| active.runs(info)) {
            writeln!(out, "  branch {branch} already runs in {active}")?;
        } else {
            code.set_active_branch(active.into(), branch.as_str())
                .await
                .map_err(failed)?;
            writeln!(out, "  branch {branch} runs in {active}")?;
        }
    }
    Ok(())
}

/// The error of the call `source` for the profile `target`.
fn api_error(target: &Selected<'_>, source: overseer::Error) -> Error {
    Error::Api {
        profile: target.name.clone(),
        source,
    }
}

/// The last component of `path`, which is in the build directory.
fn file_name(path: &Path) -> &Path {
    path.file_name().map_or(path, Path::new)
}
