//! `overseer`: the CLI of Overseer. It uploads built Screeps code to the
//! branches of Screeps servers. It also keeps a spawn of the account in the
//! world. Run `overseer help` for the usage.
//!
//! The configuration ([`config`]) names the servers and the profiles. A
//! profile names a server, a branch, where the branch runs, and how to place
//! its spawn. A server is a table of the configuration or a saved server
//! ([`saved`]), which the CLI shares with the desktop app. The commands:
//!
//! - `upload` reads the modules of the build directory ([`modules`]). Then it
//!   replaces the code of the branch of each selected profile through the
//!   Overseer library.
//! - `poll` ([`poll`]) places the spawn of an empty account in a room that the
//!   selector of the bot ([`selector`]) chooses. It resets an account when
//!   the heartbeat of the bot ([`heartbeat`]) shows that the account has no
//!   foothold.
//! - `profiles` lists the profiles.
//! - `servers` ([`servers`]) lists, adds and removes the saved servers.
//! - `login` and `logout` ([`login`]) save and delete the sign-in of a saved
//!   server.
//!
//! The `--env-file` option ([`envfile`]) supplies the variables of the
//! secrets.

mod branch;
mod config;
mod connection;
mod envfile;
mod heartbeat;
mod login;
mod modules;
mod poll;
mod prompt;
mod room;
mod saved;
mod selector;
mod servers;
mod table;
mod upload;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::config::{Config, ProfileName, ServerRef};
use crate::envfile::EnvFile;
use crate::saved::Saved;

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
    /// List, add or remove the saved servers.
    ///
    /// The desktop app shows the same servers. The variable
    /// `OVERSEER_APP_IDENTIFIER` selects the storage of another app
    /// identifier.
    /// This command needs no configuration file.
    Servers(servers::Servers),
    /// Sign in to a saved server and save the sign-in.
    ///
    /// The CLI reads each secret from stdin: a prompt without echo on a
    /// terminal, else one line for each secret. The CLI never saves a
    /// password. The desktop app uses the same sign-in. This command needs no
    /// configuration file.
    Login(login::Login),
    /// Delete the saved sign-in of a saved server.
    ///
    /// This command needs no configuration file.
    Logout(login::Logout),
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
        server: ServerRef,
        /// Why the secret is not available.
        source: config::SecretError,
    },
    /// The connection of a profile failed.
    #[error("profile {profile}: {source}")]
    Connect {
        /// The profile.
        profile: ProfileName,
        /// What failed.
        source: connection::Error,
    },
    /// The saved servers or secrets are not available.
    #[error(transparent)]
    Saved(#[from] saved::Error),
    /// The CLI refuses a change of the saved servers.
    #[error(transparent)]
    Servers(#[from] servers::Refusal),
    /// A secret on stdin is not available.
    #[error(transparent)]
    Prompt(#[from] prompt::Error),
    /// A sign-in or a sign-out failed.
    #[error(transparent)]
    Login(#[from] login::Failure),
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

/// Runs the command of `cli` and prints its output to `out`. The commands
/// of the saved servers need no configuration file.
async fn run(cli: Cli, out: &mut dyn Write) -> Result<(), Error> {
    let saved = Saved::new();
    let config = cli.config.as_deref();
    let env_file = cli.env_file.as_deref();
    match cli.command {
        Command::Servers(servers) => servers.run(&saved, out),
        Command::Login(login) => login.run(&saved, out).await,
        Command::Logout(logout) => logout.run(&saved, out).await,
        Command::Upload(upload) => {
            let (config, env) = load(config, env_file, &saved, out)?;
            upload.run(&config, &env, &saved, out).await
        }
        Command::Profiles => {
            let (config, _) = load(config, env_file, &saved, out)?;
            profiles(&config, out)
        }
        Command::Poll(poll) => {
            let (config, env) = load(config, env_file, &saved, out)?;
            poll.run(&config, &env, &saved, out).await
        }
    }
}

/// Reads the configuration `config` with the saved servers `saved`, and the
/// env file `env_file`. Prints their paths to `out`.
fn load(
    config: Option<&Path>,
    env_file: Option<&Path>,
    saved: &Saved,
    out: &mut dyn Write,
) -> Result<(Config, EnvFile), Error> {
    let config = Config::load(config, saved)?;
    writeln!(out, "config {}", config.path.display())?;
    let env = EnvFile::load(env_file)?;
    if let Some(path) = &env.path {
        writeln!(out, "env file {}: {} variables", path.display(), env.len())?;
    }
    Ok((config, env))
}

/// Prints the profiles of `config` as a table. A `*` marks each profile that
/// `upload` uses without `--profile`. If the table has profiles but no mark,
/// the last line says that `upload` needs `--profile`. The `AUTH` column
/// tells where the sign-in comes from: the file, a saved API token or a saved
/// session token. It shows no secret.
fn profiles(config: &Config, out: &mut dyn Write) -> Result<(), Error> {
    let mut rows = vec![
        ["PROFILE", "SERVER", "URL", "BRANCH", "ACTIVATE", "AUTH"]
            .map(String::from)
            .to_vec(),
    ];
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
        let auth = match &target.server.auth {
            config::Auth::Missing => format!("none: run `overseer login {}`", target.server.login),
            auth => auth.to_string(),
        };
        rows.push(vec![
            format!("{}{mark}", target.name),
            target.profile.server.to_string(),
            target.server.url.to_string(),
            target.profile.branch.to_string(),
            if activate.is_empty() {
                "-".to_owned()
            } else {
                activate.join(",")
            },
            auth,
        ]);
    }
    table::print(&rows, out)?;
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
    use overseer_storage::profiles::{SavedUser, Server, ServerKind};

    use super::*;
    use crate::saved::testing::TempSaved;

    /// A server for the configurations of the tests.
    const SERVER: &str = "[servers.s]\nurl = \"https://screeps.com\"\ntoken = \"t\"\n";

    /// The `profiles` output of the configuration `text` with the saved
    /// servers `saved`.
    fn listing_with(text: &str, saved: &Saved) -> String {
        let config = Config::parse(text, PathBuf::from(config::FILE_NAME), saved)
            .expect("a valid configuration");
        let mut out = Vec::new();
        profiles(&config, &mut out).expect("the listing writes");
        String::from_utf8(out).expect("UTF-8 output")
    }

    /// The `profiles` output of the configuration `text`.
    fn listing(text: &str) -> String {
        listing_with(text, &TempSaved::new("listing"))
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

    /// The `AUTH` column shows where each sign-in comes from, and no secret.
    #[test]
    fn sign_in_sources() {
        let saved = TempSaved::new("sources");
        saved
            .update(|profiles| {
                for (name, url, api_token) in [
                    ("api", "http://api:21025", Some(true)),
                    ("session", "http://session:21025", Some(false)),
                    ("fresh", "http://fresh:21025", None),
                ] {
                    let mut server =
                        Server::new(name.to_owned(), url.to_owned(), ServerKind::Private);
                    server.user = api_token.map(|api_token| SavedUser {
                        id: "u".to_owned(),
                        username: None,
                        api_token: Some(api_token),
                    });
                    profiles.add(server).expect("a new server");
                }
            })
            .expect("the storage writes");
        let text = format!(
            "{SERVER}[profiles.file]\nserver = \"s\"\n[profiles.api]\nserver = \"api\"\n\
             [profiles.session]\nserver = \"session\"\n[profiles.fresh]\nserver = \"fresh\"\n"
        );
        let table = listing_with(&text, &saved);
        let line = |profile: &str| {
            table
                .lines()
                .find(|line| line.starts_with(profile))
                .unwrap_or_else(|| panic!("{profile} in\n{table}"))
                .to_owned()
        };
        assert!(line("file").ends_with("token in the file"), "{table}");
        assert!(line("api").ends_with("saved API token"), "{table}");
        assert!(line("session").ends_with("saved session token"), "{table}");
        assert!(
            line("fresh").ends_with("none: run `overseer login fresh`"),
            "{table}"
        );
    }
}
