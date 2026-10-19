//! The shared HTTP client: token, sign-in, rate limits and the discovery of the server's kind
//! and world.

mod budget;
mod flows;
mod rate_limit;

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use http::HeaderValue;
use overseer_core::api::{self, ApiError, Auth, Endpoint, RateLimit, Reply, RequestError};
use overseer_core::model::account::Profile;
use overseer_core::model::platform::{ConnectionInfo, Plugins, ServerKind, Subsystem};
use overseer_core::model::world::Terrain;
use overseer_core::{ServerUrl, Token};
use reqwest_websocket::{Upgrade, WebSocket};

pub use budget::{GLOBAL_RATE_LIMIT_WAIT, Priority, REQUEST_BUDGET, REQUEST_WINDOW};
pub use rate_limit::{EndpointRateLimit, RateLimitPolicy};

use crate::api::{AccountApi as _, Api, PasswordSignIn as _};
use crate::rt::{Instant, SystemTime, UNIX_EPOCH, sleep, sleep_until};
use crate::socket::{ReconnectPolicy, Socket};
use crate::{Error, USER_AGENT, World};
use budget::Budget;
use rate_limit::{RateLimits, is_server_wide, until_reset};

/// HTTP API client and socket connector for one account on one server.
///
/// Clones share the token, the sign-in state, the rate limits, the terrain cache and the cached
/// [`World`].
#[derive(Debug, Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    http: reqwest::Client,
    /// The application supplied `http` ([`ClientBuilder::http_client`]): a retried socket
    /// upgrade goes through it, as its settings cannot be copied into a fresh client.
    #[cfg(not(target_arch = "wasm32"))]
    custom_http: bool,
    server: ServerUrl,
    /// Sent natively; browsers set their own.
    #[cfg(not(target_arch = "wasm32"))]
    user_agent: HeaderValue,
    /// Server, current token and server password. Locked only to copy or update it, never
    /// across an `.await`.
    session: Mutex<api::Session>,
    /// The session's token, published to [`Client::token_updates`] on every change.
    tokens: tokio::sync::watch::Sender<Option<Token>>,
    /// A reply replaced a held token through `X-Token`.
    rotated: AtomicBool,
    credentials: Option<Credentials>,
    /// Serializes sign-ins, so concurrent rejections sign in once.
    sign_in: tokio::sync::Mutex<()>,
    /// The server's kind, plugins and `GET /api/version`, once discovered.
    connection: Mutex<Option<ConnectionInfo>>,
    /// The kind [`ClientBuilder::server_kind`] forced, if any.
    forced_kind: Option<ServerKind>,
    /// The plugins [`ClientBuilder::plugins`] forced, if any.
    forced_plugins: Option<Plugins>,
    world: Mutex<Option<World>>,
    default_shard: Option<String>,
    reconnect: ReconnectPolicy,
    keep_alive: Option<Duration>,
    rate_limit: Option<RateLimitPolicy>,
    rate_limits: Mutex<RateLimits>,
    /// Replies with HTTP 429 so far.
    rate_limited: AtomicU64,
    /// Every request of the latest window, the rate-limit exemption and the latest
    /// server-wide refusal.
    budget: Mutex<Budget>,
    terrain: Mutex<TerrainCache>,
}

/// Decoded terrain by `(shard, room)`; terrain never changes.
type TerrainCache = HashMap<(Option<String>, String), Arc<Terrain>>;

/// E-mail and password for servers with password sign-in.
#[derive(Clone)]
struct Credentials {
    email: String,
    password: String,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Credentials(<redacted>)")
    }
}

impl Client {
    /// Starts configuring a client for `server`.
    pub fn builder(server: ServerUrl) -> ClientBuilder {
        ClientBuilder {
            server,
            token: None,
            credentials: None,
            server_password: None,
            http: None,
            reconnect: ReconnectPolicy::default(),
            keep_alive: Some(Duration::from_secs(30)),
            rate_limit: Some(RateLimitPolicy::default()),
            user_agent: None,
            default_shard: None,
            server_kind: None,
            plugins: None,
        }
    }

    /// The server.
    #[must_use]
    pub fn server(&self) -> &ServerUrl {
        &self.inner.server
    }

    /// The current token, including tokens rotated by the server or issued by a sign-in.
    #[must_use]
    pub fn token(&self) -> Option<Token> {
        self.session().token().cloned()
    }

    /// Replaces the token used by every clone of this client and by future socket connections.
    pub fn set_token(&self, token: Option<Token>) {
        let mut session = self.session();
        session.set_token(token.clone());
        // Published under the session lock, so watchers see changes in the order they were made.
        self.inner.tokens.send_if_modified(|current| {
            let changed = *current != token;
            *current = token;
            changed
        });
    }

    /// Every change of the shared token: [`Self::set_token`], `X-Token` rotations, sign-ins and
    /// the socket's `auth ok` tokens. The receiver starts at the current token; persist each new
    /// value if you store tokens.
    #[must_use]
    pub fn token_updates(&self) -> tokio::sync::watch::Receiver<Option<Token>> {
        self.inner.tokens.subscribe()
    }

    /// Whether the server has replaced a held token through an `X-Token` reply since the client
    /// was built: it issues session tokens that expire unless used, such as the one-minute
    /// tokens of private servers, which every authenticated reply replaces. Sign-ins,
    /// [`Self::set_token`] and the socket's `auth ok` do not count; API tokens (screeps.com,
    /// `OpenScreeps`, screepsmod-auth) never rotate.
    #[must_use]
    pub fn rotates_tokens(&self) -> bool {
        self.inner.rotated.load(Ordering::Relaxed)
    }

    /// Executes an endpoint; a rotated `X-Token` replaces the shared token.
    ///
    /// With credentials, signs in first when the endpoint takes a token and none is held, and
    /// signs in again and retries once when the server answers 401. Requests rejected with
    /// HTTP 429 are retried once per the [`RateLimitPolicy`]; the reported limits are kept for
    /// [`Self::rate_limit`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Request`] when the request cannot be built (for example a missing token),
    /// [`Error::Http`] for transport failures and [`Error::Api`] for error replies, including
    /// those of a needed sign-in and [`ApiError::RateLimited`] when a retry is not allowed or
    /// fails again. A retry after HTTP 401 that fails before it sends anything more returns
    /// [`Error::RetryNotSent`]. [`Error::outcome`] covers every attempt.
    pub async fn send<E: Endpoint + Sync>(
        &self,
        endpoint: &E,
    ) -> Result<Reply<E::Response>, Error> {
        if let Some(credentials) = &self.inner.credentials
            && E::AUTH != Auth::None
            && self.token().is_none()
        {
            self.renew_token(credentials, None).await?;
        }
        let (used, result) = self.execute(endpoint).await;
        match (result, &self.inner.credentials) {
            (Err(Error::Api(ApiError::Unauthorized)), Some(credentials)) => {
                let retry = async {
                    self.renew_token(credentials, used.as_ref()).await?;
                    self.execute(endpoint).await.1
                };
                retry.await.map_err(Error::after_unauthorized)
            }
            (result, _) => result,
        }
    }

    /// The latest rate limit the server reported for endpoint `E` (sent with authenticated
    /// replies and with HTTP 429), for showing remaining quota.
    #[must_use]
    pub fn rate_limit<E: Endpoint>(&self) -> Option<RateLimit> {
        lock(&self.inner.rate_limits).get::<E>()
    }

    /// The latest rate limit of every endpoint that reported one, ordered by path and method.
    #[must_use]
    pub fn rate_limits(&self) -> Vec<EndpointRateLimit> {
        lock(&self.inner.rate_limits).all()
    }

    /// Forgets the latest rate limit reported for endpoint `E`, so [`Self::rate_limit`] is
    /// `None` until a response reports one again. For a limit known to be stale, such as
    /// after the user lifted it outside this client (screeps.com's reset page): the next
    /// response's headers record the real one.
    pub fn forget_rate_limit<E: Endpoint>(&self) {
        lock(&self.inner.rate_limits).forget::<E>();
    }

    /// How many replies were HTTP 429 ([`ApiError::RateLimited`]) since the client was built,
    /// retried ones included: a change means a limit was hit, endpoint or server-wide.
    #[must_use]
    pub fn rate_limited_count(&self) -> u64 {
        self.inner.rate_limited.load(Ordering::Relaxed)
    }

    /// Requests this client sent (every attempt, retries included) in the latest
    /// [`REQUEST_WINDOW`].
    #[must_use]
    pub fn requests_in_window(&self) -> usize {
        lock(&self.inner.budget).count(Instant::now())
    }

    /// Whether the server limits request rates. Three things tell it:
    ///
    /// - The server is official: its address is screeps.com ([`ServerUrl::is_official`],
    ///   the persistent world and the season), or [`ClientBuilder::server_kind`] forced the
    ///   official contract. Only screeps.com speaks that contract, so a forced official
    ///   server on another address is screeps.com behind something. Its server-wide limit of
    ///   120 requests per minute per token applies before any reply reports it.
    /// - A response carried `X-RateLimit-*` headers.
    /// - A response was refused with HTTP 429.
    #[must_use]
    pub fn has_rate_limits(&self) -> bool {
        self.inner.server.is_official()
            || self.inner.forced_kind == Some(ServerKind::Official)
            || self.rate_limited_count() > 0
            || !self.rate_limits().is_empty()
    }

    /// Tells the client that the token is exempt from rate limits until `until` (milliseconds
    /// since the Unix epoch; screeps.com's `noRatelimitUntil`), or not (`None`): meanwhile
    /// [`Self::budget_ready_at`] lets every request go.
    pub fn set_rate_limit_exempt_until(&self, until: Option<u64>) {
        lock(&self.inner.budget).set_exempt_until(until);
    }

    /// Whether the token is exempt from rate limits now
    /// ([`Self::set_rate_limit_exempt_until`]).
    #[must_use]
    pub fn is_rate_limit_exempt(&self) -> bool {
        lock(&self.inner.budget).is_exempt(epoch_millis())
    }

    /// When a background request of `priority` may go out, after `pending` requests the
    /// caller allowed but has not sent yet: `None` at once. Background loading (the world
    /// map, badges) asks before each request; the user's own requests never ask and never
    /// wait, but count.
    ///
    /// On servers with rate limits ([`Self::has_rate_limits`]) every request of the client
    /// counts over a sliding [`REQUEST_WINDOW`], and a request of `priority` waits while
    /// [`Priority::budget`] are in it ([`REQUEST_BUDGET`], 110 of screeps.com's 120 per
    /// minute, for viewed rooms, 5 and 10 fewer for visible and background ones). A sliding
    /// window also bounds every fixed minute. After HTTP 429 of the server-wide limit every
    /// priority waits until the limit resets, at most [`GLOBAL_RATE_LIMIT_WAIT`]. Servers
    /// without rate limits, and a token exempt from them, have no budget: `None`.
    #[must_use]
    pub fn budget_ready_at(&self, priority: Priority, pending: usize) -> Option<Instant> {
        if !self.has_rate_limits() || self.is_rate_limit_exempt() {
            return None;
        }
        lock(&self.inner.budget).ready_at(priority, pending, Instant::now())
    }

    /// Waits until a background request of `priority` may go out after `pending` requests the
    /// caller allowed but has not sent yet ([`Self::budget_ready_at`]): returns at once when
    /// it may go now, and on servers without a budget. Drop the future to cancel the wait.
    ///
    /// The wait reserves nothing: other tasks can spend the budget between the end of the wait
    /// and the request. Callers that send several requests count them in `pending`.
    pub async fn wait_for_budget(&self, priority: Priority, pending: usize) {
        while let Some(at) = self.budget_ready_at(priority, pending) {
            sleep_until(at).await;
        }
    }

    /// The server's connection info (kind, plugins, `GET /api/version`), subsystems and shards,
    /// and the signed-in user, fetched once and cached.
    ///
    /// Discovers the server ([`Self::api`]), and requests `GET /api/auth/me` when the client
    /// holds a token or credentials. A cached anonymous world is fetched again once the client
    /// can authenticate.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::send`].
    pub async fn world(&self) -> Result<World, Error> {
        Ok(self.discover().await?.0)
    }

    /// [`Self::world`], and the signed-in user's profile (`GET /api/auth/me`) when this call
    /// requested it (`None` when the world was cached or the client is anonymous), so that a
    /// caller showing the profile need not request it again.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::send`].
    pub async fn discover(&self) -> Result<(World, Option<Profile>), Error> {
        let authenticated = self.inner.credentials.is_some() || self.token().is_some();
        if let Some(world) = lock(&self.inner.world)
            .as_ref()
            .filter(|world| world.user().is_some() || !authenticated)
        {
            return Ok((world.clone(), None));
        }
        let connection = self.connection(Clone::clone).await?;
        let me = if authenticated {
            Some(
                Api::new(self.clone(), connection.kind, connection.plugins)
                    .me()
                    .await?,
            )
        } else {
            None
        };
        let mut world = World::new(connection, me.as_ref().map(|me| me.id.clone()));
        if let Some(shard) = &self.inner.default_shard {
            world.set_shard(shard.clone());
        }
        *lock(&self.inner.world) = Some(world.clone());
        Ok((world, me))
    }

    /// The server's operations in Overseer's types, for the contract it speaks
    /// ([`crate::api`]). The first call discovers the server without a token: official servers
    /// by their address ([`ServerUrl::is_official`], also behind a proxy), every other server
    /// by one `GET /api/version` (`OpenScreeps` lists the `openscreeps` feature, any other is a
    /// community server, whose plugins are detected as well: screepsmod-admin-utils by its
    /// feature, screepsmod-auth by `GET /api/authmod`). A kind forced with
    /// [`ClientBuilder::server_kind`] (and, on a community server, plugins forced with
    /// [`ClientBuilder::plugins`]) needs no request at all; [`Self::world`] still reads
    /// `GET /api/version`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Http`] for transport failures and [`Error::Api`] for error replies and
    /// a `GET /api/version` the contract does not describe.
    pub async fn api(&self) -> Result<Api, Error> {
        // A forced kind (with forced plugins on a community server) needs no request.
        let forced = match (self.inner.forced_kind, self.inner.forced_plugins) {
            (Some(ServerKind::Community), None) | (None, _) => None,
            (Some(ServerKind::Community), Some(plugins)) => Some((ServerKind::Community, plugins)),
            (Some(kind), _) => Some((kind, Plugins::NONE)),
        };
        if let Some((kind, plugins)) = forced {
            return Ok(Api::new(self.clone(), kind, plugins));
        }
        let (kind, plugins) = self
            .connection(|connection| (connection.kind, connection.plugins))
            .await?;
        Ok(Api::new(self.clone(), kind, plugins))
    }

    /// `read` of the discovered server (`api::discover`), discovered on first use.
    async fn connection<T>(&self, read: impl FnOnce(&ConnectionInfo) -> T) -> Result<T, Error> {
        if let Some(connection) = lock(&self.inner.connection).as_ref() {
            return Ok(read(connection));
        }
        let connection =
            crate::api::discover(self, self.inner.forced_kind, self.inner.forced_plugins).await?;
        let value = read(&connection);
        *lock(&self.inner.connection) = Some(connection);
        Ok(value)
    }

    /// Opens a socket connection for the [`World`] ([`Self::world`]), authenticated with the
    /// current token (anonymous without one).
    ///
    /// The upgrade shares the client's connection pool, so it may reuse a keep-alive connection
    /// the server has just closed for idling (Node closes them after 5 s). Natively, an upgrade
    /// whose connection is reset or closed before any response arrives is therefore sent once
    /// more on a new connection (with a custom [`ClientBuilder::http_client`]: through that
    /// client, whose pool has dropped the failed connection). An answer of the server, such as
    /// an HTTP error, is never retried. In browsers the `WebSocket` opens its own connection.
    /// Reconnects of the [`Socket`] open their connections the same way.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::world`] and [`Error::WebSocket`] when the connection or
    /// upgrade fails.
    pub async fn connect(&self) -> Result<Socket, Error> {
        let world = self.world().await?;
        let websocket = self.open().await?;
        Ok(Socket::new(self.clone(), world, websocket))
    }

    /// Opens a WebSocket to the server's socket endpoint, retrying a lost connection natively
    /// (see [`Self::connect`]).
    pub(crate) async fn open(&self) -> Result<WebSocket, Error> {
        let response = self.upgrade(&self.inner.http).await;
        #[cfg(not(target_arch = "wasm32"))]
        let response = match response {
            Err(reqwest_websocket::Error::Reqwest(error)) if connection_lost(&error) => {
                let fresh;
                let client = if self.inner.custom_http {
                    &self.inner.http
                } else {
                    // A new client has an empty pool: the upgrade opens a new connection.
                    fresh = default_http()?;
                    &fresh
                };
                self.upgrade(client).await
            }
            response => response,
        };
        Ok(response?.into_websocket().await?)
    }

    /// Sends the socket upgrade request through `client`.
    async fn upgrade(
        &self,
        client: &reqwest::Client,
    ) -> Result<reqwest_websocket::UpgradeResponse, reqwest_websocket::Error> {
        let request = client.get(self.inner.server.socket().clone());
        #[cfg(not(target_arch = "wasm32"))]
        let request = request.header(http::header::USER_AGENT, self.inner.user_agent.clone());
        request.upgrade().send().await
    }

    pub(crate) fn reconnect_policy(&self) -> &ReconnectPolicy {
        &self.inner.reconnect
    }

    pub(crate) fn keep_alive(&self) -> Option<Duration> {
        self.inner.keep_alive
    }

    pub(crate) fn can_sign_in(&self) -> bool {
        self.inner.credentials.is_some()
    }

    /// Signs in with the credentials unless the token already differs from `rejected`; returns
    /// the token to use. Without credentials, fails with [`ApiError::Unauthorized`].
    pub(crate) async fn renew(&self, rejected: Option<&Token>) -> Result<Token, Error> {
        match &self.inner.credentials {
            Some(credentials) => self.renew_token(credentials, rejected).await,
            None => Err(ApiError::Unauthorized.into()),
        }
    }

    /// Signs in with `credentials` through the server's
    /// [`PasswordSignIn`](crate::api::PasswordSignIn) unless the token already differs from
    /// `rejected`. Discovers the server first when needed; a server without password sign-in
    /// fails with [`Error::Unsupported`].
    async fn renew_token(
        &self,
        credentials: &Credentials,
        rejected: Option<&Token>,
    ) -> Result<Token, Error> {
        let _signing_in = self.inner.sign_in.lock().await;
        if let Some(current) = self.token()
            && Some(&current) != rejected
        {
            return Ok(current);
        }
        let api = self.api().await?;
        let sign_in = api
            .password_sign_in()
            .ok_or(Error::Unsupported(Subsystem::PasswordSignIn))?;
        let token = sign_in
            .sign_in(&credentials.email, &credentials.password)
            .await?;
        self.set_token(Some(token.clone()));
        Ok(token)
    }

    /// Sends `endpoint` like [`Self::send`] but never signs in: for the requests discovery and
    /// the sign-in itself make, which take no token. Retries HTTP 429 per the
    /// [`RateLimitPolicy`].
    pub(crate) async fn send_without_sign_in<E: Endpoint + Sync>(
        &self,
        endpoint: &E,
    ) -> Result<Reply<E::Response>, Error> {
        self.execute(endpoint).await.1
    }

    /// The shard a request names: `shard`, else the [`World`]'s default shard; `None` on
    /// single-world servers.
    pub(crate) async fn resolve_shard(&self, shard: Option<&str>) -> Result<Option<String>, Error> {
        Ok(self.world().await?.resolve_shard(shard))
    }

    /// Sends a request, retrying once after HTTP 429 when the policy allows; returns the token
    /// the last attempt used. A refused attempt did not run ([`Error::outcome`]), so the retry
    /// cannot apply the request twice.
    async fn execute<E: Endpoint + Sync>(
        &self,
        endpoint: &E,
    ) -> (Option<Token>, Result<Reply<E::Response>, Error>) {
        let (used, result) = self.attempt(endpoint).await;
        if let (Err(Error::Api(ApiError::RateLimited(limit))), Some(policy)) =
            (&result, &self.inner.rate_limit)
            && let Some(wait) = policy.retry_delay(limit.as_ref(), SystemTime::now())
        {
            sleep(wait).await;
            return self.attempt(endpoint).await;
        }
        (used, result)
    }

    /// Sends one request with a copy of the session and records its rate limit; returns the
    /// token it used.
    async fn attempt<E: Endpoint + Sync>(
        &self,
        endpoint: &E,
    ) -> (Option<Token>, Result<Reply<E::Response>, Error>) {
        lock(&self.inner.budget).record(Instant::now());
        let mut session = self.session().clone();
        let used = session.token().cloned();
        let result = self.exchange(&mut session, endpoint).await;
        if session.token() != used.as_ref() {
            // Only `X-Token` changes the copy; stored before the change is published.
            if used.is_some() {
                self.inner.rotated.store(true, Ordering::Relaxed);
            }
            self.set_token(session.token().cloned());
        }
        let limit = match &result {
            Ok(reply) => reply.rate_limit,
            Err(Error::Api(ApiError::RateLimited(limit))) => {
                self.inner.rate_limited.fetch_add(1, Ordering::Relaxed);
                if is_server_wide(limit.as_ref()) {
                    let wait = limit.as_ref().map_or(GLOBAL_RATE_LIMIT_WAIT, |limit| {
                        until_reset(limit, SystemTime::now()).min(GLOBAL_RATE_LIMIT_WAIT)
                    });
                    lock(&self.inner.budget).refused(Instant::now() + wait);
                }
                *limit
            }
            Err(_) => None,
        };
        if let Some(limit) = limit {
            lock(&self.inner.rate_limits).record::<E>(limit);
        }
        (used, result)
    }

    async fn exchange<E: Endpoint + Sync>(
        &self,
        session: &mut api::Session,
        endpoint: &E,
    ) -> Result<Reply<E::Response>, Error> {
        #[cfg_attr(
            target_arch = "wasm32",
            expect(unused_mut, reason = "browsers set `User-Agent` themselves")
        )]
        let mut request = reqwest::Request::try_from(session.request(endpoint)?)?;
        #[cfg(not(target_arch = "wasm32"))]
        request
            .headers_mut()
            .insert(http::header::USER_AGENT, self.inner.user_agent.clone());
        let mut response = self.inner.http.execute(request).await?;
        let status = response.status();
        let headers = std::mem::take(response.headers_mut());
        let body = response.bytes().await?;
        let mut reply = http::Response::new(body);
        *reply.status_mut() = status;
        *reply.headers_mut() = headers;
        Ok(session.response::<E, _>(reply)?)
    }

    fn session(&self) -> MutexGuard<'_, api::Session> {
        lock(&self.inner.session)
    }
}

/// Locks a mutex whose critical sections cannot panic halfway through an update.
/// Now, in milliseconds since the Unix epoch.
fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Configures a [`Client`]; created by [`Client::builder`].
#[must_use]
pub struct ClientBuilder {
    server: ServerUrl,
    token: Option<Token>,
    credentials: Option<Credentials>,
    server_password: Option<String>,
    http: Option<reqwest::Client>,
    reconnect: ReconnectPolicy,
    keep_alive: Option<Duration>,
    rate_limit: Option<RateLimitPolicy>,
    user_agent: Option<String>,
    default_shard: Option<String>,
    server_kind: Option<ServerKind>,
    plugins: Option<Plugins>,
}

impl fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientBuilder")
            .field("server", &self.server)
            .field("reconnect", &self.reconnect)
            .field("keep_alive", &self.keep_alive)
            .field("rate_limit", &self.rate_limit)
            .field("server_kind", &self.server_kind)
            .field("plugins", &self.plugins)
            .finish_non_exhaustive()
    }
}

impl ClientBuilder {
    /// Authenticates with an account API token or a session token.
    pub fn token(mut self, token: Token) -> Self {
        self.token = Some(token);
        self
    }

    /// Signs in with e-mail and password (`POST /api/auth/signin`, private servers) when a token
    /// is needed and none is held, and again when the server rejects the token.
    pub fn credentials(mut self, email: impl Into<String>, password: impl Into<String>) -> Self {
        self.credentials = Some(Credentials {
            email: email.into(),
            password: password.into(),
        });
        self
    }

    /// Sends `X-Server-Password` to a password-protected private server.
    pub fn server_password(mut self, password: impl Into<String>) -> Self {
        self.server_password = Some(password.into());
        self
    }

    /// Uses a caller-configured `reqwest` client. It must allow HTTP/1.1 for socket upgrades and
    /// should not follow redirects (tokens must never follow a redirect). The default client is
    /// HTTP/1.1-only, follows no redirects and has a 10 s connect timeout; in browsers it is a
    /// plain `fetch` client, and redirects, timeouts and the HTTP version are the browser's.
    ///
    /// A socket upgrade that lost a pooled connection is retried through this client (see
    /// [`Client::connect`]), which takes another pooled connection if it holds one; the default
    /// client retries on a new one-shot client with an empty pool instead.
    pub fn http_client(mut self, http: reqwest::Client) -> Self {
        self.http = Some(http);
        self
    }

    /// How a dropped socket connection is re-established; [`ReconnectPolicy::default`] when not
    /// set.
    pub const fn reconnect(mut self, policy: ReconnectPolicy) -> Self {
        self.reconnect = policy;
        self
    }

    /// Interval of socket keep-alive pings (default 30 s); `None` disables them. A connection
    /// that receives no frame for two intervals is considered dead and reconnected.
    ///
    /// Ignored in browsers: their `WebSocket` answers the server's pings but cannot send any,
    /// and reports a dead connection as closed.
    pub const fn keep_alive(mut self, interval: Option<Duration>) -> Self {
        self.keep_alive = interval;
        self
    }

    /// How requests rejected with HTTP 429 are retried ([`RateLimitPolicy::default`] when not
    /// set); `None` returns [`ApiError::RateLimited`] without retrying.
    pub const fn rate_limit(mut self, policy: Option<RateLimitPolicy>) -> Self {
        self.rate_limit = policy;
        self
    }

    /// The `User-Agent` of every request and socket upgrade (default [`USER_AGENT`]); it
    /// replaces one configured on a custom `reqwest` client. Validated, but not sent, in
    /// browsers: they forbid scripts to set `User-Agent`.
    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = Some(user_agent.into());
        self
    }

    /// The shard socket helpers use when none is named, instead of the first shard listed.
    /// Ignored by single-world servers.
    pub fn default_shard(mut self, shard: impl Into<String>) -> Self {
        self.default_shard = Some(shard.into());
        self
    }

    /// Forces the contract the server speaks instead of discovering it: discovery skips
    /// telling the kind (by the address, or by the `openscreeps` feature) and reads
    /// `GET /api/version` with `kind`'s contract for [`World::version`]. The subsystems follow
    /// from the forced kind (and, on a community server, its plugins: detected unless
    /// [`Self::plugins`] forces them too). For servers discovery misjudges, or to talk to one
    /// with another contract it is compatible with; a wrong kind makes requests fail.
    pub const fn server_kind(mut self, kind: ServerKind) -> Self {
        self.server_kind = Some(kind);
        self
    }

    /// Forces the plugins a community server runs instead of detecting them (no
    /// `GET /api/authmod`, and screepsmod-admin-utils' feature is not looked for). Ignored on
    /// the other kinds, which run none.
    pub const fn plugins(mut self, plugins: Plugins) -> Self {
        self.plugins = Some(plugins);
        self
    }

    /// Builds the client.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Request`] for a user agent that is not a valid header value and
    /// [`Error::Http`] when the default `reqwest` client's TLS backend cannot be initialised.
    ///
    /// TLS uses rustls with the `ring` provider, installed as the process default unless the
    /// application installed another provider first. In browsers the browser does TLS.
    pub fn build(self) -> Result<Client, Error> {
        // Err only means a provider is already installed, which then stays in use.
        #[cfg(not(target_arch = "wasm32"))]
        let _ = rustls::crypto::ring::default_provider().install_default();
        let user_agent = HeaderValue::try_from(self.user_agent.as_deref().unwrap_or(USER_AGENT))
            .map_err(|error| RequestError::Http(error.into()))?;
        // Browsers send their own `User-Agent`.
        #[cfg(target_arch = "wasm32")]
        let _ = user_agent;
        #[cfg(not(target_arch = "wasm32"))]
        let custom_http = self.http.is_some();
        let http = match self.http {
            Some(http) => http,
            None => default_http()?,
        };
        let tokens = tokio::sync::watch::Sender::new(self.token.clone());
        let mut session = api::Session::new(self.server.clone(), self.token);
        session.set_server_password(self.server_password);
        Ok(Client {
            inner: Arc::new(Inner {
                http,
                #[cfg(not(target_arch = "wasm32"))]
                custom_http,
                server: self.server,
                #[cfg(not(target_arch = "wasm32"))]
                user_agent,
                session: Mutex::new(session),
                tokens,
                rotated: AtomicBool::new(false),
                credentials: self.credentials,
                sign_in: tokio::sync::Mutex::new(()),
                connection: Mutex::new(None),
                forced_kind: self.server_kind,
                forced_plugins: self.plugins,
                world: Mutex::new(None),
                default_shard: self.default_shard,
                reconnect: self.reconnect,
                keep_alive: self.keep_alive,
                rate_limit: self.rate_limit,
                rate_limits: Mutex::new(RateLimits::default()),
                rate_limited: AtomicU64::new(0),
                budget: Mutex::new(Budget::default()),
                terrain: Mutex::new(HashMap::new()),
            }),
        })
    }
}

/// The default HTTP client: HTTP/1.1 only (socket upgrades), no redirects (tokens must never
/// follow one) and a 10 s connect timeout.
#[cfg(not(target_arch = "wasm32"))]
fn default_http() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .http1_only()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .build()
}

/// The default HTTP client in browsers: `fetch`, whose redirects, timeouts and HTTP version
/// are the browser's.
#[cfg(target_arch = "wasm32")]
fn default_http() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder().build()
}

/// Whether a request failed because its connection was reset or closed before any response
/// arrived (not while connecting, not on a timeout, not on a malformed response).
#[cfg(not(target_arch = "wasm32"))]
fn connection_lost(error: &reqwest::Error) -> bool {
    use std::error::Error as _;
    use std::io::ErrorKind;

    if !error.is_request() || error.is_connect() || error.is_timeout() {
        return false;
    }
    let mut source = error.source();
    while let Some(error) = source {
        if let Some(error) = error.downcast_ref::<hyper::Error>()
            && (error.is_incomplete_message() || error.is_canceled() || error.is_closed())
        {
            return true;
        }
        if let Some(error) = error.downcast_ref::<std::io::Error>() {
            return matches!(
                error.kind(),
                ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted
                    | ErrorKind::BrokenPipe
                    | ErrorKind::UnexpectedEof
            );
        }
        source = error.source();
    }
    false
}
