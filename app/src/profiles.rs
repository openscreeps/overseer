//! Saved servers and who is signed in on them: everything the app remembers except secrets.
//!
//! Persisted as one JSON document (`localStorage` key [`STORAGE_KEY`]). Parsing never fails:
//! fields added later default, unknown fields are ignored, an entry that does not decode is
//! dropped on its own, and screeps.com is always listed first. Tokens and server passwords
//! are kept apart in the platform's secret store under [`token_key`] and
//! [`server_password_key`].

use serde::{Deserialize, Serialize};
use url::Url;

/// `localStorage` key of the saved profiles.
pub const STORAGE_KEY: &str = "overseer.profiles";
/// Version written into the saved document; older documents parse with defaults.
const VERSION: u32 = 1;
/// The official server.
pub const OFFICIAL_URL: &str = "https://screeps.com/";
/// The official seasonal server (screeps.com accounts).
pub const SEASON_URL: &str = "https://screeps.com/season/";

/// What a server is, which decides how to sign in and register.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerKind {
    /// screeps.com (built in).
    Official,
    /// The seasonal world on screeps.com.
    Season,
    /// A private server.
    Private,
}

impl ServerKind {
    /// screeps.com accounts: API token or e-mail sign-in, registration and password recovery
    /// on the website.
    pub const fn uses_screeps_accounts(self) -> bool {
        matches!(self, Self::Official | Self::Season)
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
}

/// A saved server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Server {
    /// Display name.
    pub name: String,
    /// Normalized base URL ([`normalize_url`]); identifies the server.
    pub url: String,
    /// What the server is.
    pub kind: ServerKind,
    /// The name last used to sign in (username or e-mail), to fill the sign-in form.
    #[serde(default)]
    pub login: Option<String>,
    /// The signed-in account; `None` when signed out.
    #[serde(default)]
    pub user: Option<SavedUser>,
    /// The room last opened, as `shard/room` or `room`.
    #[serde(default)]
    pub room: Option<String>,
}

impl Server {
    /// A server without sign-in state.
    pub const fn new(name: String, url: String, kind: ServerKind) -> Self {
        Self {
            name,
            url,
            kind,
            login: None,
            user: None,
            room: None,
        }
    }

    /// The built-in screeps.com entry.
    pub fn official() -> Self {
        Self::new(
            "screeps.com".to_owned(),
            OFFICIAL_URL.to_owned(),
            ServerKind::Official,
        )
    }
}

/// Every saved server and the one in use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profiles {
    /// Servers, screeps.com first.
    pub servers: Vec<Server>,
    /// URL of the server whose session resumes on start.
    pub active: Option<String>,
}

impl Default for Profiles {
    fn default() -> Self {
        Self {
            servers: vec![Server::official()],
            active: None,
        }
    }
}

/// The saved document, read leniently.
#[derive(Default, Deserialize)]
#[serde(default)]
struct Stored {
    servers: Vec<serde_json::Value>,
    active: Option<String>,
}

/// The saved document as written.
#[derive(Serialize)]
struct Saved<'a> {
    version: u32,
    servers: &'a [Server],
    active: Option<&'a str>,
}

impl Profiles {
    /// Reads the saved document; `None`, invalid JSON or invalid entries fall back to defaults
    /// (see the module docs).
    pub fn parse(json: Option<&str>) -> Self {
        let stored: Stored = json
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_default();
        let mut profiles = Self {
            servers: Vec::new(),
            active: None,
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
        profiles.active = stored
            .active
            .filter(|active| profiles.server(active).is_some());
        profiles
    }

    /// The document to save.
    pub fn to_json(&self) -> String {
        let saved = Saved {
            version: VERSION,
            servers: &self.servers,
            active: self.active.as_deref(),
        };
        serde_json::to_string(&saved).unwrap_or_default()
    }

    /// The server with base URL `url`.
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
        if self.active.as_deref() == Some(url) {
            self.active = None;
        }
    }
}

/// The base URL of a server as entered: `http://` is assumed without a scheme, the path gets
/// a trailing `/`, and only `http` and `https` without credentials, query or fragment are
/// accepted.
///
/// # Errors
///
/// Returns a message for anything else.
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
    let mut url = Url::parse(&with_scheme).map_err(|error| format!("Invalid address: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("The address must be an http:// or https:// URL".to_owned());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("The address must not contain credentials".to_owned());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("The address must not contain a query or fragment".to_owned());
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url.into())
}

/// Secret-store key of the session or API token for the server at `url`.
pub fn token_key(url: &str) -> String {
    format!("token:{url}")
}

/// Secret-store key of the server password (`X-Server-Password`) for the server at `url`.
pub fn server_password_key(url: &str) -> String {
    format!("server-password:{url}")
}

#[cfg(test)]
mod tests {
    use super::{OFFICIAL_URL, Profiles, SEASON_URL, SavedUser, Server, ServerKind, normalize_url};

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
        });
        local.room = Some("W1N1".to_owned());
        profiles.add(local).unwrap();
        profiles.active = Some("http://localhost:21025/".to_owned());
        let parsed = Profiles::parse(Some(&profiles.to_json()));
        assert_eq!(parsed, profiles);
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
                username: None
            })
        );
        assert_eq!(profiles.servers[1].url, "http://old.example:21025/");
        assert_eq!(profiles.servers[1].user, None);
        assert_eq!(profiles.active, None);
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
        profiles.active = Some("http://localhost:21025/".to_owned());
        profiles.remove("http://localhost:21025/");
        profiles.remove(OFFICIAL_URL);
        assert_eq!(profiles, Profiles::default());
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
        for bad in [
            "",
            "ws://host/",
            "http://user:pw@host/",
            "http://host/?a=1",
            "http://host/#x",
            "http://",
        ] {
            assert!(normalize_url(bad).is_err(), "{bad}");
        }
    }
}
