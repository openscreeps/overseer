//! Request building and reply interpretation.

use std::fmt;

use ::http::header::{ACCEPT, CONTENT_TYPE};
use ::http::{HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode};
use serde::Deserialize;
use serde::de::{self, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};

use super::{Auth, Endpoint, Format, Path, Query, Redacted};
use crate::{ServerUrl, ServerUrlError, Token};

const X_TOKEN: HeaderName = HeaderName::from_static("x-token");
const X_USERNAME: HeaderName = HeaderName::from_static("x-username");
const X_SERVER_PASSWORD: HeaderName = HeaderName::from_static("x-server-password");
const X_RATELIMIT_LIMIT: HeaderName = HeaderName::from_static("x-ratelimit-limit");
const X_RATELIMIT_REMAINING: HeaderName = HeaderName::from_static("x-ratelimit-remaining");
const X_RATELIMIT_RESET: HeaderName = HeaderName::from_static("x-ratelimit-reset");
const APPLICATION_JSON: &str = "application/json";

/// HTTP API state of one account on one server: the server address, the current token and the
/// server password of a password-protected private server.
///
/// `Debug` output redacts the token and the password.
#[derive(Clone)]
pub struct Session {
    server: ServerUrl,
    token: Option<Token>,
    server_password: Option<String>,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("server", &self.server)
            .field("token", &self.token)
            .field(
                "server_password",
                &self.server_password.as_ref().map(|_| Redacted),
            )
            .finish()
    }
}

impl Session {
    /// A session on `server`, authenticated with `token` when given.
    #[must_use]
    pub const fn new(server: ServerUrl, token: Option<Token>) -> Self {
        Self {
            server,
            token,
            server_password: None,
        }
    }

    /// The server.
    #[must_use]
    pub const fn server(&self) -> &ServerUrl {
        &self.server
    }

    /// The current token, including tokens rotated by the server.
    #[must_use]
    pub const fn token(&self) -> Option<&Token> {
        self.token.as_ref()
    }

    /// Replaces the token, for example with the token returned by a sign-in.
    pub fn set_token(&mut self, token: Option<Token>) {
        self.token = token;
    }

    /// The server password sent as `X-Server-Password`, if any.
    #[must_use]
    pub fn server_password(&self) -> Option<&str> {
        self.server_password.as_deref()
    }

    /// Sets or clears the password of a password-protected private server, sent as
    /// `X-Server-Password` with every `api/…` and `room-history` request.
    pub fn set_server_password(&mut self, password: Option<String>) {
        self.server_password = password;
    }

    /// Builds the request for `endpoint`.
    ///
    /// Sends `X-Token` and `X-Username` whenever a token is held, `X-Server-Password` with
    /// `api/…` and `room-history` paths whenever a server password is set, `Accept:
    /// application/json` for JSON endpoints, and `Content-Type: application/json` with a body.
    ///
    /// # Errors
    ///
    /// Returns [`RequestError::MissingToken`] for an [`Auth::Required`] endpoint without a token,
    /// and the other [`RequestError`] variants when the URL, body or request cannot be built.
    pub fn request<E: Endpoint>(&self, endpoint: &E) -> Result<Request<Vec<u8>>, RequestError> {
        if E::AUTH == Auth::Required && self.token.is_none() {
            return Err(RequestError::MissingToken);
        }
        let mut path = Path::default();
        endpoint.path(&mut path);
        let mut url = self.server.join(path.as_str())?;
        endpoint.query(&mut Query {
            serializer: url.query_pairs_mut(),
        });
        if url.query() == Some("") {
            url.set_query(None);
        }
        let body = endpoint.body()?;
        let accept = match E::FORMAT {
            Format::Envelope | Format::Json => APPLICATION_JSON,
            Format::Text | Format::Empty => "*/*",
        };
        let mut builder = Request::builder()
            .method(E::METHOD)
            .uri(String::from(url))
            .header(ACCEPT, HeaderValue::from_static(accept));
        if let Some(token) = &self.token {
            let value = sensitive_header(token.as_str())?;
            builder = builder
                .header(X_TOKEN, value.clone())
                .header(X_USERNAME, value);
        }
        let protected =
            path.as_str().starts_with("api/") || path.as_str().starts_with("room-history");
        if let Some(password) = self.server_password.as_deref()
            && protected
        {
            builder = builder.header(X_SERVER_PASSWORD, sensitive_header(password)?);
        }
        if body.is_some() {
            builder = builder.header(CONTENT_TYPE, HeaderValue::from_static(APPLICATION_JSON));
        }
        Ok(builder.body(body.unwrap_or_default())?)
    }

    /// Interprets the reply to a request built for `E`.
    ///
    /// Adopts a rotated `X-Token` first, even from error replies, then reads the rate-limit
    /// headers, maps the status and the `{error}` envelope to [`ApiError`] and reads
    /// `E::Response` as [`E::FORMAT`](Endpoint::FORMAT) says.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError`] for 401, 429 and other non-2xx statuses, `{error}` bodies, envelope
    /// bodies without a truthy `ok`, and bodies that do not decode into `E::Response`.
    pub fn response<E: Endpoint, B: AsRef<[u8]>>(
        &mut self,
        response: Response<B>,
    ) -> Result<Reply<E::Response>, ApiError> {
        let (parts, body) = response.into_parts();
        let token_rotated = self.adopt_token(&parts.headers);
        let rate_limit = RateLimit::from_headers(&parts.headers);
        let status = parts.status;
        let body = body.as_ref();
        match status {
            StatusCode::UNAUTHORIZED => return Err(ApiError::Unauthorized),
            StatusCode::TOO_MANY_REQUESTS => return Err(ApiError::RateLimited(rate_limit)),
            _ if !status.is_success() => {
                let message = match E::FORMAT {
                    // A text endpoint's error reply may be text too.
                    Format::Text => error_of(body).or_else(|| text_of(body)),
                    Format::Envelope | Format::Json | Format::Empty => error_of(body),
                };
                return Err(ApiError::Status { status, message });
            }
            _ => {}
        }
        let body = match E::FORMAT {
            Format::Envelope => {
                // A JSON `null` reply carries no envelope; it decodes only into optional bodies.
                if let Some(envelope) = serde_json::from_slice::<Option<Envelope>>(body)? {
                    if let Some(error) = envelope.error {
                        return Err(ApiError::Server(error));
                    }
                    if !envelope.ok.0 {
                        return Err(ApiError::MissingOk);
                    }
                }
                serde_json::from_slice(body)?
            }
            Format::Json => {
                if let Some(error) = error_of(body) {
                    return Err(ApiError::Server(error));
                }
                serde_json::from_slice(body)?
            }
            Format::Text => {
                let text = std::str::from_utf8(body).map_err(|_| {
                    <serde_json::Error as de::Error>::custom("reply body is not UTF-8 text")
                })?;
                serde_json::from_value(serde_json::Value::String(text.to_owned()))?
            }
            Format::Empty => {
                if let Some(error) = error_of(body) {
                    return Err(ApiError::Server(error));
                }
                serde_json::from_value(serde_json::Value::Null)?
            }
        };
        Ok(Reply {
            body,
            rate_limit,
            token_rotated,
        })
    }

    /// Stores a non-empty `X-Token` that differs from the held token; returns whether it did.
    fn adopt_token(&mut self, headers: &HeaderMap) -> bool {
        let Some(value) = headers
            .get(X_TOKEN)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
        else {
            return false;
        };
        if self
            .token
            .as_ref()
            .is_some_and(|held| held.as_str() == value)
        {
            return false;
        }
        self.token = Some(Token::new(value));
        true
    }
}

/// A successful reply.
#[derive(Debug, Clone)]
pub struct Reply<T> {
    /// The deserialized body.
    pub body: T,
    /// The endpoint's rate limit, sent with authenticated replies.
    pub rate_limit: Option<RateLimit>,
    /// The reply carried a new token, now held by the session; persist it if tokens are stored.
    pub token_rotated: bool,
}

/// The rate limit of one endpoint, from the `X-RateLimit-*` headers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateLimit {
    /// Requests allowed in the current window.
    pub limit: u32,
    /// Requests left in the current window.
    pub remaining: u32,
    /// When the window resets, in seconds since the Unix epoch.
    pub reset: u64,
}

impl RateLimit {
    /// All three headers, when present and numeric.
    fn from_headers(headers: &HeaderMap) -> Option<Self> {
        Some(Self {
            limit: header_number(headers, &X_RATELIMIT_LIMIT)?,
            remaining: header_number(headers, &X_RATELIMIT_REMAINING)?,
            reset: header_number(headers, &X_RATELIMIT_RESET)?,
        })
    }
}

fn header_number<T: std::str::FromStr>(headers: &HeaderMap, name: &HeaderName) -> Option<T> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
}

/// A header value hidden from `Debug` output and HTTP/2 header compression.
fn sensitive_header(value: &str) -> Result<HeaderValue, RequestError> {
    let mut value =
        HeaderValue::from_str(value).map_err(|error| RequestError::Http(error.into()))?;
    value.set_sensitive(true);
    Ok(value)
}

/// The non-empty `error` of a JSON object body, if the body is one.
fn error_of(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<Envelope>(body)
        .ok()
        .and_then(|envelope| envelope.error)
}

/// The body as trimmed text, if it is non-empty UTF-8.
fn text_of(body: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(body).ok()?.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// A request that cannot be built.
#[derive(Debug, thiserror::Error)]
pub enum RequestError {
    /// The endpoint requires a token and the session holds none.
    #[error("the endpoint requires a token")]
    MissingToken,
    /// The endpoint path does not resolve against the server URL.
    #[error(transparent)]
    Url(#[from] ServerUrlError),
    /// The request body does not serialize.
    #[error("request body does not serialize")]
    Body(#[from] serde_json::Error),
    /// The request is not valid HTTP, for example a token or server password that is not a
    /// valid header value.
    #[error("invalid HTTP request")]
    Http(#[from] ::http::Error),
}

/// A reply that does not carry the endpoint's success body.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// HTTP 401: the token is missing, invalid or expired.
    #[error("unauthorized")]
    Unauthorized,
    /// HTTP 429. `Some`: this endpoint's limit, wait until [`RateLimit::reset`]; `None`: the
    /// server-wide limit, retry after a short delay.
    #[error("rate limited")]
    RateLimited(Option<RateLimit>),
    /// Another non-2xx status, with the `{error}` message when the body is such JSON (or, for
    /// a [`Format::Text`] endpoint, the body's text).
    #[error("HTTP {status}{}", ErrorSuffix(.message.as_deref()))]
    Status {
        /// The status.
        status: StatusCode,
        /// The `error` of a JSON body, else the text of a [`Format::Text`] endpoint's body.
        message: Option<String>,
    },
    /// A 2xx `{error}` body: a logical failure reported by the server.
    #[error("server error: {0}")]
    Server(String),
    /// A 2xx JSON body without a truthy `ok` and without `error`.
    #[error("reply has no truthy `ok`")]
    MissingOk,
    /// The body is not JSON or not the endpoint's response type.
    #[error("reply body does not decode")]
    Decode(#[from] serde_json::Error),
}

/// Formats an optional message as `: message`.
struct ErrorSuffix<'a>(Option<&'a str>);

impl fmt::Display for ErrorSuffix<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.map_or(Ok(()), |message| write!(f, ": {message}"))
    }
}

/// The success/failure markers of a reply body; other fields are skipped without being built.
#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    ok: Truthy,
    #[serde(default, deserialize_with = "error_message")]
    error: Option<String>,
}

/// `error` as JavaScript clients test it: an empty string is no error.
fn error_message<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.filter(|message| !message.is_empty()))
}

/// A JSON value reduced to its JavaScript truthiness.
#[derive(Default)]
struct Truthy(bool);

impl<'de> Deserialize<'de> for Truthy {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(TruthyVisitor).map(Self)
    }
}

struct TruthyVisitor;

impl<'de> Visitor<'de> for TruthyVisitor {
    type Value = bool;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<bool, E> {
        Ok(value)
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<bool, E> {
        Ok(value != 0)
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<bool, E> {
        Ok(value != 0)
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<bool, E> {
        // False for 0, -0 and NaN, like JavaScript.
        Ok(value.abs() > 0.0)
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<bool, E> {
        Ok(!value.is_empty())
    }

    fn visit_unit<E: de::Error>(self) -> Result<bool, E> {
        Ok(false)
    }

    fn visit_none<E: de::Error>(self) -> Result<bool, E> {
        Ok(false)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<bool, A::Error> {
        IgnoredAny.visit_seq(seq).map(|_| true)
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<bool, A::Error> {
        IgnoredAny.visit_map(map).map(|_| true)
    }
}

#[cfg(test)]
mod tests {
    use ::http::{Method, Response, StatusCode};

    use super::{ApiError, RateLimit, RequestError, Session};
    use crate::api::official::endpoints::{
        AuthSignin, GameRoomTerrain, GameTime, PlatformOfficialRoomHistory, PlatformVersion,
        UserBadgeSvg, UserConsole, UserMemoryGet, UserWorldStatus,
    };
    use crate::api::official::types::{AuthSigninRequest, GameTimeResponse, UserConsoleRequest};
    use crate::{ServerUrl, Token};

    fn session(token: Option<&str>) -> Session {
        Session::new(ServerUrl::official(), token.map(Token::new))
    }

    fn reply(status: u16, headers: &[(&str, &str)], body: &'static str) -> Response<&'static str> {
        let mut builder = Response::builder().status(status);
        for &(name, value) in headers {
            builder = builder.header(name, value);
        }
        builder.body(body).unwrap()
    }

    fn game_time(
        session: &mut Session,
        response: Response<&'static str>,
    ) -> Result<GameTimeResponse, ApiError> {
        session
            .response::<GameTime, _>(response)
            .map(|reply| reply.body)
    }

    const RATE_HEADERS: [(&str, &str); 3] = [
        ("X-RateLimit-Limit", "60"),
        ("X-RateLimit-Remaining", "0"),
        ("X-RateLimit-Reset", "1760000000"),
    ];
    const RATE: RateLimit = RateLimit {
        limit: 60,
        remaining: 0,
        reset: 1_760_000_000,
    };

    #[test]
    fn error_envelope_with_http_200_is_a_server_error() {
        let result = game_time(
            &mut session(None),
            reply(200, &[], r#"{"error":"invalid shard"}"#),
        );
        assert!(matches!(result, Err(ApiError::Server(message)) if message == "invalid shard"));
    }

    #[test]
    fn success_requires_a_javascript_truthy_ok() {
        for body in [
            r#"{"time":5}"#,
            r#"{"ok":0,"time":5}"#,
            r#"{"ok":"","time":5}"#,
            r#"{"ok":null,"time":5}"#,
        ] {
            let result = game_time(&mut session(None), reply(200, &[], body));
            assert!(matches!(result, Err(ApiError::MissingOk)), "{body}");
        }
        for body in [
            r#"{"ok":1,"time":5}"#,
            r#"{"ok":true,"time":5}"#,
            r#"{"ok":"yes","time":5}"#,
            r#"{"ok":{},"time":5}"#,
            r#"{"ok":[0],"time":5}"#,
        ] {
            let result = game_time(&mut session(None), reply(201, &[], body));
            assert_eq!(result.unwrap(), GameTimeResponse { time: 5 }, "{body}");
        }
    }

    #[test]
    fn body_of_another_shape_is_a_decode_error() {
        let result = game_time(
            &mut session(None),
            reply(200, &[], r#"{"ok":1,"time":"soon"}"#),
        );
        assert!(matches!(result, Err(ApiError::Decode(_))));
        let result = game_time(&mut session(None), reply(200, &[], "<html>"));
        assert!(matches!(result, Err(ApiError::Decode(_))));
    }

    #[test]
    fn unauthorized_and_rate_limits_with_and_without_headers() {
        let mut session = session(Some("old"));
        let unauthorized = reply(401, &RATE_HEADERS, r#"{"error":"unauthorized"}"#);
        assert!(matches!(
            game_time(&mut session, unauthorized),
            Err(ApiError::Unauthorized)
        ));
        let endpoint = reply(429, &RATE_HEADERS, r#"{"error":"rate limit exceeded"}"#);
        assert!(matches!(
            game_time(&mut session, endpoint),
            Err(ApiError::RateLimited(Some(RATE)))
        ));
        let global = reply(429, &[], "Too Many Requests");
        assert!(matches!(
            game_time(&mut session, global),
            Err(ApiError::RateLimited(None))
        ));
    }

    #[test]
    fn other_statuses_carry_the_json_error_message_if_any() {
        let result = game_time(
            &mut session(None),
            reply(502, &[], "<html>Bad Gateway</html>"),
        );
        assert!(matches!(
            result,
            Err(ApiError::Status {
                status: StatusCode::BAD_GATEWAY,
                message: None
            })
        ));
        let result = game_time(
            &mut session(None),
            reply(400, &[], r#"{"error":"invalid params"}"#),
        );
        assert!(matches!(
            result,
            Err(ApiError::Status { status: StatusCode::BAD_REQUEST, message: Some(message) })
                if message == "invalid params"
        ));
    }

    #[test]
    fn rotated_token_is_adopted_on_success_and_on_errors() {
        let mut session = session(Some("old"));
        let mut headers = RATE_HEADERS.to_vec();
        headers.push(("X-Token", "new"));
        let ok = session
            .response::<UserWorldStatus, _>(reply(200, &headers, r#"{"ok":1,"status":"normal"}"#))
            .unwrap();
        assert!(ok.token_rotated);
        assert_eq!(ok.rate_limit, Some(RATE));
        assert_eq!(session.token().map(Token::as_str), Some("new"));

        let same = session
            .response::<UserWorldStatus, _>(reply(
                200,
                &[("X-Token", "new")],
                r#"{"ok":1,"status":"lost"}"#,
            ))
            .unwrap();
        assert!(!same.token_rotated);
        assert_eq!(same.rate_limit, None);

        let error = reply(200, &[("X-Token", "newer")], r#"{"error":"not found"}"#);
        assert!(session.response::<UserWorldStatus, _>(error).is_err());
        assert_eq!(session.token().map(Token::as_str), Some("newer"));
    }

    #[test]
    fn token_headers_are_sent_only_with_a_token() {
        let anonymous = session(None).request(&PlatformVersion).unwrap();
        assert_eq!(anonymous.method(), Method::GET);
        assert_eq!(anonymous.uri(), "https://screeps.com/api/version");
        assert_eq!(anonymous.headers()["accept"], "application/json");
        assert!(anonymous.headers().get("x-token").is_none());
        assert!(anonymous.headers().get("x-username").is_none());
        assert!(anonymous.headers().get("content-type").is_none());
        assert_eq!(anonymous.body().as_slice(), b"");

        let signed_in = session(Some("test-token"))
            .request(&PlatformVersion)
            .unwrap();
        assert_eq!(signed_in.headers()["x-token"], "test-token");
        assert_eq!(signed_in.headers()["x-username"], "test-token");
        assert!(signed_in.headers()["x-token"].is_sensitive());
    }

    #[test]
    fn server_password_is_sent_when_set_and_redacted() {
        let mut session = session(None);
        assert_eq!(session.server_password(), None);
        assert!(
            session
                .request(&PlatformVersion)
                .unwrap()
                .headers()
                .get("x-server-password")
                .is_none()
        );
        session.set_server_password(Some("open sesame".to_owned()));
        assert_eq!(session.server_password(), Some("open sesame"));
        let history = PlatformOfficialRoomHistory {
            shard: "shard0".to_owned(),
            room: "W1N1".to_owned(),
            tick: 100,
        };
        for request in [
            session.request(&PlatformVersion).unwrap(),
            session.request(&history).unwrap(),
        ] {
            let password = &request.headers()["x-server-password"];
            assert_eq!(password, "open sesame");
            assert!(password.is_sensitive());
        }
        assert!(!format!("{session:?}").contains("sesame"));
        session.set_server_password(None);
        assert!(
            session
                .request(&history)
                .unwrap()
                .headers()
                .get("x-server-password")
                .is_none()
        );
    }

    #[test]
    fn required_endpoints_need_a_token() {
        assert!(matches!(
            session(None).request(&UserWorldStatus),
            Err(RequestError::MissingToken)
        ));
        assert!(
            session(Some("test-token"))
                .request(&UserWorldStatus)
                .is_ok()
        );
    }

    #[test]
    fn query_parameters_are_encoded_and_absent_ones_omitted() {
        let session = session(Some("test-token"));
        let terrain = |shard: Option<&str>| {
            let endpoint = GameRoomTerrain {
                room: "W1N1".to_owned(),
                encoded: Some("1".to_owned()),
                shard: shard.map(str::to_owned),
            };
            session.request(&endpoint).unwrap().uri().to_string()
        };
        assert_eq!(
            terrain(Some("shard3")),
            "https://screeps.com/api/game/room-terrain?room=W1N1&encoded=1&shard=shard3"
        );
        assert_eq!(
            terrain(None),
            "https://screeps.com/api/game/room-terrain?room=W1N1&encoded=1"
        );
        let memory = UserMemoryGet {
            path: Some("creeps.Bob & co".to_owned()),
            shard: None,
        };
        assert_eq!(
            session.request(&memory).unwrap().uri(),
            "https://screeps.com/api/user/memory?path=creeps.Bob+%26+co"
        );
        assert_eq!(
            session.request(&GameTime::default()).unwrap().uri(),
            "https://screeps.com/api/game/time"
        );
    }

    #[test]
    fn paths_outside_api_and_path_parameters_are_encoded() {
        let history = PlatformOfficialRoomHistory {
            shard: "shard 0".to_owned(),
            room: "W1N1/../x".to_owned(),
            tick: 3500,
        };
        let request = session(None).request(&history).unwrap();
        assert_eq!(
            request.uri(),
            "https://screeps.com/room-history/shard%200/W1N1%2F..%2Fx/3500.json"
        );
        assert_eq!(request.headers()["accept"], "application/json");
        let season = Session::new(
            ServerUrl::parse("https://screeps.com/season").unwrap(),
            None,
        );
        assert_eq!(
            season.request(&history).unwrap().uri(),
            "https://screeps.com/season/room-history/shard%200/W1N1%2F..%2Fx/3500.json"
        );
    }

    #[test]
    fn bare_json_and_text_replies_have_no_envelope() {
        let history = session(None)
            .response::<PlatformOfficialRoomHistory, _>(reply(
                200,
                &[],
                r#"{"timestamp":1760000000000,"room":"W1N1","base":3500,"ticks":{"3500":{"a1":{"x":1}},"3501":{"a1":null}}}"#,
            ))
            .unwrap()
            .body;
        assert_eq!(history.base, 3500);
        assert_eq!(history.ticks["3501"]["a1"], None);
        let missing = session(None).response::<PlatformOfficialRoomHistory, _>(reply(
            200,
            &[],
            r#"{"error":"not found"}"#,
        ));
        assert!(matches!(missing, Err(ApiError::Server(message)) if message == "not found"));

        let badge = UserBadgeSvg {
            username: "Bob".to_owned(),
            border: None,
        };
        let request = session(None).request(&badge).unwrap();
        assert_eq!(
            request.uri(),
            "https://screeps.com/api/user/badge-svg?username=Bob"
        );
        let svg = session(None)
            .response::<UserBadgeSvg, _>(reply(200, &[], "<svg/>"))
            .unwrap();
        assert_eq!(svg.body, "<svg/>");
        // A text endpoint's error reply is text too: it is the message.
        let unknown = session(None).response::<UserBadgeSvg, _>(reply(404, &[], "Unknown user\n"));
        assert!(matches!(
            unknown,
            Err(ApiError::Status {
                status: StatusCode::NOT_FOUND,
                message: Some(message)
            }) if message == "Unknown user"
        ));
        let empty = session(None).response::<UserBadgeSvg, _>(reply(404, &[], " "));
        assert!(matches!(
            empty,
            Err(ApiError::Status {
                status: StatusCode::NOT_FOUND,
                message: None
            })
        ));
    }

    #[test]
    fn post_bodies_are_json() {
        let console = UserConsole {
            body: UserConsoleRequest {
                expression: "Game.time".to_owned(),
                shard: None,
            },
        };
        let request = session(Some("test-token")).request(&console).unwrap();
        assert_eq!(request.method(), Method::POST);
        assert_eq!(request.uri(), "https://screeps.com/api/user/console");
        assert_eq!(request.headers()["content-type"], "application/json");
        assert_eq!(request.body().as_slice(), br#"{"expression":"Game.time"}"#);

        let console = UserConsole {
            body: UserConsoleRequest {
                expression: "1".to_owned(),
                shard: Some("shard3".to_owned()),
            },
        };
        let request = session(Some("test-token")).request(&console).unwrap();
        assert_eq!(
            request.body().as_slice(),
            br#"{"expression":"1","shard":"shard3"}"#
        );

        let sign_in = AuthSignin {
            body: AuthSigninRequest {
                email: "a@example.com".to_owned(),
                password: "hunter2".to_owned(),
                recaptcha: None,
                steam_ticket: None,
            },
        };
        assert!(!format!("{sign_in:?}").contains("hunter2"));
        let request = session(None).request(&sign_in).unwrap();
        assert_eq!(
            request.body().as_slice(),
            br#"{"email":"a@example.com","password":"hunter2"}"#
        );
    }
}
