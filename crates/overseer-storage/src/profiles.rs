//! The saved servers and who is signed in on them: all that the app remembers, except
//! secrets.
//!
//! The profiles are one JSON document, written with [`VERSION`]. Parsing never fails: fields
//! added later get their defaults, unknown fields are ignored, an entry that does not decode is
//! dropped alone, and screeps.com is always first. Tokens and server passwords are in the
//! secret store of the platform, under [`token_key`] and [`server_password_key`].

mod change;

use overseer_core::{ServerUrl, ServerUrlError};
use serde::{Deserialize, Serialize};

pub use change::Change;

/// Version written into the saved document; older documents parse with defaults.
pub const VERSION: u32 = 2;
/// The official server.
pub const OFFICIAL_URL: &str = "https://screeps.com/";
/// The official seasonal server (screeps.com accounts).
pub const SEASON_URL: &str = "https://screeps.com/season/";

/// How the user added the server and how the server list shows it. For screeps.com entries,
/// it also gives the contract to force ([`Self::forced`]). It gives nothing more: what the app
/// offers on the server (sign-in methods, registration, lifting rate limits, …) follows from
/// the subsystems of the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerKind {
    /// screeps.com (built in, never removed).
    Official,
    /// The seasonal world on screeps.com (the preset of the "Add server" dialog).
    Season,
    /// A private server.
    Private,
}

impl ServerKind {
    /// The contract to force when connecting (`ServerProfile::server_kind` of `overseer`):
    /// the official one for screeps.com entries, whatever their address. `None` discovers the
    /// contract (private servers).
    #[must_use]
    pub const fn forced(self) -> Option<overseer_core::model::platform::ServerKind> {
        match self {
            Self::Official | Self::Season => {
                Some(overseer_core::model::platform::ServerKind::Official)
            }
            Self::Private => None,
        }
    }
}

/// The signed-in account of a server; its token is in the secret store.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedUser {
    /// User id (`_id`).
    pub id: String,
    /// Username; `None` until the account chose one.
    #[serde(default)]
    pub username: Option<String>,
    /// How the account signed in: `Some(true)` with an API token (its rate limits can be
    /// lifted where the server lifts them), `Some(false)` with a password (a session token,
    /// which matches no API token), `None` when saved before the app recorded it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_token: Option<bool>,
}

/// A saved server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Server {
    /// Display name.
    pub name: String,
    /// Normalized base URL ([`normalize_url`]); identifies the server.
    pub url: String,
    /// How the server was added and is listed.
    pub kind: ServerKind,
    /// The name last used to sign in (username or e-mail), to fill the sign-in form.
    #[serde(default)]
    pub login: Option<String>,
    /// The signed-in account; `None` when signed out.
    #[serde(default)]
    pub user: Option<SavedUser>,
}

impl Server {
    /// A server without sign-in state.
    #[must_use]
    pub const fn new(name: String, url: String, kind: ServerKind) -> Self {
        Self {
            name,
            url,
            kind,
            login: None,
            user: None,
        }
    }

    /// The built-in screeps.com entry.
    #[must_use]
    pub fn official() -> Self {
        Self::new(
            "screeps.com".to_owned(),
            OFFICIAL_URL.to_owned(),
            ServerKind::Official,
        )
    }
}

/// Every saved server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profiles {
    /// Servers, screeps.com first.
    pub servers: Vec<Server>,
}

impl Default for Profiles {
    fn default() -> Self {
        Self {
            servers: vec![Server::official()],
        }
    }
}

/// The saved document, read leniently.
#[derive(Default, Deserialize)]
#[serde(default)]
struct Stored {
    servers: Vec<serde_json::Value>,
}

/// The saved document as written.
#[derive(Serialize)]
struct Saved<'a> {
    version: u32,
    servers: &'a [Server],
}

impl Profiles {
    /// Reads the saved document; `None`, invalid JSON or invalid entries fall back to defaults
    /// (see the module docs).
    #[must_use]
    pub fn parse(json: Option<&str>) -> Self {
        let stored: Stored = json
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_default();
        let mut profiles = Self {
            servers: Vec::new(),
        };
        for value in stored.servers {
            let Ok(mut server) = serde_json::from_value::<Server>(value) else {
                continue;
            };
            let Ok(url) = normalize_url(&server.url) else {
                continue;
            };
            server.url = url;
            if profiles.server(&server.url).is_none() {
                profiles.servers.push(server);
            }
        }
        match profiles
            .servers
            .iter()
            .position(|server| server.url == OFFICIAL_URL)
        {
            Some(index) => {
                let mut official = profiles.servers.remove(index);
                official.kind = ServerKind::Official;
                profiles.servers.insert(0, official);
            }
            None => profiles.servers.insert(0, Server::official()),
        }
        profiles
    }

    /// The document to save.
    #[must_use]
    pub fn to_json(&self) -> String {
        let saved = Saved {
            version: VERSION,
            servers: &self.servers,
        };
        serde_json::to_string(&saved).unwrap_or_default()
    }

    /// The server with base URL `url`.
    #[must_use]
    pub fn server(&self, url: &str) -> Option<&Server> {
        self.servers.iter().find(|server| server.url == url)
    }

    /// The server with base URL `url`, for changes.
    pub fn server_mut(&mut self, url: &str) -> Option<&mut Server> {
        self.servers.iter_mut().find(|server| server.url == url)
    }

    /// Adds `server` (its URL normalized) and returns its URL.
    ///
    /// # Errors
    ///
    /// Returns a message when the URL is invalid or the server is already listed.
    pub fn add(&mut self, mut server: Server) -> Result<String, String> {
        if server.name.trim().is_empty() {
            return Err("Enter a name for the server".to_owned());
        }
        server.url = normalize_url(&server.url)?;
        if let Some(existing) = self.server(&server.url) {
            return Err(format!(
                "{} is already listed as {}",
                server.url, existing.name
            ));
        }
        let name = server.name.trim().to_owned();
        server.name = name;
        let url = server.url.clone();
        self.servers.push(server);
        Ok(url)
    }

    /// Removes the server with base URL `url` (never screeps.com).
    pub fn remove(&mut self, url: &str) {
        self.servers
            .retain(|server| server.kind == ServerKind::Official || server.url != url);
    }

    /// The saved server that `key` names: the only server with the name `key`, else the
    /// server whose base URL is the normalized `key` ([`normalize_url`]). `None` when no server
    /// matches.
    ///
    /// # Errors
    ///
    /// Returns [`Ambiguous`] when two or more servers have the name `key`.
    pub fn find(&self, key: &str) -> Result<Option<&Server>, Ambiguous> {
        let key = key.trim();
        let named: Vec<&Server> = self
            .servers
            .iter()
            .filter(|server| server.name == key)
            .collect();
        match named.as_slice() {
            [] => Ok(normalize_url(key).ok().and_then(|url| self.server(&url))),
            [server] => Ok(Some(server)),
            several => Err(Ambiguous {
                name: key.to_owned(),
                urls: several.iter().map(|server| server.url.clone()).collect(),
            }),
        }
    }
}

/// Two or more saved servers have the name that [`Profiles::find`] looks for.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{} saved servers have the name {name}: {}; use the URL", urls.len(), urls.join(", "))]
pub struct Ambiguous {
    /// The name.
    pub name: String,
    /// The base URLs of the servers with this name.
    pub urls: Vec<String>,
}

/// The base URL of a server as entered: `http://` is assumed without a scheme, then
/// [`ServerUrl::parse`] makes the base.
///
/// # Errors
///
/// Returns a message for empty input and for the errors of [`ServerUrl::parse`].
pub fn normalize_url(input: &str) -> Result<String, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Enter the server address".to_owned());
    }
    let with_scheme = if input.contains("://") {
        input.to_owned()
    } else {
        format!("http://{input}")
    };
    match ServerUrl::parse(&with_scheme) {
        Ok(url) => Ok(url.base().as_str().to_owned()),
        Err(ServerUrlError::Parse(error)) => Err(format!("Invalid address: {error}")),
        Err(ServerUrlError::Scheme(_)) => {
            Err("The address must be an http:// or https:// URL".to_owned())
        }
        Err(ServerUrlError::Credentials) => {
            Err("The address must not contain credentials".to_owned())
        }
        Err(ServerUrlError::QueryOrFragment) => {
            Err("The address must not contain a query or fragment".to_owned())
        }
    }
}

/// Secret-store key of the session or API token for the server at `url`.
#[must_use]
pub fn token_key(url: &str) -> String {
    format!("token:{url}")
}

/// Secret-store key of the server password (`X-Server-Password`) for the server at `url`.
#[must_use]
pub fn server_password_key(url: &str) -> String {
    format!("server-password:{url}")
}

#[cfg(test)]
mod tests {
    use super::{
        Ambiguous, OFFICIAL_URL, Profiles, SEASON_URL, SavedUser, Server, ServerKind, VERSION,
        normalize_url, server_password_key, token_key,
    };

    fn private(url: &str) -> Server {
        Server::new("Local".to_owned(), url.to_owned(), ServerKind::Private)
    }

    #[test]
    fn defaults_list_screeps_com_only() {
        for json in [None, Some(""), Some("not json"), Some("[1,2]"), Some("{}")] {
            let profiles = Profiles::parse(json);
            assert_eq!(profiles, Profiles::default(), "{json:?}");
            assert_eq!(profiles.servers[0].url, OFFICIAL_URL);
        }
    }

    #[test]
    fn screeps_com_entries_force_the_official_contract_and_private_ones_discover_it() {
        use overseer_core::model::platform::ServerKind as Contract;
        assert_eq!(ServerKind::Official.forced(), Some(Contract::Official));
        assert_eq!(ServerKind::Season.forced(), Some(Contract::Official));
        assert_eq!(ServerKind::Private.forced(), None);
    }

    #[test]
    fn round_trips() {
        let mut profiles = Profiles::default();
        profiles
            .add(Server::new(
                "Season".to_owned(),
                SEASON_URL.to_owned(),
                ServerKind::Season,
            ))
            .unwrap();
        let mut local = private("localhost:21025");
        local.login = Some("player".to_owned());
        local.user = Some(SavedUser {
            id: "u1".to_owned(),
            username: Some("Player".to_owned()),
            api_token: Some(false),
        });
        let official = profiles.server_mut(OFFICIAL_URL).unwrap();
        official.user = Some(SavedUser {
            id: "u2".to_owned(),
            username: None,
            api_token: Some(true),
        });
        profiles.add(local).unwrap();
        let parsed = Profiles::parse(Some(&profiles.to_json()));
        assert_eq!(parsed, profiles);
    }

    #[test]
    fn the_document_format_stays_the_same() {
        let mut profiles = Profiles::default();
        let mut local = private("localhost:21025");
        local.login = Some("player".to_owned());
        local.user = Some(SavedUser {
            id: "u1".to_owned(),
            username: None,
            api_token: Some(true),
        });
        profiles.add(local).unwrap();
        assert_eq!(VERSION, 2);
        assert_eq!(
            profiles.to_json(),
            concat!(
                r#"{"version":2,"servers":["#,
                r#"{"name":"screeps.com","url":"https://screeps.com/","kind":"official","login":null,"user":null},"#,
                r#"{"name":"Local","url":"http://localhost:21025/","kind":"private","login":"player","#,
                r#""user":{"id":"u1","username":null,"api_token":true}}]}"#
            )
        );
        assert_eq!(token_key(OFFICIAL_URL), "token:https://screeps.com/");
        assert_eq!(
            server_password_key(OFFICIAL_URL),
            "server-password:https://screeps.com/"
        );
    }

    #[test]
    fn parsing_keeps_what_decodes() {
        let json = r#"{
            "version": 7,
            "future": {"x": 1},
            "active": "http://gone/",
            "servers": [
                {"name": "Old", "url": "http://old.example:21025", "kind": "private"},
                {"name": "Bad kind", "url": "http://b/", "kind": "steam"},
                {"name": "No URL", "kind": "private"},
                {"name": "Bad URL", "url": "ftp://x/", "kind": "private"},
                {"name": "Duplicate", "url": "http://old.example:21025/", "kind": "private"},
                {"name": "Mine", "url": "https://screeps.com/", "kind": "private",
                 "login": "me", "user": {"id": "1", "extra": true}, "newField": 3}
            ]
        }"#;
        let profiles = Profiles::parse(Some(json));
        let names: Vec<_> = profiles.servers.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Mine", "Old"]);
        let official = &profiles.servers[0];
        assert_eq!(official.kind, ServerKind::Official);
        assert_eq!(official.login.as_deref(), Some("me"));
        assert_eq!(
            official.user,
            Some(SavedUser {
                id: "1".to_owned(),
                username: None,
                // Saved before the app recorded how the account signed in.
                api_token: None,
            })
        );
        assert_eq!(profiles.servers[1].url, "http://old.example:21025/");
        assert_eq!(profiles.servers[1].user, None);
    }

    #[test]
    fn adding_rejects_duplicates_and_bad_input() {
        let mut profiles = Profiles::default();
        profiles.add(private("http://LOCALHOST:21025")).unwrap();
        assert!(profiles.add(private("http://localhost:21025/")).is_err());
        assert!(profiles.add(private("https://screeps.com")).is_err());
        assert!(profiles.add(private("")).is_err());
        assert!(
            profiles
                .add(Server::new(
                    " ".to_owned(),
                    "http://x/".to_owned(),
                    ServerKind::Private
                ))
                .is_err()
        );
        assert_eq!(profiles.servers.len(), 2);
        profiles.remove("http://localhost:21025/");
        profiles.remove(OFFICIAL_URL);
        assert_eq!(profiles, Profiles::default());
    }

    #[test]
    fn finds_a_server_by_name_or_url() {
        let mut profiles = Profiles::default();
        profiles.add(private("localhost:21025")).unwrap();
        let url = |found: Option<&Server>| found.map(|server| server.url.clone());
        assert_eq!(
            url(profiles.find(" Local ").unwrap()).as_deref(),
            Some("http://localhost:21025/")
        );
        assert_eq!(
            url(profiles.find("screeps.com").unwrap()).as_deref(),
            Some(OFFICIAL_URL)
        );
        assert_eq!(
            url(profiles.find("https://screeps.com").unwrap()).as_deref(),
            Some(OFFICIAL_URL)
        );
        assert_eq!(
            url(profiles.find("localhost:21025").unwrap()).as_deref(),
            Some("http://localhost:21025/")
        );
        assert_eq!(profiles.find("local").unwrap(), None);
        assert_eq!(profiles.find("http://other:21025").unwrap(), None);
        assert_eq!(profiles.find("").unwrap(), None);

        profiles.add(private("http://second:21025")).unwrap();
        let error = profiles.find("Local").unwrap_err();
        assert_eq!(
            error,
            Ambiguous {
                name: "Local".to_owned(),
                urls: vec![
                    "http://localhost:21025/".to_owned(),
                    "http://second:21025/".to_owned()
                ],
            }
        );
        assert_eq!(
            error.to_string(),
            "2 saved servers have the name Local: http://localhost:21025/, http://second:21025/; use the URL"
        );
        // The URL still finds one of them.
        assert_eq!(
            url(profiles.find("http://second:21025/").unwrap()).as_deref(),
            Some("http://second:21025/")
        );
    }

    #[test]
    fn normalizes_urls() {
        assert_eq!(
            normalize_url(" localhost:21025 ").unwrap(),
            "http://localhost:21025/"
        );
        assert_eq!(
            normalize_url("https://screeps.com/season").unwrap(),
            SEASON_URL
        );
        assert_eq!(
            normalize_url("HTTPS://Screeps.com:443/").unwrap(),
            OFFICIAL_URL
        );
        for (bad, message) in [
            ("  ", "Enter the server address"),
            (
                "ws://host/",
                "The address must be an http:// or https:// URL",
            ),
            ("user:pw@host", "The address must not contain credentials"),
            (
                "host/#x",
                "The address must not contain a query or fragment",
            ),
            ("http://", "Invalid address: empty host"),
        ] {
            assert_eq!(normalize_url(bad), Err(message.to_owned()), "{bad}");
        }
    }
}
