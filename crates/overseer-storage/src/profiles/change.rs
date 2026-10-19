//! The changes of the saved profiles. A store applies a change to its current document under
//! its lock ([`Profiles::apply`]). Thus the changes of other processes stay. The app, its
//! desktop shell and the CLI use the same changes, so the change logic exists one time.

use serde::{Deserialize, Serialize};

use super::{Profiles, SavedUser, Server};

/// A change of the saved profiles. A store applies it to its current document under its lock
/// ([`Profiles::apply`]), so that changes from other processes stay.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum Change {
    /// Adds `server` ([`Profiles::add`]).
    AddServer {
        /// The server to add.
        server: Server,
    },
    /// Removes the server with base URL `url` ([`Profiles::remove`]).
    RemoveServer {
        /// The base URL of the server.
        url: String,
    },
    /// A session began on the server with base URL `url`.
    SignedIn {
        /// The base URL of the server.
        url: String,
        /// The account; `None` saves no account (signed out).
        user_id: Option<String>,
        /// How it signed in ([`SavedUser::api_token`]); `None` keeps the saved value of the
        /// same account.
        api_token: Option<bool>,
        /// The name used to sign in; `None` keeps the saved `login`.
        login: Option<String>,
    },
    /// Saves the username of the account signed in on `url` (no account: no change).
    Username {
        /// The base URL of the server.
        url: String,
        /// The username; `None` until the account chose one.
        username: Option<String>,
    },
    /// Marks the server with base URL `url` signed out (`user = None`).
    SignedOut {
        /// The base URL of the server.
        url: String,
    },
}

impl Profiles {
    /// Applies `change`. Returns the URL of an added server, else `None`. A change of a server
    /// that is not listed does nothing.
    ///
    /// # Errors
    ///
    /// Returns the message of [`Self::add`].
    pub fn apply(&mut self, change: Change) -> Result<Option<String>, String> {
        match change {
            Change::AddServer { server } => return self.add(server).map(Some),
            Change::RemoveServer { url } => self.remove(&url),
            Change::SignedIn {
                url,
                user_id,
                api_token,
                login,
            } => {
                if let Some(server) = self.server_mut(&url) {
                    let saved = server.user.take();
                    if let Some(login) = login {
                        server.login = Some(login);
                    }
                    server.user = user_id.map(|id| {
                        // The username and the sign-in kind stay for the same account only.
                        let same = saved.as_ref().filter(|user| user.id == id);
                        SavedUser {
                            username: same.and_then(|user| user.username.clone()),
                            api_token: api_token.or_else(|| same.and_then(|user| user.api_token)),
                            id,
                        }
                    });
                }
            }
            Change::Username { url, username } => {
                if let Some(user) = self
                    .server_mut(&url)
                    .and_then(|server| server.user.as_mut())
                {
                    user.username = username;
                }
            }
            Change::SignedOut { url } => {
                if let Some(server) = self.server_mut(&url) {
                    server.user = None;
                }
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::Change;
    use crate::profiles::{OFFICIAL_URL, Profiles, SavedUser, Server, ServerKind};

    const URL: &str = "http://localhost:21025/";

    fn local() -> Server {
        Server::new("Local".to_owned(), URL.to_owned(), ServerKind::Private)
    }

    /// The profiles with the local server, signed in as `u1` with the username `Player`.
    fn signed_in() -> Profiles {
        let mut profiles = Profiles::default();
        let mut server = local();
        server.login = Some("player".to_owned());
        server.user = Some(SavedUser {
            id: "u1".to_owned(),
            username: Some("Player".to_owned()),
            api_token: Some(false),
        });
        profiles.add(server).unwrap();
        profiles
    }

    fn user(profiles: &Profiles) -> Option<&SavedUser> {
        profiles.server(URL).and_then(|server| server.user.as_ref())
    }

    #[test]
    fn add_server_returns_the_normalized_url() {
        let mut profiles = Profiles::default();
        let mut server = local();
        server.url = "localhost:21025".to_owned();
        assert_eq!(
            profiles.apply(Change::AddServer {
                server: server.clone()
            }),
            Ok(Some(URL.to_owned()))
        );
        assert!(profiles.apply(Change::AddServer { server }).is_err());
        assert_eq!(profiles.servers.len(), 2);
    }

    #[test]
    fn remove_server_never_removes_screeps_com() {
        let mut profiles = signed_in();
        for url in [URL, OFFICIAL_URL] {
            assert_eq!(
                profiles.apply(Change::RemoveServer {
                    url: url.to_owned()
                }),
                Ok(None)
            );
        }
        assert_eq!(profiles, Profiles::default());
    }

    #[test]
    fn signed_in_keeps_the_username_and_kind_of_the_same_account_only() {
        let mut profiles = signed_in();
        let same = Change::SignedIn {
            url: URL.to_owned(),
            user_id: Some("u1".to_owned()),
            api_token: None,
            login: None,
        };
        assert_eq!(profiles.apply(same), Ok(None));
        assert_eq!(
            user(&profiles),
            Some(&SavedUser {
                id: "u1".to_owned(),
                username: Some("Player".to_owned()),
                api_token: Some(false),
            })
        );
        assert_eq!(
            profiles.server(URL).unwrap().login.as_deref(),
            Some("player")
        );

        let other = Change::SignedIn {
            url: URL.to_owned(),
            user_id: Some("u2".to_owned()),
            api_token: None,
            login: Some("other".to_owned()),
        };
        profiles.apply(other).unwrap();
        assert_eq!(
            user(&profiles),
            Some(&SavedUser {
                id: "u2".to_owned(),
                username: None,
                api_token: None,
            })
        );
        assert_eq!(
            profiles.server(URL).unwrap().login.as_deref(),
            Some("other")
        );

        let api = Change::SignedIn {
            url: URL.to_owned(),
            user_id: Some("u2".to_owned()),
            api_token: Some(true),
            login: None,
        };
        profiles.apply(api).unwrap();
        assert_eq!(user(&profiles).unwrap().api_token, Some(true));

        let none = Change::SignedIn {
            url: URL.to_owned(),
            user_id: None,
            api_token: Some(true),
            login: None,
        };
        profiles.apply(none).unwrap();
        assert_eq!(user(&profiles), None);
    }

    #[test]
    fn username_changes_a_signed_in_account_only() {
        let mut profiles = signed_in();
        let change = |username: &str| Change::Username {
            url: URL.to_owned(),
            username: Some(username.to_owned()),
        };
        profiles.apply(change("Renamed")).unwrap();
        assert_eq!(
            user(&profiles).unwrap().username.as_deref(),
            Some("Renamed")
        );
        profiles
            .apply(Change::SignedOut {
                url: URL.to_owned(),
            })
            .unwrap();
        profiles.apply(change("Ghost")).unwrap();
        assert_eq!(user(&profiles), None);
    }

    #[test]
    fn signed_out_forgets_the_account_but_keeps_the_login() {
        let mut profiles = signed_in();
        profiles
            .apply(Change::SignedOut {
                url: URL.to_owned(),
            })
            .unwrap();
        let server = profiles.server(URL).unwrap();
        assert_eq!(server.user, None);
        assert_eq!(server.login.as_deref(), Some("player"));
    }

    #[test]
    fn changes_of_unlisted_servers_do_nothing() {
        let mut profiles = Profiles::default();
        for change in [
            Change::SignedIn {
                url: URL.to_owned(),
                user_id: Some("u1".to_owned()),
                api_token: Some(true),
                login: None,
            },
            Change::Username {
                url: URL.to_owned(),
                username: None,
            },
            Change::SignedOut {
                url: URL.to_owned(),
            },
            Change::RemoveServer {
                url: URL.to_owned(),
            },
        ] {
            assert_eq!(profiles.apply(change), Ok(None));
        }
        assert_eq!(profiles, Profiles::default());
    }

    #[test]
    fn changes_round_trip_through_json() {
        let changes = [
            Change::SignedIn {
                url: URL.to_owned(),
                user_id: Some("u1".to_owned()),
                api_token: Some(false),
                login: Some("player".to_owned()),
            },
            Change::AddServer { server: local() },
        ];
        for change in changes {
            let json = serde_json::to_string(&change).unwrap();
            assert_eq!(serde_json::from_str::<Change>(&json).unwrap(), change);
        }
        assert_eq!(
            serde_json::to_string(&Change::SignedOut {
                url: URL.to_owned()
            })
            .unwrap(),
            r#"{"change":"signed_out","url":"http://localhost:21025/"}"#
        );
    }
}
