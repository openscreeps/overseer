//! Platform area: `GET /api/version`, and the discovery of the server's kind and plugins.

use overseer_core::api::plugins::{screepsmod_admin_utils, screepsmod_auth};
use overseer_core::api::{
    ApiError, Auth, Endpoint, Format, Query, community, official, openscreeps,
};
use overseer_core::model::platform::{
    ADMIN_UTILS_FEATURE, ConnectionInfo, Discovery, Plugins, STOCK_ROOM_HISTORY, ServerInfo,
    ServerKind,
};

use super::{Api, Community, Official, OpenScreeps, delegate};
use crate::{Client, Error};

/// Operations on the server itself.
pub trait PlatformApi {
    /// What the server reports about itself (`GET /api/version`), read again. A community
    /// server with screepsmod-admin-utils is read with the plugin's reply.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`].
    fn version(&self) -> impl Future<Output = Result<ServerInfo, Error>>;
}

impl PlatformApi for Api {
    async fn version(&self) -> Result<ServerInfo, Error> {
        delegate!(self, api => api.version().await)
    }
}

// `/api/version` takes no token: none of these signs in, so discovery can use them while
// signing in.

impl PlatformApi for Official {
    async fn version(&self) -> Result<ServerInfo, Error> {
        let reply = self
            .client
            .send_without_sign_in(&official::endpoints::PlatformVersion)
            .await?;
        Ok(reply.body.into())
    }
}

impl PlatformApi for OpenScreeps {
    async fn version(&self) -> Result<ServerInfo, Error> {
        let reply = self
            .client
            .send_without_sign_in(&openscreeps::endpoints::PlatformVersion)
            .await?;
        Ok(reply.body.into())
    }
}

impl PlatformApi for Community {
    async fn version(&self) -> Result<ServerInfo, Error> {
        if self.plugins.screepsmod_admin_utils {
            let reply = self
                .client
                .send_without_sign_in(&screepsmod_admin_utils::endpoints::PlatformVersion)
                .await?;
            Ok(reply.body.into())
        } else {
            let reply = self
                .client
                .send_without_sign_in(&community::endpoints::PlatformVersion)
                .await?;
            Ok(ServerInfo::from_community(reply.body))
        }
    }
}

/// `GET /api/version` as JSON, read before the contract is known.
struct VersionReply;

impl Endpoint for VersionReply {
    type Response = serde_json::Value;
    const METHOD: http::Method = <official::endpoints::PlatformVersion as Endpoint>::METHOD;
    const PATH: &'static str = <official::endpoints::PlatformVersion as Endpoint>::PATH;
    const AUTH: Auth = Auth::None;
    const FORMAT: Format = Format::Envelope;
}

/// A `GET /room-history` read as text, to tell the stock server's reply from a history mod's.
struct RoomHistoryProbe;

impl Endpoint for RoomHistoryProbe {
    type Response = String;
    const METHOD: http::Method = <community::endpoints::PlatformRoomHistory as Endpoint>::METHOD;
    const PATH: &'static str = <community::endpoints::PlatformRoomHistory as Endpoint>::PATH;
    const AUTH: Auth = Auth::None;
    const FORMAT: Format = Format::Text;

    fn query(&self, query: &mut Query<'_>) {
        // Any chunk: the stock provider ignores both, a mod answers whether it holds it or not.
        query.pair("room", "W0N0");
        query.pair("time", "0");
    }
}

/// Finds out which contract `client`'s server speaks and which plugins it runs, without a
/// token (and so without signing in), with `GET /api/version` (and, on a community server, a
/// request per plugin that lists no feature):
///
/// 1. The kind is `forced_kind` when the caller forced one. Otherwise a server known as
///    official ([`ServerUrl::is_official`](overseer_core::ServerUrl::is_official), also behind
///    a proxy) speaks the official contract, and any other is read with [`Discovery`]: a
///    server listing the `openscreeps` feature speaks the `OpenScreeps` contract, any other the
///    community one.
/// 2. The plugins of a community server are `forced_plugins` when the caller forced them
///    (ignored on the other kinds, which run none). Otherwise screepsmod-admin-utils runs when
///    the reply lists its feature, screepsmod-auth when `GET /api/authmod` answers (as
///    node-screeps-api detects it), and a room-history mod when `GET /room-history` answers
///    other than the stock HTTP 500 [`STOCK_ROOM_HISTORY`].
/// 3. The reply is read with the contract's type: on a community server running
///    screepsmod-admin-utils, the plugin's.
pub(crate) async fn discover(
    client: &Client,
    forced_kind: Option<ServerKind>,
    forced_plugins: Option<Plugins>,
) -> Result<ConnectionInfo, Error> {
    let reply = client.send_without_sign_in(&VersionReply).await?.body;
    let kind = match forced_kind {
        Some(kind) => kind,
        None if client.server().is_official() => ServerKind::Official,
        None => ServerKind::of(&Discovery::from_reply(&reply).map_err(ApiError::Decode)?),
    };
    let decode = ApiError::Decode;
    let (plugins, server) = match kind {
        ServerKind::Official => (
            Plugins::NONE,
            serde_json::from_value::<official::types::PlatformVersion>(reply)
                .map_err(decode)?
                .into(),
        ),
        ServerKind::OpenScreeps => (
            Plugins::NONE,
            serde_json::from_value::<openscreeps::types::PlatformVersion>(reply)
                .map_err(decode)?
                .into(),
        ),
        ServerKind::Community => {
            let plugins = match forced_plugins {
                Some(plugins) => plugins,
                None => Plugins {
                    screepsmod_auth: runs_screepsmod_auth(client).await?,
                    screepsmod_admin_utils: Discovery::from_reply(&reply)
                        .map_err(decode)?
                        .has_feature(ADMIN_UTILS_FEATURE),
                    room_history: keeps_room_history(client).await?,
                },
            };
            let server = if plugins.screepsmod_admin_utils {
                serde_json::from_value::<screepsmod_admin_utils::types::PlatformVersion>(reply)
                    .map_err(decode)?
                    .into()
            } else {
                ServerInfo::from_community(
                    serde_json::from_value::<community::types::PlatformVersion>(reply)
                        .map_err(decode)?,
                )
            };
            (plugins, server)
        }
    };
    Ok(ConnectionInfo {
        kind,
        plugins,
        forced: forced_kind.is_some(),
        server,
    })
}

/// Whether the community server runs screepsmod-auth: its `GET /api/authmod` answers. Any
/// error reply (HTTP 404 without the plugin) means it does not; transport failures are
/// returned.
async fn runs_screepsmod_auth(client: &Client) -> Result<bool, Error> {
    match client
        .send_without_sign_in(&screepsmod_auth::endpoints::AuthModInfo)
        .await
    {
        Ok(_) => Ok(true),
        Err(Error::Api(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

/// Whether the community server runs a room-history mod: its `GET /room-history` answers a
/// chunk, or HTTP 500 with anything but the stock provider's [`STOCK_ROOM_HISTORY`] (a mod's
/// own error for a chunk it does not hold; backend-local answers every provider error with
/// HTTP 500). Other error replies do not come from the route and tell nothing of a mod;
/// transport failures are returned.
async fn keeps_room_history(client: &Client) -> Result<bool, Error> {
    match client.send_without_sign_in(&RoomHistoryProbe).await {
        Ok(_) => Ok(true),
        Err(Error::Api(ApiError::Status { status, message }))
            if status == http::StatusCode::INTERNAL_SERVER_ERROR =>
        {
            Ok(message.as_deref() != Some(STOCK_ROOM_HISTORY))
        }
        Err(Error::Api(_)) => Ok(false),
        Err(error) => Err(error),
    }
}
