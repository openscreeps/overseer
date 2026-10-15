//! HTTP API: typed endpoints, request building and reply interpretation.
//!
//! An [`Endpoint`] describes one operation of the Screeps HTTP API; [`endpoints`] holds one per
//! operation of the `openscreeps/openapi` reference and [`types`] its schemas, both generated
//! by `cargo xtask codegen` from `reference/`. A [`Session`] holds the server address, the
//! current token and an optional server password; it turns an endpoint into an
//! [`::http::Request`] for the driver to send, and turns the driver's [`::http::Response`] into
//! the endpoint's response type or an [`ApiError`]. Logical failures arrive as `{error}`
//! bodies, usually with HTTP 200; success bodies carry a truthy `ok`.

pub mod endpoints;
#[cfg(test)]
mod fixtures;
mod indexed_list;
mod memory;
mod room_object;
mod session;
mod terrain;
pub mod types;
mod version;

use std::fmt;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use url::UrlQuery;
use url::form_urlencoded::Serializer;

pub use indexed_list::IndexedList;
pub use memory::{MemoryError, MemoryValue};
pub use session::{ApiError, RateLimit, Reply, RequestError, Session};
pub use terrain::{Terrain, TerrainError, Tile};

/// Whether an endpoint needs a token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Auth {
    /// Public endpoint; a held token is still sent.
    None,
    /// Public endpoint whose reply depends on the account when a token is sent.
    Optional,
    /// Building the request fails with [`RequestError::MissingToken`] without a token.
    Required,
}

/// How the success body of an endpoint is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// JSON with the API envelope: an `{error}` body is a failure and success needs a truthy
    /// `ok`.
    Envelope,
    /// A JSON document without `ok`, such as a room-history chunk; an `{error}` body is still a
    /// failure.
    Json,
    /// UTF-8 text, such as an SVG image; the response type is `String`.
    Text,
    /// No documented body: only the status (and a JSON `{error}` body) is checked; the response
    /// type is `()`.
    Empty,
}

/// One operation of the HTTP API.
pub trait Endpoint {
    /// The success body; unknown fields are ignored.
    type Response: serde::de::DeserializeOwned;
    /// The HTTP method.
    const METHOD: ::http::Method;
    /// The path template relative to the server base, for example `api/game/time` or
    /// `room-history/{shard}/{room}/{tick}.json`.
    const PATH: &'static str;
    /// Whether a token is needed.
    const AUTH: Auth;
    /// How the success body is read.
    const FORMAT: Format;

    /// Writes the path relative to the server base. The default writes [`Self::PATH`], which
    /// is right for templates without parameters.
    fn path(&self, path: &mut Path) {
        path.literal(Self::PATH);
    }

    /// Appends the query parameters. The default appends none.
    fn query(&self, query: &mut Query<'_>) {
        let _ = query;
    }

    /// The JSON request body. The default sends no body.
    ///
    /// # Errors
    ///
    /// Returns the [`serde_json::Error`] of a body that does not serialize.
    fn body(&self) -> Result<Option<Vec<u8>>, serde_json::Error> {
        Ok(None)
    }
}

/// Characters escaped in a path segment: everything but `A-Z a-z 0-9 - . _ ~`.
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// The path of a request being built, relative to the server base.
#[derive(Debug, Default)]
pub struct Path {
    path: String,
}

impl Path {
    /// Appends `text` verbatim.
    pub fn literal(&mut self, text: &str) {
        self.path.push_str(text);
    }

    /// Appends `value` as one percent-encoded path segment, so `/`, `?`, `#`, `%` and spaces in
    /// a parameter cannot change the path's structure.
    pub fn segment(&mut self, value: &str) {
        self.path.extend(utf8_percent_encode(value, SEGMENT));
    }

    /// The path built so far.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.path
    }
}

/// The query string of a request being built, encoded as `application/x-www-form-urlencoded`.
pub struct Query<'a> {
    serializer: Serializer<'a, UrlQuery<'a>>,
}

impl Query<'_> {
    /// Appends `name=value`.
    pub fn pair(&mut self, name: &str, value: &str) {
        self.serializer.append_pair(name, value);
    }

    /// Appends `name=value` when `value` is present.
    pub fn optional(&mut self, name: &str, value: Option<&str>) {
        if let Some(value) = value {
            self.pair(name, value);
        }
    }
}

impl fmt::Debug for Query<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Query").finish_non_exhaustive()
    }
}

/// Stands in for a secret in `Debug` output.
struct Redacted;

impl fmt::Debug for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

#[cfg(test)]
mod tests {
    use super::Path;

    #[test]
    fn path_segments_are_percent_encoded() {
        let mut path = Path::default();
        path.literal("room-history/");
        path.segment("shard 0/../x?y#z%");
        path.literal("/");
        path.segment("W1N1-é_~.");
        assert_eq!(
            path.as_str(),
            "room-history/shard%200%2F..%2Fx%3Fy%23z%25/W1N1-%C3%A9_~."
        );
    }
}
