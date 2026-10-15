//! Forwarding `api/*`, `room-history/*` and `room-history` requests.

use std::io;

use http::header::CONTENT_LENGTH;
use http::{HeaderMap, Method, Request, Response, StatusCode};
use http_body_util::{BodyExt as _, LengthLimitError, Limited};
use hyper::body::Incoming;

use crate::MAX_REQUEST_BODY;
use crate::serve::{Body, State, retryable, status};

/// The request headers forwarded to the upstream; everything else (cookies included) stays.
const REQUEST_HEADERS: [&str; 5] = [
    "x-token",
    "x-username",
    "x-server-password",
    "content-type",
    "accept",
];

/// The response headers forwarded to the client; everything else (`Location` included, so
/// redirects lead nowhere) stays.
const RESPONSE_HEADERS: [&str; 6] = [
    "x-token",
    "x-ratelimit-limit",
    "x-ratelimit-remaining",
    "x-ratelimit-reset",
    "content-type",
    "content-encoding",
];

/// Forwards `request` to the upstream and relays the answer. A request that lost its pooled
/// connection is sent once more on a new one when that is safe (see [`retryable`]).
pub(crate) async fn forward(state: &State, request: Request<Incoming>) -> Response<Body> {
    let (parts, body) = request.into_parts();
    if !forwarded(&parts.method) {
        return status(StatusCode::METHOD_NOT_ALLOWED);
    }
    let path_and_query = parts.uri.path_and_query().map_or("/", |path| path.as_str());
    let Some(url) = state.upstream_url(path_and_query) else {
        return status(StatusCode::NOT_FOUND);
    };
    if declared_length(&parts.headers).is_some_and(|length| length > MAX_REQUEST_BODY) {
        return status(StatusCode::PAYLOAD_TOO_LARGE);
    }
    let body = match Limited::new(body, MAX_REQUEST_BODY).collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(error) if error.is::<LengthLimitError>() => {
            return status(StatusCode::PAYLOAD_TOO_LARGE);
        }
        Err(_) => return status(StatusCode::BAD_REQUEST),
    };
    let has_body = !body.is_empty()
        || parts.method == Method::POST
        || parts.method == Method::PUT
        || parts.method == Method::PATCH;
    let mut upstream = state.http.request(parts.method, url);
    for name in REQUEST_HEADERS {
        for value in parts.headers.get_all(name) {
            let mut value = value.clone();
            // Keeps tokens and passwords out of any debug output.
            value.set_sensitive(true);
            upstream = upstream.header(name, value);
        }
    }
    if has_body {
        upstream = upstream.body(body);
    }
    let Ok(request) = upstream.build() else {
        return status(StatusCode::BAD_GATEWAY);
    };
    // The body is collected, so the copy for a retry shares its bytes.
    let retry = request.try_clone();
    let response = match (state.http.execute(request).await, retry) {
        (Err(error), Some(retry)) if retryable(retry.method(), &error) => {
            state.fresh_http.execute(retry).await
        }
        (response, _) => response,
    };
    match response {
        Ok(response) => relay(response),
        Err(error) if error.is_timeout() => status(StatusCode::GATEWAY_TIMEOUT),
        Err(_) => status(StatusCode::BAD_GATEWAY),
    }
}

/// Methods forwarded to the upstream (`OPTIONS` is answered locally).
fn forwarded(method: &Method) -> bool {
    [
        Method::GET,
        Method::HEAD,
        Method::POST,
        Method::PUT,
        Method::PATCH,
        Method::DELETE,
    ]
    .contains(method)
}

/// The declared `Content-Length`, if any; lengths beyond `usize` count as `usize::MAX`.
fn declared_length(headers: &HeaderMap) -> Option<usize> {
    let length = headers.get(CONTENT_LENGTH)?.to_str().ok()?;
    Some(length.parse().unwrap_or(usize::MAX))
}

/// The upstream status, the allowed headers and the body as received (still encoded).
fn relay(response: reqwest::Response) -> Response<Body> {
    let (parts, body) = http::Response::from(response).into_parts();
    // Drop the URL from body errors: it may carry a query.
    let body = body
        .map_err(|error: reqwest::Error| io::Error::other(error.without_url()))
        .boxed_unsync();
    let mut relayed = Response::new(body);
    *relayed.status_mut() = parts.status;
    let headers = relayed.headers_mut();
    for name in RESPONSE_HEADERS {
        for value in parts.headers.get_all(name) {
            headers.append(name, value.clone());
        }
    }
    relayed
}
