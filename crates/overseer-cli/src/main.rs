//! `overseer`: the CLI of Overseer. It uploads built Screeps code to the
//! branches of Screeps servers. It also keeps a spawn of the account in the
//! world. Run `overseer help` for the usage.
//!
//! The configuration ([`config`]) names the servers and the profiles. A
//! profile names a server, a branch, where the branch runs, and how to place
//! its spawn. The commands:
//!
//! - `upload` reads the modules of the build directory ([`modules`]). Then it
//!   replaces the code of the branch of each selected profile through the
//!   Overseer library.
//! - `poll` ([`poll`]) places the spawn of an empty account in a room that the
//!   selector of the bot ([`selector`]) chooses. It resets an account when
//!   the heartbeat of the bot ([`heartbeat`]) shows that the account has no
//!   foothold.
//! - `profiles` lists the profiles.
//!
//! The `--env-file` option ([`envfile`]) supplies the variables of the
//! secrets.

mod branch;
mod config;
mod connection;
mod envfile;
mod heartbeat;
mod modules;
mod poll;
mod room;
mod selector;
mod upload;

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::config::{Config, ProfileName, ServerName};
use crate::envfile::EnvFile;

/// Uploads built Screeps code to the branches of Screeps servers. Keeps a
/// spawn of the account in the world.
#[derive(Debug, Parser)]
#[command(name = "overseer", version)]
struct Cli {
    /// The configuration file [default: the nearest overseer.toml in the
    /// working directory or in a parent directory]
    #[arg(
        long,
        short,
        global = true,
        env = "OVERSEER_CONFIG",
        value_name = "FILE"
    )]
    config: Option<PathBuf>,
    /// A dotenv file with the variables of `{ env = "VARIABLE" }` secrets. A
    /// variable of the process environment has priority over the file
    /// [default: none]
    #[arg(long, global = true, env = "OVERSEER_ENV_FILE", value_name = "FILE")]
    env_file: Option<PathBuf>,
    /// The command.
    #[command(subcommand)]
    command: Command,
}

/// A command.
#[derive(Debug, Subcommand)]
enum Command {
    /// Upload the modules of a build directory to the branches of profiles.
    ///
    /// The CLI reads the files directly in the directory. A .js file is a
    /// module. Its module name is the file name without .js.
    ///
    /// A .js.map file is a source map module. Its module name is the whole
    /// file name. The CLI wraps a JSON map as `module.exports = MAP;`.
    ///
    /// A .wasm file is a binary module. Its module name is the file name
    /// without .wasm.
    ///
    /// The CLI skips other entries. It also skips a file whose module name is
    /// empty or starts with `$` or `.`, because the server drops this module.
    ///
    /// The upload replaces all modules of each branch. If the account does
    /// not have the branch, the upload creates it.
    Upload(upload::Upload),
    /// List the profiles of the configuration.
    Profiles,
    /// Keep a spawn in the world for the account of each profile with a spawn
    /// table.
    ///
    /// Each poll reads the account and its world status.
    ///
    /// If the account is empty, poll places a spawn. The selector of the
    /// profile chooses the room from the candidates. Before the placement,
    /// poll checks the room and the tile again.
    ///
    /// An account is lost when it has objects but no spawn. Poll resets a
    /// lost account only if the heartbeat of the bot on each lost shard is
    /// fresh and counts no foothold. This must stay true for 180 s. Poll
    /// checks it again immediately before the reset.
    ///
    /// Without --execute, poll changes nothing. It prints what it would do.
    Poll(poll::Poll),
}

/// A command that failed.
#[derive(Debug, thiserror::Error)]
enum Error {
    /// The configuration is not available or not valid.
    #[error(transparent)]
    Config(#[from] config::Error),
    /// The env file is not available or not valid.
    #[error(transparent)]
    EnvFile(#[from] envfile::Error),
    /// The CLI cannot use the build directory for an upload.
    #[error(transparent)]
    Modules(#[from] modules::Error),
    /// Git gives no current branch for `auto`.
    #[error("branch auto: {0}")]
    Git(#[from] branch::GitError),
    /// A secret of a server is not available.
    #[error("server {server}: {source}")]
    Secret {
        /// The server.
        server: ServerName,
        /// Why the secret is not available.
        source: config::SecretError,
    },
    /// A call of the API for a profile failed.
    #[error("profile {profile}: {source}")]
    Api {
        /// The profile.
        profile: ProfileName,
        /// The call that failed.
        source: overseer::Error,
    },
    /// A poll of a profile failed.
    #[error("profile {profile}: {source}")]
    Poll {
        /// The profile.
        profile: ProfileName,
        /// What failed.
        source: poll::Failure,
    },
    /// The output is not writable.
    #[error("the output: {0}")]
    Output(#[from] std::io::Error),
}

#[tokio::main(worker_threads = 1)]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli, &mut std::io::stdout().lock()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}: {error}", env!("CARGO_BIN_NAME"));
            ExitCode::FAILURE
        }
    }
}

/// Runs the command of `cli` and prints its output to `out`.
async fn run(cli: Cli, out: &mut dyn Write) -> Result<(), Error> {
    let config = Config::load(cli.config.as_deref())?;
    writeln!(out, "config {}", config.path.display())?;
    let env = EnvFile::load(cli.env_file.as_deref())?;
    if let Some(path) = &env.path {
        writeln!(out, "env file {}: {} variables", path.display(), env.len())?;
    }
    match cli.command {
        Command::Upload(upload) => upload.run(&config, &env, out).await,
        Command::Profiles => profiles(&config, out),
        Command::Poll(poll) => poll.run(&config, &env, out).await,
    }
}

/// Prints the profiles of `config` as a table. A `*` marks each profile that
/// `upload` uses without `--profile`. If the table has profiles but no mark,
/// the last line says that `upload` needs `--profile`.
fn profiles(config: &Config, out: &mut dyn Write) -> Result<(), Error> {
    let mut rows =
        vec![["PROFILE", "SERVER", "URL", "BRANCH", "ACTIVATE", "AUTH"].map(String::from)];
    let mut marked = false;
    for target in config.profiles() {
        let default = config.is_default(target.name);
        marked |= default;
        let mark = if default { " *" } else { "" };
        let activate: Vec<String> = target
            .profile
            .activate
            .iter()
            .map(ToString::to_string)
            .collect();
        rows.push([
            format!("{}{mark}", target.name),
            target.profile.server.to_string(),
            target.server.url.to_string(),
            target.profile.branch.to_string(),
            if activate.is_empty() {
                "-".to_owned()
            } else {
                activate.join(",")
            },
            target.server.auth.to_string(),
        ]);
    }
    let mut widths = [0; 6];
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    for row in &rows {
        let last = row.len() - 1;
        for (at, (cell, width)) in row.iter().zip(widths).enumerate() {
            if at == last {
                writeln!(out, "{cell}")?;
            } else {
                write!(out, "{cell:<width$}  ")?;
            }
        }
    }
    if marked {
        writeln!(out, "* the profiles of `upload` without --profile")?;
    } else if rows.len() > 1 {
        writeln!(
            out,
            "no default profile: `upload` needs --profile, or set `default`"
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server for the configurations of the tests.
    const SERVER: &str = "[servers.s]\nurl = \"https://screeps.com\"\ntoken = \"t\"\n";

    /// The `profiles` output of the configuration `text`.
    fn listing(text: &str) -> String {
        let config =
            Config::parse(text, PathBuf::from(config::FILE_NAME)).expect("a valid configuration");
        let mut out = Vec::new();
        profiles(&config, &mut out).expect("the listing writes");
        String::from_utf8(out).expect("UTF-8 output")
    }

    /// The table shows the legend of the `*` mark only with a marked profile.
    /// Without `default`, a table of two profiles says that `upload` needs
    /// `--profile` (issue #1).
    #[test]
    fn default_legend() {
        let two = format!("{SERVER}[profiles.a]\nserver = \"s\"\n[profiles.b]\nserver = \"s\"\n");
        let unmarked = listing(&two);
        assert!(!unmarked.contains('*'), "{unmarked}");
        assert!(unmarked.contains("`upload` needs --profile"), "{unmarked}");

        let with_default = listing(&format!("default = [\"b\"]\n{two}"));
        assert!(with_default.contains("b *"), "{with_default}");
        assert!(with_default.ends_with("* the profiles of `upload` without --profile\n"));

        let only = listing(&format!("{SERVER}[profiles.a]\nserver = \"s\"\n"));
        assert!(only.contains("a *"), "{only}");
        assert!(only.ends_with("* the profiles of `upload` without --profile\n"));
    }
}
