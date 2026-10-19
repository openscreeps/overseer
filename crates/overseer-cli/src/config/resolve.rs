//! Finds the server of a profile. The `server` of a profile names a
//! `[servers.NAME]` table of the file first. If the file has no such table,
//! it names a saved server: by its name, or by its URL.

use std::collections::BTreeMap;
use std::path::Path;

use overseer_storage::profiles::Profiles;

use super::name::ServerName;
use super::server::{FileServer, Server};
use super::{Error, Profile, ProfileName};

/// The server of the profile `name` (`profile`) of the file `path`: the
/// table of `files` with the name, or else the server of `profiles` with
/// the name or the URL.
///
/// # Errors
///
/// Returns an error when no server matches, when two saved servers have the
/// name, or when the saved URL is not valid.
pub(super) fn server(
    files: &BTreeMap<ServerName, FileServer>,
    profiles: &Profiles,
    path: &Path,
    name: &ProfileName,
    profile: &Profile,
) -> Result<Server, Error> {
    let key = &profile.server;
    if let Some((file_name, file)) = files.get_key_value(key.as_str()) {
        return Ok(Server::of_file(file_name, file, profiles));
    }
    match profiles.find(key.as_str()) {
        Ok(Some(entry)) => Server::of_saved(entry, profiles).map_err(|source| Error::SavedUrl {
            path: path.to_owned(),
            profile: name.clone(),
            source,
        }),
        Ok(None) => Err(Error::UnknownServer {
            path: path.to_owned(),
            profile: name.clone(),
            server: key.clone(),
            servers: files.keys().cloned().collect(),
            saved: profiles.servers.iter().map(|s| s.name.clone()).collect(),
        }),
        Err(source) => Err(Error::AmbiguousServer {
            path: path.to_owned(),
            profile: name.clone(),
            source,
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use overseer::model::platform::ServerKind as Contract;
    use overseer_storage::profiles::{
        SEASON_URL, SavedUser, Server as SavedServer, ServerKind, server_password_key, token_key,
    };

    use super::super::secret::{Secret, SecretError, Sensitive};
    use super::super::server::{Auth, SavedToken};
    use super::super::{Config, Error, FILE_NAME, ProfileName};
    use crate::envfile::EnvFile;
    use crate::saved::testing::TempSaved;

    /// The URL of the saved private server of the tests.
    const LOCAL: &str = "http://127.0.0.1:21025/";

    /// A storage with the private server `local` at [`LOCAL`] and the
    /// seasonal server, both signed in. `local` has a session token and a
    /// server password, the seasonal server an API token.
    fn signed_in(name: &str) -> TempSaved {
        let saved = TempSaved::new(name);
        saved
            .update(|profiles| {
                let mut local =
                    SavedServer::new("local".to_owned(), LOCAL.to_owned(), ServerKind::Private);
                local.user = Some(SavedUser {
                    id: "u1".to_owned(),
                    username: Some("Me".to_owned()),
                    api_token: Some(false),
                });
                profiles.add(local).expect("a new server");
                let mut season = SavedServer::new(
                    "Season".to_owned(),
                    SEASON_URL.to_owned(),
                    ServerKind::Season,
                );
                season.user = Some(SavedUser {
                    id: "u2".to_owned(),
                    username: None,
                    api_token: Some(true),
                });
                profiles.add(season).expect("a new server");
            })
            .expect("the storage writes");
        for (key, value) in [
            (token_key(LOCAL), "local-token"),
            (server_password_key(LOCAL), "local-server-password"),
            (token_key(SEASON_URL), "season-token"),
        ] {
            saved
                .set_secret(&key, &Sensitive::new(value.to_owned()))
                .expect("the secret saves");
        }
        saved
    }

    /// Parses `text` with the saved servers of `saved`.
    fn parse(text: &str, saved: &TempSaved) -> Result<Config, Error> {
        Config::parse(text, PathBuf::from(FILE_NAME), saved)
    }

    /// The server of the profile `p` of `config`.
    fn server(config: &Config) -> &super::Server {
        config
            .select(&["p".parse::<ProfileName>().expect("a name")])
            .expect("the profile")[0]
            .server
    }

    /// The token and the server password that the CLI sends for the
    /// profile `p` of `config`.
    fn sent(config: &Config, saved: &TempSaved) -> (String, Option<String>) {
        let credentials = server(config)
            .credentials(&EnvFile::default(), saved)
            .expect("credentials");
        let token = match credentials.auth {
            overseer::Auth::Token(token) => token.as_str().to_owned(),
            other => panic!("not a token: {other:?}"),
        };
        (
            token,
            credentials
                .server_password
                .map(|password| password.expose().to_owned()),
        )
    }

    /// A profile names a saved server by its name or by its URL. The CLI
    /// sends the saved token and the saved server password, and forces the
    /// official contract for screeps.com entries.
    #[test]
    fn saved_server_by_name_or_url() {
        let saved = signed_in("by-name-or-url");
        for key in ["local", "http://127.0.0.1:21025", "127.0.0.1:21025"] {
            let config = parse(&format!("[profiles.p]\nserver = \"{key}\"\n"), &saved)
                .expect("a valid configuration");
            let local = server(&config);
            assert_eq!(local.url.to_string(), LOCAL, "{key}");
            assert_eq!(local.auth, Auth::Saved(SavedToken::Session));
            assert_eq!(local.kind, None);
            assert_eq!(local.login, "local");
            assert_eq!(
                sent(&config, &saved),
                (
                    "local-token".to_owned(),
                    Some("local-server-password".to_owned())
                )
            );
        }
        let config = parse("[profiles.p]\nserver = \"Season\"\n", &saved).expect("valid");
        let season = server(&config);
        assert_eq!(season.auth, Auth::Saved(SavedToken::Api));
        assert_eq!(season.kind, Some(Contract::Official));
        assert_eq!(sent(&config, &saved), ("season-token".to_owned(), None));
    }

    /// A `[servers.NAME]` table wins over a saved server with that name. A
    /// table without credentials uses the saved sign-in of its URL. The
    /// credentials of the file win over the saved sign-in.
    #[test]
    fn file_first_then_saved_credentials() {
        let saved = signed_in("file-first");
        let without = "[servers.local]\nurl = \"http://127.0.0.1:21025\"\n\
                       [profiles.p]\nserver = \"local\"\n";
        let config = parse(without, &saved).expect("valid");
        assert_eq!(server(&config).auth, Auth::Saved(SavedToken::Session));
        assert_eq!(
            sent(&config, &saved),
            (
                "local-token".to_owned(),
                Some("local-server-password".to_owned())
            )
        );

        let with = "[servers.local]\nurl = \"http://127.0.0.1:21025\"\ntoken = \"file-token\"\n\
                    [profiles.p]\nserver = \"local\"\n";
        let config = parse(with, &saved).expect("valid");
        assert_eq!(
            server(&config).auth,
            Auth::Token(Secret::Value(Sensitive::new("file-token".to_owned())))
        );
        assert_eq!(
            sent(&config, &saved),
            (
                "file-token".to_owned(),
                Some("local-server-password".to_owned())
            )
        );

        // The table `local` points to another URL: the saved `local` does not apply.
        let other = "[servers.local]\nurl = \"http://other:21025\"\n\
                     [profiles.p]\nserver = \"local\"\n";
        let config = parse(other, &saved).expect("valid");
        assert_eq!(server(&config).auth, Auth::Missing);
    }

    /// Two profiles of one saved server use one account, also when one names
    /// it by its name and the other by its URL.
    #[test]
    fn one_account_by_name_and_url() {
        let saved = signed_in("one-account");
        let text = "[profiles.a]\nserver = \"local\"\n[profiles.a.spawn]\nselector = [\"s\"]\n\
                    [profiles.b]\nserver = \"http://127.0.0.1:21025/\"\n\
                    [profiles.b.spawn]\nselector = [\"s\"]\n";
        let config = parse(text, &saved).expect("valid");
        assert!(matches!(
            config.select_spawn(&[]),
            Err(Error::SharedServer { .. })
        ));
    }

    /// A name of two saved servers is an error that lists their URLs. The URL
    /// still finds each of them.
    #[test]
    fn ambiguous_name() {
        let saved = signed_in("ambiguous");
        saved
            .update(|profiles| {
                profiles
                    .add(SavedServer::new(
                        "local".to_owned(),
                        "http://second:21025".to_owned(),
                        ServerKind::Private,
                    ))
                    .expect("a new server");
            })
            .expect("the storage writes");
        let error = parse("[profiles.p]\nserver = \"local\"\n", &saved).expect_err("ambiguous");
        assert!(
            error.to_string().contains(
                "profile p: 2 saved servers have the name local: http://127.0.0.1:21025/, http://second:21025/"
            ),
            "{error}"
        );
        let config = parse("[profiles.p]\nserver = \"http://second:21025\"\n", &saved)
            .expect("the URL is unique");
        assert_eq!(server(&config).url.to_string(), "http://second:21025/");
    }

    /// Without credentials, the error names `overseer login`. `profiles`
    /// still lists the server.
    #[test]
    fn missing_credentials() {
        let saved = signed_in("missing");
        saved
            .update(|profiles| {
                profiles
                    .add(SavedServer::new(
                        "fresh".to_owned(),
                        "http://fresh:21025".to_owned(),
                        ServerKind::Private,
                    ))
                    .expect("a new server");
            })
            .expect("the storage writes");
        let env = EnvFile::default();
        let error = |text: &str| {
            server(&parse(text, &saved).expect("valid"))
                .credentials(&env, &saved)
                .expect_err("no credentials")
        };
        assert_eq!(
            error("[profiles.p]\nserver = \"fresh\"\n"),
            SecretError::NotSignedIn("fresh".to_owned())
        );
        let unsaved =
            error("[servers.x]\nurl = \"http://unsaved:21025\"\n[profiles.p]\nserver = \"x\"\n");
        assert_eq!(
            unsaved,
            SecretError::NotSignedIn("http://unsaved:21025/".to_owned())
        );
        assert!(
            unsaved
                .to_string()
                .contains("overseer login http://unsaved:21025/ --token"),
            "{unsaved}"
        );

        saved
            .delete_secret(&token_key(LOCAL))
            .expect("the secret deletes");
        let lost = error("[profiles.p]\nserver = \"local\"\n");
        assert_eq!(lost, SecretError::NoSavedToken("local".to_owned()));
        assert!(lost.to_string().contains("overseer login local"), "{lost}");

        let unknown = parse("[profiles.p]\nserver = \"nowhere\"\n", &saved).expect_err("unknown");
        assert!(
            unknown.to_string().contains(
                "no server nowhere; servers: none; saved servers: screeps.com, local, Season, fresh"
            ),
            "{unknown}"
        );
    }

    /// A name with a space is no argument for `overseer login`: the hint
    /// gives the URL.
    #[test]
    fn login_hint_with_a_space() {
        let saved = TempSaved::new("hint");
        saved
            .update(|profiles| {
                profiles
                    .add(SavedServer::new(
                        "My server".to_owned(),
                        LOCAL.to_owned(),
                        ServerKind::Private,
                    ))
                    .expect("a new server");
            })
            .expect("the storage writes");
        let config = parse("[profiles.p]\nserver = \"My server\"\n", &saved).expect("valid");
        assert_eq!(server(&config).login, LOCAL);
    }
}
