//! The route slugs of the saved servers ([`crate::route`]). The saved servers themselves are
//! [`overseer_storage::profiles`].

use overseer_storage::profiles::{Profiles, Server};
use url::Url;

/// The route slugs of the saved servers.
pub trait Slugs {
    /// The route slug of the server with base URL `url` (see [`crate::route`]): its
    /// [`base_slug`], and when another saved server has the same one, a suffix: `-http` for a
    /// plain-HTTP URL whose HTTPS twin is listed too, else `-<n>` with `n` its position among
    /// those servers (from 2).
    fn slug(&self, url: &str) -> Option<String>;

    /// The server whose [`Self::slug`] is `slug`.
    fn by_slug(&self, slug: &str) -> Option<&Server>;
}

impl Slugs for Profiles {
    fn slug(&self, url: &str) -> Option<String> {
        self.server(url)?;
        let base = base_slug(url);
        let twins: Vec<&str> = self
            .servers
            .iter()
            .map(|server| server.url.as_str())
            .filter(|other| base_slug(other) == base)
            .collect();
        if twins.len() == 1 {
            return Some(base);
        }
        let without_scheme = |url: &str| url.split_once("://").map(|(_, rest)| rest.to_owned());
        let https_twin = url.starts_with("http://")
            && twins.iter().any(|other| {
                other.starts_with("https://") && without_scheme(other) == without_scheme(url)
            });
        if https_twin {
            return Some(format!("{base}-http"));
        }
        let position = twins.iter().position(|other| *other == url)?;
        Some(if position == 0 {
            base
        } else {
            format!("{base}-{}", position + 1)
        })
    }

    fn by_slug(&self, slug: &str) -> Option<&Server> {
        self.servers
            .iter()
            .find(|server| self.slug(&server.url).as_deref() == Some(slug))
    }
}

/// The slug of a server URL, before disambiguation ([`Slugs::slug`]): its host, port and
/// path segments joined by `-`, lower-case, with every character outside `[a-z0-9.]`
/// replaced by `-` and runs of `-` collapsed (`https://screeps.com/season/` →
/// `screeps.com-season`, `http://localhost:21025/` → `localhost-21025`). It depends on the
/// URL only, so links stay valid across sessions and machines.
pub fn base_slug(url: &str) -> String {
    let Ok(url) = Url::parse(url) else {
        return "server".to_owned();
    };
    let mut parts: Vec<String> = Vec::new();
    parts.extend(url.host_str().map(str::to_owned));
    parts.extend(url.port().map(|port| port.to_string()));
    parts.extend(
        url.path()
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(str::to_owned),
    );
    let mut slug = String::new();
    for character in parts.join("-").chars() {
        let character = character.to_ascii_lowercase();
        let keep = character.is_ascii_lowercase() || character.is_ascii_digit() || character == '.';
        if keep {
            slug.push(character);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "server".to_owned()
    } else {
        slug.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use overseer_storage::profiles::{OFFICIAL_URL, Profiles, SEASON_URL, Server, ServerKind};

    use super::{Slugs, base_slug};

    fn private(url: &str) -> Server {
        Server::new("Local".to_owned(), url.to_owned(), ServerKind::Private)
    }

    #[test]
    fn slugs_are_stable_and_distinct() {
        assert_eq!(base_slug(OFFICIAL_URL), "screeps.com");
        assert_eq!(base_slug(SEASON_URL), "screeps.com-season");
        assert_eq!(base_slug("http://localhost:21025/"), "localhost-21025");
        assert_eq!(base_slug("http://127.0.0.1:21025/"), "127.0.0.1-21025");
        assert_eq!(base_slug("http://My_Host:80/a b/C/"), "my-host-a-20b-c");
        assert_eq!(base_slug("not a url"), "server");

        let mut profiles = Profiles::default();
        for url in [
            "http://screeps.com/",
            "http://a.example/x-y/",
            "http://a.example/x/y/",
            SEASON_URL,
        ] {
            profiles.add(private(url)).unwrap();
        }
        let slugs: Vec<_> = profiles
            .servers
            .iter()
            .map(|server| profiles.slug(&server.url).unwrap())
            .collect();
        assert_eq!(
            slugs,
            [
                "screeps.com",
                "screeps.com-http",
                "a.example-x-y",
                "a.example-x-y-2",
                "screeps.com-season"
            ]
        );
        for (server, slug) in profiles.servers.iter().zip(&slugs) {
            assert_eq!(profiles.by_slug(slug), Some(server));
        }
        assert_eq!(profiles.slug("http://unknown/"), None);
        assert_eq!(profiles.by_slug("unknown"), None);
        // Slugs depend on the URL alone, except between servers sharing one.
        profiles.remove("http://a.example/x-y/");
        assert_eq!(
            profiles.slug(SEASON_URL).as_deref(),
            Some("screeps.com-season")
        );
        assert_eq!(
            profiles.slug("https://screeps.com/").as_deref(),
            Some("screeps.com")
        );
    }
}
