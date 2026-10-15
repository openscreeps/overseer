//! Origin checks and CORS headers.

use std::collections::HashSet;

use http::header::{
    ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
    ACCESS_CONTROL_EXPOSE_HEADERS, ACCESS_CONTROL_MAX_AGE, HOST, ORIGIN, VARY,
};
use http::{HeaderMap, HeaderValue, Response};
use url::Url;

use crate::ProxyError;

/// The request headers a cross-origin page may send.
const ALLOW_HEADERS: &str = "X-Token, X-Username, X-Server-Password, Content-Type";

/// The response headers a cross-origin page may read.
const EXPOSE_HEADERS: &str = "X-Token, X-RateLimit-Limit, X-RateLimit-Remaining, X-RateLimit-Reset";

/// The methods a cross-origin page may use.
const ALLOW_METHODS: &str = "GET, HEAD, POST, PUT, PATCH, DELETE";

/// How long browsers may cache a preflight answer, in seconds.
const MAX_AGE: &str = "600";

/// Chromium's Private Network Access preflight: a public page asking for a loopback server.
const REQUEST_PRIVATE_NETWORK: &str = "access-control-request-private-network";
const ALLOW_PRIVATE_NETWORK: &str = "access-control-allow-private-network";

/// The configured cross-origin allow-list, as `scheme://host[:port]`.
#[derive(Debug)]
pub(crate) struct Origins(HashSet<String>);

/// How a request's `Origin` relates to the configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    /// No `Origin` header: a native client or a same-origin navigation.
    Absent,
    /// A configured origin (the header as sent); responses carry CORS headers for it.
    Allowed(HeaderValue),
    /// The proxy's own origin: `Origin` names the `Host` the request was sent to.
    Same,
    /// Any other origin.
    Denied,
}

impl Origins {
    /// Normalizes the configured origins.
    pub(crate) fn new(origins: &[String]) -> Result<Self, ProxyError> {
        origins
            .iter()
            .map(|origin| {
                parse(origin)
                    .map(|parsed| parsed.origin)
                    .ok_or_else(|| ProxyError::Origin(origin.clone()))
            })
            .collect::<Result<_, _>>()
            .map(Self)
    }

    /// Whether no cross-origin page is allowed.
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Classifies the `Origin` of a request with `headers`.
    pub(crate) fn classify(&self, headers: &HeaderMap) -> Origin {
        let Some(value) = headers.get(ORIGIN) else {
            return Origin::Absent;
        };
        let Some(parsed) = value.to_str().ok().and_then(parse) else {
            return Origin::Denied;
        };
        if self.0.contains(&parsed.origin) {
            return Origin::Allowed(value.clone());
        }
        let host = headers.get(HOST).and_then(|host| host.to_str().ok());
        if parsed.web && host.is_some_and(|host| host.eq_ignore_ascii_case(&parsed.authority)) {
            Origin::Same
        } else {
            Origin::Denied
        }
    }
}

/// A parsed origin.
struct Parsed {
    /// `scheme://host[:port]`, lowercase, without a default port.
    origin: String,
    /// `host[:port]`, comparable to a `Host` header.
    authority: String,
    /// Whether the scheme is http or https (the schemes a `Host` header can belong to).
    web: bool,
}

/// Parses `scheme://host[:port]` (a final `/` is tolerated).
fn parse(origin: &str) -> Option<Parsed> {
    let url = Url::parse(origin).ok()?;
    let host = url.host_str()?;
    let bare = url.username().is_empty()
        && url.password().is_none()
        && matches!(url.path(), "" | "/")
        && url.query().is_none()
        && url.fragment().is_none();
    if !bare {
        return None;
    }
    let authority = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    };
    Some(Parsed {
        origin: format!("{}://{authority}", url.scheme()),
        web: matches!(url.scheme(), "http" | "https"),
        authority,
    })
}

/// Adds the CORS headers of `origin` to a response. `vary` marks responses that differ by
/// origin (some origins are allowed).
pub(crate) fn decorate<B>(response: &mut Response<B>, origin: &Origin, vary: bool) {
    let headers = response.headers_mut();
    if vary {
        headers.append(VARY, HeaderValue::from_static("Origin"));
    }
    if let Origin::Allowed(value) = origin {
        headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, value.clone());
        headers.insert(
            ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static(EXPOSE_HEADERS),
        );
    }
}

/// Adds the preflight answer for an allowed origin (`Access-Control-Allow-Origin` itself comes
/// from [`decorate`]).
pub(crate) fn allow_preflight<B>(response: &mut Response<B>, request: &HeaderMap) {
    let headers = response.headers_mut();
    headers.insert(
        ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static(ALLOW_METHODS),
    );
    headers.insert(
        ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static(ALLOW_HEADERS),
    );
    headers.insert(ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static(MAX_AGE));
    if request
        .get(REQUEST_PRIVATE_NETWORK)
        .is_some_and(|value| value == "true")
    {
        headers.insert(ALLOW_PRIVATE_NETWORK, HeaderValue::from_static("true"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origins(list: &[&str]) -> Origins {
        Origins::new(
            &list
                .iter()
                .map(|&origin| origin.to_owned())
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        pairs
            .iter()
            .map(|&(name, value)| {
                (
                    http::HeaderName::from_static(name),
                    HeaderValue::from_static(value),
                )
            })
            .collect()
    }

    #[test]
    fn configured_origins_are_normalized() {
        let list = origins(&[
            "HTTP://Tauri.Localhost/",
            "tauri://localhost",
            "http://127.0.0.1:1420",
            "https://example.com:443",
        ]);
        let mut normalized: Vec<_> = list.0.iter().map(String::as_str).collect();
        normalized.sort_unstable();
        assert_eq!(
            normalized,
            [
                "http://127.0.0.1:1420",
                "http://tauri.localhost",
                "https://example.com",
                "tauri://localhost"
            ]
        );
    }

    #[test]
    fn origins_with_paths_or_credentials_are_rejected() {
        for origin in [
            "http://a.example/app",
            "http://u@a.example",
            "a.example",
            "http://a.example/?x",
        ] {
            assert!(
                Origins::new(&[origin.to_owned()]).is_err(),
                "{origin} accepted"
            );
        }
    }

    #[test]
    fn requests_are_classified_by_origin_and_host() {
        let list = origins(&["http://allowed.example", "tauri://localhost"]);
        assert_eq!(list.classify(&headers(&[])), Origin::Absent);
        assert_eq!(
            list.classify(&headers(&[("origin", "http://allowed.example")])),
            Origin::Allowed(HeaderValue::from_static("http://allowed.example"))
        );
        assert_eq!(
            list.classify(&headers(&[("origin", "tauri://localhost")])),
            Origin::Allowed(HeaderValue::from_static("tauri://localhost"))
        );
        assert_eq!(
            list.classify(&headers(&[
                ("origin", "http://127.0.0.1:8080"),
                ("host", "127.0.0.1:8080")
            ])),
            Origin::Same
        );
        assert_eq!(
            list.classify(&headers(&[
                ("origin", "http://evil.example"),
                ("host", "127.0.0.1:8080")
            ])),
            Origin::Denied
        );
        assert_eq!(
            list.classify(&headers(&[("origin", "null"), ("host", "127.0.0.1:8080")])),
            Origin::Denied
        );
        // A custom scheme never counts as the proxy's own origin.
        assert_eq!(
            list.classify(&headers(&[
                ("origin", "app://localhost"),
                ("host", "localhost")
            ])),
            Origin::Denied
        );
    }
}
