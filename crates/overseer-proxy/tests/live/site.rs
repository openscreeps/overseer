//! What the proxy answers itself (static files, discovery, unknown paths) and its CORS policy,
//! whatever the server's own.

use http::StatusCode;
use overseer_net::serde_json::{self, json};

use crate::support::{self, TempDir, header};

const ALLOWED: &str = "http://allowed.example";
const DENIED: &str = "http://denied.example";
const INDEX: &str = "<!doctype html><title>overseer live</title>";

/// Whether `response` carries no `Access-Control-*` header.
fn no_cors(response: &reqwest::Response) -> bool {
    response
        .headers()
        .keys()
        .all(|name| !name.as_str().starts_with("access-control-"))
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn static_files_discovery_and_unknown_paths() {
    let site = TempDir::new("static");
    site.write("index.html", INDEX);
    let proxy = support::start(Some(site.0.clone()), &[]).await;
    let http = support::http();
    let get = |path: &str| http.get(proxy.url().join(path).expect("a URL")).send();

    let index = get("").await.expect("the index");
    assert_eq!(index.status(), StatusCode::OK);
    assert_eq!(header(&index, "content-type"), Some("text/html"));
    assert_eq!(index.text().await.expect("a body"), INDEX);

    let discovery = get("overseer-proxy.json").await.expect("discovery");
    assert_eq!(discovery.status(), StatusCode::OK);
    let document: serde_json::Value =
        serde_json::from_str(&discovery.text().await.expect("a body")).expect("JSON");
    assert_eq!(
        document,
        json!({ "upstream": support::upstream().as_str() })
    );

    // Forwarded paths reach the server even with a static directory.
    let version = get("api/version").await.expect("the version");
    assert_eq!(version.status(), StatusCode::OK);
    let version: serde_json::Value =
        serde_json::from_str(&version.text().await.expect("a body")).expect("JSON");
    assert_eq!(version["ok"], json!(1));

    // The server's own non-API paths are never forwarded (see `support::upstream_own_path`).
    let own = support::upstream_own_path();
    let upstream_own = support::http()
        .get(support::upstream().join(own).expect("a URL"))
        .send()
        .await
        .expect("the upstream's own path");
    assert_eq!(upstream_own.status(), StatusCode::OK);
    assert_eq!(
        get(own).await.expect("an answer").status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get("missing.js").await.expect("an answer").status(),
        StatusCode::NOT_FOUND
    );
    proxy.shutdown().await;

    // Without a static directory everything but the forwarded paths is 404, the server's root
    // page included.
    let proxy = support::start(None, &[]).await;
    let get = |path: &str| http.get(proxy.url().join(path).expect("a URL")).send();
    assert_eq!(
        get("").await.expect("an answer").status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(own).await.expect("an answer").status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get("overseer-proxy.json")
            .await
            .expect("discovery")
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        get("api/version").await.expect("the version").status(),
        StatusCode::OK
    );
    proxy.shutdown().await;
}

/// The proxy sends CORS headers to the origins it allows and to no other, whatever the server
/// sends: `openscreeps/server` allows every origin, a community server none
/// (screepsmod-admin-utils 1.37.1 sends CORS headers only to the origins its `cors` setting
/// lists, none by default: `lib/services/cors/backend.js`,
/// <https://unpkg.com/screepsmod-admin-utils@1.37.1/lib/services/cors/backend.js>).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn cors_headers_are_sent_to_allowed_origins_only() {
    let proxy = support::start(None, &[ALLOWED]).await;
    let http = support::http();
    let version = proxy.url().join("api/version").expect("a URL");

    // The server's own policy; the proxy decides on its own.
    let upstream = http
        .get(support::upstream().join("api/version").expect("a URL"))
        .header("Origin", DENIED)
        .send()
        .await
        .expect("the upstream version");
    if support::community() {
        assert!(no_cors(&upstream), "{:?}", upstream.headers());
    } else {
        assert_eq!(
            header(&upstream, "access-control-allow-origin"),
            Some(DENIED)
        );
    }

    let allowed = http
        .get(version.clone())
        .header("Origin", ALLOWED)
        .send()
        .await
        .expect("the version");
    assert_eq!(allowed.status(), StatusCode::OK);
    assert_eq!(
        header(&allowed, "access-control-allow-origin"),
        Some(ALLOWED)
    );
    assert_eq!(
        header(&allowed, "access-control-expose-headers"),
        Some("X-Token, X-RateLimit-Limit, X-RateLimit-Remaining, X-RateLimit-Reset")
    );
    let denied = http
        .get(version.clone())
        .header("Origin", DENIED)
        .send()
        .await
        .expect("the version");
    assert_eq!(denied.status(), StatusCode::OK);
    assert!(no_cors(&denied), "{:?}", denied.headers());
    let native = http.get(version).send().await.expect("the version");
    assert!(no_cors(&native), "{:?}", native.headers());

    // Preflights are answered by the proxy: allowed with the permitted headers, others refused.
    let me = proxy.url().join("api/auth/me").expect("a URL");
    let preflight = |origin: &'static str| {
        http.request(reqwest::Method::OPTIONS, me.clone())
            .header("Origin", origin)
            .header("Access-Control-Request-Method", "GET")
            .header("Access-Control-Request-Headers", "x-token")
            .send()
    };
    let allowed = preflight(ALLOWED).await.expect("a preflight answer");
    assert_eq!(allowed.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        header(&allowed, "access-control-allow-origin"),
        Some(ALLOWED)
    );
    assert_eq!(
        header(&allowed, "access-control-allow-headers"),
        Some("X-Token, X-Username, X-Server-Password, Content-Type")
    );
    let denied = preflight(DENIED).await.expect("a preflight answer");
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert!(no_cors(&denied), "{:?}", denied.headers());

    // An allowed page reads the rotated token of an authenticated reply.
    let account = support::register_with(&support::via(&proxy), None).await;
    let client = account.client(&support::via(&proxy));
    client.world().await.expect("sign-in");
    let token = client.token().expect("a token");
    let authenticated = http
        .get(me)
        .header("Origin", ALLOWED)
        .header("X-Token", token.as_str())
        .send()
        .await
        .expect("auth/me");
    assert_eq!(authenticated.status(), StatusCode::OK);
    assert_eq!(
        header(&authenticated, "access-control-allow-origin"),
        Some(ALLOWED)
    );
    let rotated = header(&authenticated, "x-token").expect("a rotated token");
    assert_ne!(rotated, token.as_str());
    assert_eq!(rotated.len(), support::token_length());
    proxy.shutdown().await;
}
