//! The `servers` command: lists, adds and removes the saved servers. The
//! desktop app shows the same servers. The command needs no configuration
//! file.

use std::io::Write;

use overseer::core::ServerUrl;
use overseer_storage::profiles::{
    Change, SEASON_URL, Server, ServerKind, normalize_url, server_password_key, token_key,
};

use crate::Error;
use crate::saved::Saved;
use crate::table;

/// The arguments of `servers`.
#[derive(Debug, clap::Args)]
pub(crate) struct Servers {
    /// The change. Without a change, `servers` lists the saved servers.
    #[command(subcommand)]
    action: Option<Action>,
}

/// A change of the saved servers.
#[derive(Debug, clap::Subcommand)]
enum Action {
    /// Save a server. Then sign in to it with `overseer login`.
    Add {
        /// The URL of the server, for example `http://127.0.0.1:21025`.
        /// Without a scheme, the CLI uses `http://`
        url: String,
        /// The name of the server [default: Season for the seasonal server,
        /// else the host, the port and the path of the URL]
        #[arg(long)]
        name: Option<String>,
    },
    /// Remove a saved server, its saved token and its saved server password.
    /// The CLI cannot remove screeps.com.
    Remove {
        /// The name or the URL of the server
        server: String,
    },
}

/// A change of the saved servers that the CLI refuses.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Refusal {
    /// The server is not valid, or it is saved already.
    #[error("cannot save the server: {0}")]
    Add(String),
    /// The server is the built-in screeps.com entry.
    #[error("{0} is built in: the CLI cannot remove it")]
    BuiltIn(String),
}

impl Servers {
    /// Lists or changes the saved servers of `saved`. Prints to `out`.
    pub(crate) fn run(self, saved: &Saved, out: &mut dyn Write) -> Result<(), Error> {
        match self.action {
            None => list(saved, out),
            Some(Action::Add { url, name }) => {
                let server = add(saved, &url, name)?;
                writeln!(out, "saved server {} ({})", server.name, server.url)?;
                writeln!(
                    out,
                    "sign in with `overseer login {} --token`",
                    login_arg(&server)
                )?;
                Ok(())
            }
            Some(Action::Remove { server }) => {
                let server = saved.find(&server)?;
                if server.kind == ServerKind::Official {
                    return Err(Refusal::BuiltIn(server.name).into());
                }
                saved
                    .update(|profiles| {
                        profiles.apply(Change::RemoveServer {
                            url: server.url.clone(),
                        })
                    })?
                    .map_err(Refusal::Add)?;
                saved.delete_secret(&token_key(&server.url))?;
                saved.delete_secret(&server_password_key(&server.url))?;
                writeln!(out, "removed server {} ({})", server.name, server.url)?;
                Ok(())
            }
        }
    }
}

/// Saves the server at `url` with the name `name`, or with the default name.
/// Returns the saved server.
///
/// # Errors
///
/// Returns an error when the URL is not valid, the server is saved already,
/// or the storage fails.
pub(crate) fn add(saved: &Saved, url: &str, name: Option<String>) -> Result<Server, Error> {
    let url = normalize_url(url).map_err(Refusal::Add)?;
    let kind = if url == SEASON_URL {
        ServerKind::Season
    } else {
        ServerKind::Private
    };
    let name = name.unwrap_or_else(|| default_name(&url, kind));
    let server = Server::new(name, url, kind);
    let added = saved.update(|profiles| {
        profiles.apply(Change::AddServer {
            server: server.clone(),
        })
    })?;
    let url = added.map_err(Refusal::Add)?.unwrap_or(server.url);
    Ok(Server { url, ..server })
}

/// The name of a new server at the normalized URL `url` without a name:
/// `Season` for the seasonal server, else the host, the port and the path.
fn default_name(url: &str, kind: ServerKind) -> String {
    if kind == ServerKind::Season {
        return "Season".to_owned();
    }
    let Ok(parsed) = ServerUrl::parse(url) else {
        return url.to_owned();
    };
    let base = parsed.base();
    let mut name = base.host_str().unwrap_or_default().to_owned();
    if let Some(port) = base.port() {
        name = format!("{name}:{port}");
    }
    let path = base.path().trim_end_matches('/');
    format!("{name}{path}")
}

/// The argument of `overseer login` for `server`: its name, or its URL when
/// the name has a space.
pub(crate) fn login_arg(server: &Server) -> &str {
    if server.name.contains(char::is_whitespace) {
        &server.url
    } else {
        &server.name
    }
}

/// Prints the saved servers. Reads the credential store only to tell
/// whether a private server has a server password. Prints no secret.
fn list(saved: &Saved, out: &mut dyn Write) -> Result<(), Error> {
    let storage = saved.storage()?;
    writeln!(
        out,
        "saved servers {} (credential service {})",
        storage.profiles_path().display(),
        storage.service()
    )?;
    let mut rows = vec![
        ["NAME", "URL", "KIND", "USER", "SIGN-IN", "SERVER PASSWORD"]
            .map(String::from)
            .to_vec(),
    ];
    for server in saved.load()?.servers {
        let user = server.user.as_ref();
        let account = user.map_or_else(
            || "-".to_owned(),
            |user| user.username.clone().unwrap_or_else(|| user.id.clone()),
        );
        let sign_in = match user.map(|user| user.api_token) {
            None => "none",
            Some(Some(true)) => "API token",
            Some(Some(false)) => "session token",
            Some(None) => "token",
        };
        let server_password = if server.kind == ServerKind::Private {
            if saved.secret(&server_password_key(&server.url))?.is_some() {
                "set"
            } else {
                "none"
            }
        } else {
            "-"
        };
        let kind = match server.kind {
            ServerKind::Official => "official",
            ServerKind::Season => "season",
            ServerKind::Private => "private",
        };
        rows.push(vec![
            server.name,
            server.url,
            kind.to_owned(),
            account,
            sign_in.to_owned(),
            server_password.to_owned(),
        ]);
    }
    table::print(&rows, out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use overseer_storage::profiles::{OFFICIAL_URL, SavedUser};

    use super::*;
    use crate::config::Sensitive;
    use crate::saved::testing::TempSaved;

    /// Runs `servers` with `action` and returns the output.
    fn run(saved: &Saved, action: Option<Action>) -> Result<String, Error> {
        let mut out = Vec::new();
        Servers { action }.run(saved, &mut out)?;
        Ok(String::from_utf8(out).expect("UTF-8 output"))
    }

    /// `add` saves a server with a default name, `remove` removes it and its
    /// secrets, and screeps.com stays.
    #[test]
    fn add_list_remove() {
        let saved = TempSaved::new("servers");
        let added = run(
            &saved,
            Some(Action::Add {
                url: "127.0.0.1:21025".to_owned(),
                name: None,
            }),
        )
        .expect("add");
        assert!(
            added.contains("saved server 127.0.0.1:21025 (http://127.0.0.1:21025/)"),
            "{added}"
        );
        let season = add(&saved, "https://screeps.com/season", None).expect("add season");
        assert_eq!(
            (season.name.as_str(), season.kind),
            ("Season", ServerKind::Season)
        );
        let named = add(&saved, "http://host/sub/", Some("e2e".to_owned())).expect("add");
        assert_eq!(named.url, "http://host/sub/");
        assert_eq!(
            default_name("http://host/sub/", ServerKind::Private),
            "host/sub"
        );
        assert!(matches!(
            add(&saved, "http://127.0.0.1:21025/", None),
            Err(Error::Servers(Refusal::Add(_)))
        ));

        let url = "http://127.0.0.1:21025/";
        saved
            .update(|profiles| {
                profiles.server_mut(url).expect("saved").user = Some(SavedUser {
                    id: "u1".to_owned(),
                    username: Some("Me".to_owned()),
                    api_token: Some(false),
                });
            })
            .expect("update");
        for key in [token_key(url), server_password_key(url)] {
            saved
                .set_secret(&key, &Sensitive::new("secret-value".to_owned()))
                .expect("set");
        }
        let listing = run(&saved, None).expect("list");
        assert!(!listing.contains("secret-value"), "{listing}");
        let line = listing
            .lines()
            .find(|line| line.starts_with("127.0.0.1:21025"))
            .expect("the server line");
        for cell in ["private", "Me", "session token", "set"] {
            assert!(line.contains(cell), "{line}");
        }

        let removed = run(
            &saved,
            Some(Action::Remove {
                server: "127.0.0.1:21025".to_owned(),
            }),
        )
        .expect("remove");
        assert!(removed.contains("removed server"), "{removed}");
        assert_eq!(saved.secret(&token_key(url)).expect("read"), None);
        assert_eq!(saved.secret(&server_password_key(url)).expect("read"), None);
        assert!(matches!(
            run(
                &saved,
                Some(Action::Remove {
                    server: "screeps.com".to_owned()
                })
            ),
            Err(Error::Servers(Refusal::BuiltIn(_)))
        ));
        let profiles = saved.load().expect("load");
        assert_eq!(profiles.servers[0].url, OFFICIAL_URL);
        assert_eq!(profiles.servers.len(), 3);
    }
}
