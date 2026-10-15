//! The `proxy_url` command: one in-process `overseer-proxy` per server, on loopback.
//!
//! The webview cannot call servers whose HTTP API sends no CORS headers (screeps.com), so
//! the UI reaches every server through a proxy started here; the proxy only answers the app's
//! own origins.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};

use overseer_proxy::{Proxy, ProxyConfig};
use tokio::sync::Mutex;
use url::Url;

/// Origins of the app's pages: the bundled app on Windows/Android (`http(s)://tauri.localhost`)
/// and elsewhere (`tauri://localhost`), and the development server (`trunk serve`).
const APP_ORIGINS: [&str; 5] = [
    "http://tauri.localhost",
    "https://tauri.localhost",
    "tauri://localhost",
    "http://127.0.0.1:1420",
    "http://localhost:1420",
];

/// The running proxies by upstream base URL.
#[derive(Default)]
pub struct Proxies {
    running: Mutex<HashMap<Url, Proxy>>,
}

impl Proxies {
    /// The URL of the proxy forwarding to `upstream`, started on first use (one per base URL).
    async fn url(&self, upstream: &str) -> Result<String, String> {
        let upstream = upstream_base(upstream)?;
        let mut running = self.running.lock().await;
        if let Some(proxy) = running.get(&upstream) {
            return Ok(proxy.url().into());
        }
        let proxy = Proxy::start(ProxyConfig {
            upstream: upstream.clone(),
            listen: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            static_dir: None,
            allowed_origins: APP_ORIGINS.map(str::to_owned).to_vec(),
        })
        .await
        .map_err(|error| error.to_string())?;
        let url = proxy.url().into();
        running.insert(upstream, proxy);
        Ok(url)
    }

    /// Stops every proxy.
    pub async fn shutdown(&self) {
        let proxies: Vec<_> = self
            .running
            .lock()
            .await
            .drain()
            .map(|(_, proxy)| proxy)
            .collect();
        for proxy in proxies {
            proxy.shutdown().await;
        }
    }
}

/// The URL of the proxy forwarding to `upstream` (a server's base URL), started on first use.
///
/// # Errors
///
/// Returns a message when `upstream` is not an http(s) URL or the proxy cannot start.
#[tauri::command]
pub async fn proxy_url(
    upstream: String,
    proxies: tauri::State<'_, Proxies>,
) -> Result<String, String> {
    proxies.url(&upstream).await
}

/// `input` as an http(s) base URL with a trailing `/`: the key of its proxy.
fn upstream_base(input: &str) -> Result<Url, String> {
    let mut url = Url::parse(input).map_err(|error| format!("invalid server URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("invalid server URL: {input} is not http(s)"));
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::{Proxies, upstream_base};

    #[test]
    fn upstreams_are_keyed_by_their_base() {
        assert_eq!(
            upstream_base("https://SCREEPS.com/season").unwrap(),
            upstream_base("https://screeps.com:443/season/").unwrap()
        );
        assert!(upstream_base("ftp://screeps.com/").is_err());
        assert!(upstream_base("screeps.com").is_err());
    }

    #[test]
    fn one_loopback_proxy_per_server() {
        tauri::async_runtime::block_on(async {
            let proxies = Proxies::default();
            let a = proxies.url("http://127.0.0.1:9/a").await.unwrap();
            assert!(a.starts_with("http://127.0.0.1:"), "{a}");
            assert_eq!(proxies.url("http://127.0.0.1:9/a/").await.unwrap(), a);
            let b = proxies.url("http://127.0.0.1:9/b/").await.unwrap();
            assert_ne!(a, b);
            assert!(proxies.url("ftp://127.0.0.1/").await.is_err());
            proxies.shutdown().await;
            assert!(proxies.running.lock().await.is_empty());
        });
    }
}
