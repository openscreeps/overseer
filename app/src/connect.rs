//! Which address the app uses to reach a server.
//!
//! - In the Tauri shell every server goes through the shell's in-process `overseer-proxy`
//!   (command `proxy_url`), so all servers behave the same whatever CORS they send.
//! - In a browser the app connects directly, except to the server fronted by the
//!   `overseer-proxy` that serves the page itself (`GET /overseer-proxy.json` names its
//!   upstream): that one is reached through the page's own origin.

use serde::Deserialize;
use url::Url;

/// The `overseer-proxy` that serves the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageProxy {
    /// The page's origin with path `/`: the proxy's base URL.
    pub origin: Url,
    /// The server it forwards to.
    pub upstream: Url,
}

impl PageProxy {
    /// The proxy serving the page at `page_origin` (`scheme://host[:port]`), from the body of
    /// its `GET /overseer-proxy.json`; `None` unless both parse.
    pub fn parse(page_origin: &str, discovery: &str) -> Option<Self> {
        #[derive(Deserialize)]
        struct Discovery {
            upstream: String,
        }
        let discovery: Discovery = serde_json::from_str(discovery).ok()?;
        let origin = Url::parse(page_origin).ok()?.join("/").ok()?;
        let upstream = base(&discovery.upstream)?;
        Some(Self { origin, upstream })
    }
}

/// How to reach a server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectTarget {
    /// Through the Tauri shell's proxy for the server (ask it with `proxy_url`).
    TauriProxy,
    /// The server's own address.
    Direct(Url),
    /// The page's origin, whose proxy forwards to the server.
    PageOrigin(Url),
}

/// Where to connect to `server` (a base URL) from the Tauri shell (`tauri`) or a page served
/// by `page_proxy`.
pub fn connect_target(tauri: bool, page_proxy: Option<&PageProxy>, server: &Url) -> ConnectTarget {
    if tauri {
        return ConnectTarget::TauriProxy;
    }
    match (page_proxy, base(server.as_str())) {
        (Some(proxy), Some(server)) if proxy.upstream == server => {
            ConnectTarget::PageOrigin(proxy.origin.clone())
        }
        _ => ConnectTarget::Direct(server.clone()),
    }
}

/// `input` as a base URL with a trailing `/` in its path, for comparisons.
fn base(input: &str) -> Option<Url> {
    let mut url = Url::parse(input).ok()?;
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    url.set_query(None);
    url.set_fragment(None);
    Some(url)
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::{ConnectTarget, PageProxy, connect_target};

    fn url(input: &str) -> Url {
        Url::parse(input).unwrap()
    }

    #[test]
    fn tauri_always_uses_its_proxy() {
        let proxy = PageProxy::parse(
            "http://127.0.0.1:8080",
            r#"{"upstream":"https://screeps.com/"}"#,
        );
        for server in ["https://screeps.com/", "http://localhost:21025/"] {
            assert_eq!(
                connect_target(true, proxy.as_ref(), &url(server)),
                ConnectTarget::TauriProxy
            );
        }
    }

    #[test]
    fn web_connects_directly_without_a_page_proxy() {
        let server = url("http://localhost:21025/");
        assert_eq!(
            connect_target(false, None, &server),
            ConnectTarget::Direct(server.clone())
        );
    }

    #[test]
    fn web_uses_the_page_origin_for_the_proxied_server_only() {
        let proxy = PageProxy::parse(
            "http://127.0.0.1:8080",
            r#"{"upstream":"https://SCREEPS.com:443/season"}"#,
        )
        .unwrap();
        assert_eq!(proxy.origin, url("http://127.0.0.1:8080/"));
        assert_eq!(
            connect_target(false, Some(&proxy), &url("https://screeps.com/season/")),
            ConnectTarget::PageOrigin(url("http://127.0.0.1:8080/"))
        );
        let other = url("https://screeps.com/");
        assert_eq!(
            connect_target(false, Some(&proxy), &other),
            ConnectTarget::Direct(other.clone())
        );
    }

    #[test]
    fn discovery_needs_a_proxy_document() {
        for body in [
            "",
            "<!doctype html><html></html>",
            "{}",
            r#"{"upstream":"nope"}"#,
        ] {
            assert_eq!(
                PageProxy::parse("http://127.0.0.1:1420", body),
                None,
                "{body}"
            );
        }
        assert_eq!(
            PageProxy::parse("not an origin", r#"{"upstream":"http://x/"}"#),
            None
        );
    }
}
