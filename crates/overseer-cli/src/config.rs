//! The configuration file `overseer.toml`. It names the build directory, the
//! servers and the profiles.
//!
//! ```toml
//! dir = "dist"         # the build directory, relative to this file (default: dist)
//! default = ["main"]   # the profiles that `upload` uses without --profile
//!
//! [servers.official]
//! url = "https://screeps.com"
//! token = { env = "SCREEPS_TOKEN" }
//!
//! [servers.season]
//! url = "https://screeps.com/season"
//! token = { env = "SCREEPS_TOKEN" }
//!
//! [servers.local]
//! url = "http://127.0.0.1:21025"
//! email = "me@example.com"
//! password = "secret"
//!
//! [profiles.main]
//! server = "official"
//! branch = "main"
//! activate = ["world"]
//!
//! [profiles.sim]
//! server = "official"
//! branch = "sim"
//! activate = ["sim"]
//!
//! [profiles.dev]
//! server = "local"
//! branch = "auto"
//! ```
//!
//! A server has these keys:
//!
//! - `url`: the URL of the API (`http` or `https`).
//! - `token`: an API token of the account.
//! - `email` and `password`: the sign-in of an account on a private server
//!   with screepsmod-auth. Use them instead of `token`.
//!
//! A server without `token`, `email` and `password` uses the saved sign-in of
//! its URL ([`Saved`]). The credentials of the file win over a saved sign-in.
//!
//! A secret is a string or a `{ env = "VARIABLE" }` table. The CLI reads the
//! variable from the process environment, or else from the `--env-file`
//! ([`EnvFile`](crate::envfile::EnvFile)). The CLI reads a secret only when a
//! command needs it.
//!
//! A profile has these keys:
//!
//! - `server`: the name of a `[servers.NAME]` table. If the file has no such
//!   table, the name or the URL of a saved server (`overseer servers`).
//! - `branch`: the branch (default: `default`). `auto` is the current git
//!   branch.
//! - `activate`: where the upload makes the branch run (`world`, `sim`).
//!
//! A key that the format does not have is an error.
//!
//! `poll` keeps a spawn for each profile with a spawn table:
//!
//! ```toml
//! [profiles.main.spawn]
//! selector = ["node", "scripts/select-spawn.mjs"]  # program and arguments, no shell; runs in the directory of this file
//! shard = "auto"      # auto (the only shard with CPU), a shard name, or no key on a server without shards
//! interval = 60       # seconds between two polls (60..=86400)
//! radius = 5          # rooms around the start room that the candidates come from (1..=10)
//! candidates = 16     # the maximum number of candidates for the selector (1..=64)
//! ```
//!
//! The submodules:
//!
//! - [`name`] checks the names.
//! - [`secret`] holds and resolves the secrets.
//! - [`server`] reads the URLs and the sign-in of the servers.
//! - [`resolve`] finds the server of each profile.
//! - [`spawn`] reads the spawn tables.
//!
//! This module connects the profiles to their servers and selects profiles.

mod name;
mod resolve;
mod secret;
mod server;
mod spawn;

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use overseer_storage::profiles::{Ambiguous, Profiles};
use serde::Deserialize;

pub(crate) use name::{ProfileName, ServerRef};
pub(crate) use secret::{SecretError, Sensitive};
pub(crate) use server::{Auth, Credentials, Server};
pub(crate) use spawn::{ShardChoice, Spawn};

use crate::branch::{Active, Branch};
use crate::saved::{self, Saved};
use name::ServerName;
use server::FileServer;

/// The name of the configuration file. The CLI looks for it in the working
/// directory and its parents.
pub(crate) const FILE_NAME: &str = "overseer.toml";
/// The build directory if the configuration does not set `dir`.
const DEFAULT_DIR: &str = "dist";

/// A profile: a server, a branch, where the branch runs after an upload, and
/// how `poll` keeps a spawn in the world.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Profile {
    /// The server.
    pub(crate) server: ServerRef,
    /// The branch.
    #[serde(default)]
    pub(crate) branch: Branch,
    /// Where the upload makes the branch run.
    #[serde(default)]
    pub(crate) activate: Vec<Active>,
    /// How `poll` places the spawn of the account. `poll` skips a profile
    /// without a spawn table.
    pub(crate) spawn: Option<Spawn>,
}

/// The configuration in the form of the file.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    dir: Option<PathBuf>,
    #[serde(default)]
    default: Vec<ProfileName>,
    #[serde(default)]
    servers: BTreeMap<ServerName, FileServer>,
    #[serde(default)]
    profiles: BTreeMap<ProfileName, Profile>,
}

/// A valid configuration. Each profile has a server of the file or a saved
/// server.
#[derive(Debug)]
pub(crate) struct Config {
    /// The file.
    pub(crate) path: PathBuf,
    /// The build directory, relative to the file.
    pub(crate) dir: PathBuf,
    /// The server of each profile, by the `server` of the profile.
    servers: BTreeMap<ServerRef, Server>,
    profiles: BTreeMap<ProfileName, Profile>,
    default: Vec<ProfileName>,
}

/// A profile of a configuration with its name and its server.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Selected<'a> {
    /// The name of the profile.
    pub(crate) name: &'a ProfileName,
    /// The profile.
    pub(crate) profile: &'a Profile,
    /// The server of the profile.
    pub(crate) server: &'a Server,
}

impl Config {
    /// Reads the configuration from the file `path`. Without `path`, reads
    /// the nearest [`FILE_NAME`] in the working directory or its parents.
    /// `saved` gives the saved servers.
    ///
    /// # Errors
    ///
    /// Returns an error when the CLI finds no file, or when the file is not
    /// readable or not valid.
    pub(crate) fn load(path: Option<&Path>, saved: &Saved) -> Result<Self, Error> {
        let path = if let Some(path) = path {
            path.to_owned()
        } else {
            let dir = std::env::current_dir().map_err(Error::WorkingDir)?;
            let found = dir
                .ancestors()
                .map(|dir| dir.join(FILE_NAME))
                .find(|path| path.is_file());
            found.ok_or(Error::NotFound { dir })?
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text, path, saved),
            Err(source) => Err(Error::Read { path, source }),
        }
    }

    /// Parses the text `text` of the file `path` as a configuration. Finds
    /// the server of each profile in the file, or else in `saved`.
    ///
    /// # Errors
    ///
    /// Returns an error when the text is not a valid configuration, when a
    /// profile names no server, or when the saved servers are not available.
    pub(crate) fn parse(text: &str, path: PathBuf, saved: &Saved) -> Result<Self, Error> {
        let file: File = match toml::from_str(text) {
            Ok(file) => file,
            Err(source) => return Err(Error::Parse { path, source }),
        };
        for name in &file.default {
            if !file.profiles.contains_key(name) {
                return Err(Error::UnknownDefault {
                    path,
                    profile: name.clone(),
                    profiles: file.profiles.keys().cloned().collect(),
                });
            }
        }
        let profiles = if file.profiles.is_empty() {
            Profiles::default()
        } else {
            match saved.load() {
                Ok(profiles) => profiles,
                Err(source) => return Err(Error::Saved { path, source }),
            }
        };
        let mut servers = BTreeMap::new();
        for (name, profile) in &file.profiles {
            if !servers.contains_key(&profile.server) {
                let server = resolve::server(&file.servers, &profiles, &path, name, profile)?;
                servers.insert(profile.server.clone(), server);
            }
        }
        let dir = path
            .parent()
            .unwrap_or(Path::new(""))
            .join(file.dir.as_deref().unwrap_or(Path::new(DEFAULT_DIR)));
        Ok(Self {
            path,
            dir,
            servers,
            profiles: file.profiles,
            default: file.default,
        })
    }

    /// Selects the profiles `names`, in order and without repeats. Without
    /// names, selects the `default` profiles, or else the only profile.
    ///
    /// # Errors
    ///
    /// Returns an error when a name is not a profile. Also returns an error
    /// when `names` is empty, the configuration has no `default`, and it does
    /// not have exactly one profile.
    pub(crate) fn select(&self, names: &[ProfileName]) -> Result<Vec<Selected<'_>>, Error> {
        let names = match (names, self.default.as_slice()) {
            ([], []) => {
                let mut all = self.profiles();
                return match (all.next(), all.next()) {
                    (Some(only), None) => Ok(vec![only]),
                    (None, _) => Err(Error::NoProfile {
                        path: self.path.clone(),
                    }),
                    (Some(_), Some(_)) => Err(Error::Ambiguous {
                        path: self.path.clone(),
                        profiles: self.profiles.keys().cloned().collect(),
                    }),
                };
            }
            ([], default) => default,
            (names, _) => names,
        };
        let mut selected: Vec<Selected<'_>> = Vec::with_capacity(names.len());
        for name in names {
            if selected.iter().all(|other| other.name != name) {
                selected.push(self.profile(name)?);
            }
        }
        Ok(selected)
    }

    /// Each profile, in the order of the names.
    pub(crate) fn profiles(&self) -> impl Iterator<Item = Selected<'_>> {
        self.profiles.iter().filter_map(|(name, profile)| {
            let server = self.servers.get(&profile.server)?;
            Some(Selected {
                name,
                profile,
                server,
            })
        })
    }

    /// Whether `upload` without `--profile` uploads to the profile `name`.
    pub(crate) fn is_default(&self, name: &ProfileName) -> bool {
        if self.default.is_empty() {
            self.profiles.len() == 1 && self.profiles.contains_key(name)
        } else {
            self.default.contains(name)
        }
    }

    /// Selects the profiles of `poll`: the profiles `names`, in order and
    /// without repeats. Each of them must have a spawn table. Without names,
    /// selects each profile with a spawn table. Two profiles of one account
    /// (the same [`Server::identity`]) must name two different shards.
    ///
    /// # Errors
    ///
    /// Returns an error when a name is not a profile or has no spawn table.
    /// Also returns an error when no profile has a spawn table, or when two
    /// profiles of one server do not name two different shards.
    pub(crate) fn select_spawn(&self, names: &[ProfileName]) -> Result<Vec<Selected<'_>>, Error> {
        let selected: Vec<Selected<'_>> = if names.is_empty() {
            self.profiles()
                .filter(|target| target.profile.spawn.is_some())
                .collect()
        } else {
            let mut selected: Vec<Selected<'_>> = Vec::with_capacity(names.len());
            for name in names {
                if selected.iter().all(|other| other.name != name) {
                    let target = self.profile(name)?;
                    if target.profile.spawn.is_none() {
                        return Err(Error::NoSpawn {
                            path: self.path.clone(),
                            profile: name.clone(),
                        });
                    }
                    selected.push(target);
                }
            }
            selected
        };
        if selected.is_empty() {
            return Err(Error::NoSpawnProfile {
                path: self.path.clone(),
            });
        }
        for (at, first) in selected.iter().enumerate() {
            if let Some(second) = selected[at + 1..].iter().find(|other| {
                other.server.identity == first.server.identity
                    && !matches!(
                        (
                            first.profile.spawn.as_ref().and_then(|spawn| spawn.shard.as_ref()),
                            other.profile.spawn.as_ref().and_then(|spawn| spawn.shard.as_ref()),
                        ),
                        (Some(ShardChoice::Named(a)), Some(ShardChoice::Named(b))) if a != b
                    )
            }) {
                return Err(Error::SharedServer {
                    path: self.path.clone(),
                    first: first.name.clone(),
                    second: second.name.clone(),
                    server: first.profile.server.clone(),
                });
            }
        }
        Ok(selected)
    }

    /// The directory of the file. The selector runs in this directory.
    pub(crate) fn root(&self) -> &Path {
        match self.path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir,
            _ => Path::new("."),
        }
    }

    /// The profile `name`.
    fn profile(&self, name: &ProfileName) -> Result<Selected<'_>, Error> {
        let unknown = || Error::UnknownProfile {
            path: self.path.clone(),
            profile: name.clone(),
            profiles: self.profiles.keys().cloned().collect(),
        };
        let (name, profile) = self.profiles.get_key_value(name).ok_or_else(unknown)?;
        let server = self.servers.get(&profile.server).ok_or_else(unknown)?;
        Ok(Selected {
            name,
            profile,
            server,
        })
    }
}

/// `names`, separated by commas, or `none` when there are no names.
fn list<T: fmt::Display>(names: &[T]) -> String {
    let names: Vec<String> = names.iter().map(ToString::to_string).collect();
    if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(", ")
    }
}

/// A configuration that is not available or not valid.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// The working directory is not available.
    #[error("the working directory: {0}")]
    WorkingDir(#[source] std::io::Error),
    /// No configuration file in the working directory or its parents.
    #[error(
        "no {FILE_NAME} in {} or its parents; create one or pass --config FILE",
        .dir.display()
    )]
    NotFound {
        /// The working directory.
        dir: PathBuf,
    },
    /// The file is not readable.
    #[error("{}: {source}", .path.display())]
    Read {
        /// The file.
        path: PathBuf,
        /// The read error.
        source: std::io::Error,
    },
    /// The file is not a valid configuration.
    #[error("{}: {source}", .path.display())]
    Parse {
        /// The file.
        path: PathBuf,
        /// The location and the cause.
        source: toml::de::Error,
    },
    /// A profile names a server that neither the file nor the saved servers
    /// have.
    #[error(
        "{}: profile {profile}: no server {server}; servers: {}; saved servers: {}",
        .path.display(),
        list(.servers),
        list(.saved)
    )]
    UnknownServer {
        /// The file.
        path: PathBuf,
        /// The profile.
        profile: ProfileName,
        /// The server that it names.
        server: ServerRef,
        /// The servers of the file.
        servers: Box<[ServerName]>,
        /// The names of the saved servers.
        saved: Box<[String]>,
    },
    /// A profile names a saved server, and two saved servers have that name.
    #[error("{}: profile {profile}: {source}", .path.display())]
    AmbiguousServer {
        /// The file.
        path: PathBuf,
        /// The profile.
        profile: ProfileName,
        /// The saved servers with the name.
        source: Ambiguous,
    },
    /// The URL of a saved server is not a server URL.
    #[error("{}: profile {profile}: the saved server URL: {source}", .path.display())]
    SavedUrl {
        /// The file.
        path: PathBuf,
        /// The profile.
        profile: ProfileName,
        /// Why the URL is not valid.
        source: overseer::core::ServerUrlError,
    },
    /// The saved servers are not available.
    #[error("{}: {source}", .path.display())]
    Saved {
        /// The file.
        path: PathBuf,
        /// Why the saved servers are not available.
        source: saved::Error,
    },
    /// `default` names a profile that the file does not have.
    #[error(
        "{}: default: no profile {profile}; profiles: {}",
        .path.display(),
        list(.profiles)
    )]
    UnknownDefault {
        /// The file.
        path: PathBuf,
        /// The profile that `default` names.
        profile: ProfileName,
        /// The profiles of the file.
        profiles: Vec<ProfileName>,
    },
    /// A selected profile that the file does not have.
    #[error("{}: no profile {profile}; profiles: {}", .path.display(), list(.profiles))]
    UnknownProfile {
        /// The file.
        path: PathBuf,
        /// The profile.
        profile: ProfileName,
        /// The profiles of the file.
        profiles: Vec<ProfileName>,
    },
    /// A profile of `poll` has no spawn table.
    #[error("{}: profile {profile}: no [profiles.{profile}.spawn] table", .path.display())]
    NoSpawn {
        /// The file.
        path: PathBuf,
        /// The profile.
        profile: ProfileName,
    },
    /// `poll` has no `--profile`, and no profile has a spawn table.
    #[error("{}: no profile has a [profiles.NAME.spawn] table", .path.display())]
    NoSpawnProfile {
        /// The file.
        path: PathBuf,
    },
    /// Two profiles of one account do not name two different shards.
    #[error(
        "{}: profiles {first} and {second} share server {server}: use distinct explicit shard targets",
        .path.display()
    )]
    SharedServer {
        /// The file.
        path: PathBuf,
        /// The first profile.
        first: ProfileName,
        /// The second profile.
        second: ProfileName,
        /// The server of the first profile.
        server: ServerRef,
    },
    /// The file has no profile.
    #[error("{}: no profile; add a [profiles.NAME] table", .path.display())]
    NoProfile {
        /// The file.
        path: PathBuf,
    },
    /// `select` has no names, and the file has no `default` and more than one
    /// profile.
    #[error(
        "{}: profiles {}: choose with --profile, or set `default`",
        .path.display(),
        list(.profiles)
    )]
    Ambiguous {
        /// The file.
        path: PathBuf,
        /// The profiles of the file.
        profiles: Vec<ProfileName>,
    },
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::secret::Secret;
    use super::server::Auth;
    use super::*;
    use crate::saved::testing::TempSaved;

    /// A server and one profile without a spawn table.
    const SERVER_ONLY_PROFILE: &str =
        "[servers.s]\nurl = \"https://screeps.com\"\ntoken = \"t\"\n[profiles.p]\nserver = \"s\"\n";

    /// The configuration of the module documentation, with `default`.
    const EXAMPLE: &str = r#"
        default = ["main"]

        [servers.official]
        url = "https://screeps.com"
        token = { env = "SCREEPS_TOKEN" }

        [servers.season]
        url = "https://screeps.com/season"
        token = "t"

        [servers.local]
        url = "http://127.0.0.1:21025"
        email = "me@example.com"
        password = "p"

        [profiles.main]
        server = "official"
        branch = "main"
        activate = ["world"]

        [profiles.sim]
        server = "official"
        branch = "sim"
        activate = ["sim"]

        [profiles.season]
        server = "season"

        [profiles.dev]
        server = "local"
        branch = "auto"
    "#;

    /// Parses `text` as the file `project/overseer.toml`. The text must be a
    /// valid configuration.
    fn parsed(text: &str) -> Config {
        parse(text, PathBuf::from("project").join(FILE_NAME)).expect("a valid configuration")
    }

    /// Parses `text` as the file `path`, with an empty storage of saved
    /// servers.
    fn parse(text: &str, path: PathBuf) -> Result<Config, Error> {
        Config::parse(text, path, &TempSaved::new("config"))
    }

    /// The names of `selected`.
    fn names(selected: &[Selected<'_>]) -> Vec<String> {
        selected.iter().map(|s| s.name.to_string()).collect()
    }

    /// Profile names.
    fn profiles(names: &[&str]) -> Vec<ProfileName> {
        names
            .iter()
            .map(|name| name.parse().expect("a valid name"))
            .collect()
    }

    /// Servers, secrets, branches and the build directory resolve. Server
    /// URLs end in `/`, so that API paths join below them.
    #[test]
    fn example() {
        let config = parsed(EXAMPLE);
        assert_eq!(config.dir, Path::new("project").join(DEFAULT_DIR));
        let [main, sim, season, dev] = ["main", "sim", "season", "dev"].map(|name| {
            *config
                .select(&profiles(&[name]))
                .expect("a profile")
                .first()
                .expect("one")
        });
        assert_eq!(
            main.server.auth,
            Auth::Token(Secret::Env("SCREEPS_TOKEN".to_owned()))
        );
        assert_eq!(main.profile.activate, [Active::World]);
        assert_eq!(sim.profile.branch.to_string(), "sim");
        assert_eq!(season.profile.branch, Branch::default());
        assert_eq!(dev.server.url.to_string(), "http://127.0.0.1:21025/");
        assert_eq!(dev.profile.branch, Branch::Auto);
        assert!(matches!(
            &dev.server.auth,
            Auth::Password { email, password: Secret::Value(_) } if email == "me@example.com"
        ));
    }

    /// The `--profile` names have priority, in order and without repeats.
    /// Then `default` applies, and then the only profile.
    #[test]
    fn selection() {
        let config = parsed(EXAMPLE);
        let selected = config.select(&profiles(&["sim", "main", "sim"]));
        assert_eq!(
            selected.map(|s| names(&s)).ok(),
            Some(vec!["sim".to_owned(), "main".to_owned()])
        );
        assert_eq!(
            config.select(&[]).map(|s| names(&s)).ok(),
            Some(vec!["main".to_owned()])
        );
        let error = config
            .select(&profiles(&["prod"]))
            .expect_err("no profile prod");
        assert!(
            error
                .to_string()
                .contains("profiles: dev, main, season, sim"),
            "{error}"
        );

        let without_default = parsed(&EXAMPLE.replace(r#"default = ["main"]"#, ""));
        assert!(matches!(
            without_default.select(&[]),
            Err(Error::Ambiguous { .. })
        ));
        let only = parsed(
            "[servers.s]\nurl = \"https://screeps.com\"\ntoken = \"t\"\n[profiles.p]\nserver = \"s\"\n",
        );
        assert_eq!(
            only.select(&[]).map(|s| names(&s)).ok(),
            Some(vec!["p".to_owned()])
        );
        assert!(only.is_default(&"p".parse().expect("a name")));
    }

    /// The error of a configuration that is not valid names the problem.
    #[test]
    fn invalid() {
        let server = "[servers.s]\nurl = \"https://screeps.com\"\n";
        for (text, expected) in [
            (
                format!("{server}token = \"t\"\nhostname = \"x\"\n"),
                "hostname",
            ),
            (format!("{server}email = \"e\"\n"), "set `token`"),
            (
                format!("{server}token = \"t\"\nemail = \"e\"\npassword = \"p\"\n"),
                "not both",
            ),
            (format!("{server}token = \"\"\n"), "not empty"),
            (format!("{server}token = {{ file = \"x\" }}\n"), "file"),
            (
                format!("{server}token = \"t\"\n[profiles.p]\nserver = \"x\"\n"),
                "no server x; servers: s",
            ),
            (
                format!(
                    "{server}token = \"t\"\n[profiles.p]\nserver = \"s\"\nactivate = [\"moon\"]\n"
                ),
                "moon",
            ),
            (
                format!("{server}token = \"t\"\n[profiles.p]\nserver = \"s\"\nbranch = \"\"\n"),
                "branch",
            ),
            (
                format!("default = [\"q\"]\n{server}token = \"t\"\n[profiles.p]\nserver = \"s\"\n"),
                "default: no profile q",
            ),
            (
                "[servers.\"a b\"]\nurl = \"https://screeps.com\"\ntoken = \"t\"\n".to_owned(),
                "a name has",
            ),
        ] {
            let error = parse(&text, PathBuf::from(FILE_NAME)).expect_err(&text);
            assert!(error.to_string().contains(expected), "{text}\n{error}");
        }
    }

    /// A spawn table has a selector, a shard (`auto`, a name, or none) and
    /// numbers with bounds and defaults.
    #[test]
    fn spawn() {
        let text = format!(
            "{}[profiles.main.spawn]\nselector = [\"node\", \"scripts/select-spawn.mjs\"]\nshard = \"auto\"\n\
             [profiles.dev.spawn]\nselector = [\"sel\"]\ninterval = 120\nradius = 3\ncandidates = 4\n\
             [profiles.season.spawn]\nselector = [\"sel\"]\nshard = \"shardSeason\"\n",
            EXAMPLE.replace(r#"default = ["main"]"#, "")
        );
        let config = parsed(&text);
        let spawn = |name: &str| {
            config.select(&profiles(&[name])).expect("a profile")[0]
                .profile
                .spawn
                .clone()
        };
        let main = spawn("main").expect("a spawn table");
        assert_eq!(main.selector, ["node", "scripts/select-spawn.mjs"]);
        assert_eq!(main.shard, Some(ShardChoice::Auto));
        assert_eq!(main.interval, Duration::from_secs(60));
        assert_eq!((main.radius, main.candidates), (5, 16));
        let dev = spawn("dev").expect("a spawn table");
        assert_eq!(dev.shard, None);
        assert_eq!(dev.interval, Duration::from_secs(120));
        assert_eq!((dev.radius, dev.candidates), (3, 4));
        assert_eq!(
            spawn("season").and_then(|s| s.shard).map(|s| s.to_string()),
            Some("shardSeason".to_owned())
        );
        assert_eq!(spawn("sim"), None);
        assert_eq!(config.root(), Path::new("project"));
        let bare =
            parse(SERVER_ONLY_PROFILE, PathBuf::from(FILE_NAME)).expect("a valid configuration");
        assert_eq!(bare.root(), Path::new("."));

        let all = config.select_spawn(&[]).expect("spawn profiles");
        assert_eq!(names(&all), ["dev", "main", "season"]);
        assert!(matches!(
            config.select_spawn(&profiles(&["sim"])),
            Err(Error::NoSpawn { .. })
        ));
        assert!(matches!(
            parsed(EXAMPLE).select_spawn(&[]),
            Err(Error::NoSpawnProfile { .. })
        ));
        let shared = text.replace(
            "[profiles.dev]\n        server = \"local\"",
            "[profiles.dev]\n        server = \"official\"",
        );
        assert_ne!(shared, text, "the replacement applies");
        assert!(matches!(
            parsed(&shared).select_spawn(&profiles(&["main", "dev"])),
            Err(Error::SharedServer { .. })
        ));
        let separate = shared
            .replace("shard = \"auto\"", "shard = \"shard3\"")
            .replace(
                "[profiles.dev.spawn]",
                "[profiles.dev.spawn]\nshard = \"shard0\"",
            );
        assert_eq!(
            names(
                &parsed(&separate)
                    .select_spawn(&profiles(&["main", "dev"]))
                    .expect("distinct shards")
            ),
            ["main", "dev"]
        );
        for overlap in [
            separate.replace("shard = \"shard0\"", "shard = \"shard3\""),
            separate.replace("shard = \"shard0\"", "shard = \"auto\""),
        ] {
            assert!(matches!(
                parsed(&overlap).select_spawn(&profiles(&["main", "dev"])),
                Err(Error::SharedServer { .. })
            ));
        }
    }

    /// The error of a spawn table that is not valid names the problem.
    #[test]
    fn invalid_spawn() {
        let base = format!("{SERVER_ONLY_PROFILE}[profiles.p.spawn]\n");
        for (text, expected) in [
            (format!("{base}selector = []\n"), "`selector`"),
            (format!("{base}selector = [\"\"]\n"), "`selector`"),
            (format!("{base}shard = \"auto\"\n"), "selector"),
            (
                format!("{base}selector = [\"s\"]\ninterval = 5\n"),
                "`interval` = 5: use 60..=86400",
            ),
            (
                format!("{base}selector = [\"s\"]\nradius = 0\n"),
                "`radius` = 0",
            ),
            (
                format!("{base}selector = [\"s\"]\ncandidates = 65\n"),
                "`candidates` = 65",
            ),
            (
                format!("{base}selector = [\"s\"]\nshard = \"a b\"\n"),
                "a name has",
            ),
            (format!("{base}selector = [\"s\"]\nname = \"x\"\n"), "name"),
        ] {
            let error = parse(&text, PathBuf::from(FILE_NAME)).expect_err(&text);
            assert!(error.to_string().contains(expected), "{text}\n{error}");
        }
    }
}
