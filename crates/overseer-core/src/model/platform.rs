//! Platform: which kind of server Overseer talks to, the plugins it runs, the subsystems they
//! make up and what its `GET /api/version` reports.
//!
//! A client decides the [`ServerKind`] before it knows the contract: official servers by their
//! address ([`ServerUrl::is_official`](crate::ServerUrl::is_official)), every other server by
//! its `GET /api/version` reply, read with the tolerant [`Discovery`]. The reply is then read
//! again with the chosen contract's type and converted into [`ServerInfo`]. A caller may force
//! the kind (and the plugins) instead.
//!
//! Callers never branch on the kind or the plugins for behaviour: they ask for the
//! [`Subsystem`]s the server offers ([`Subsystems`], computed from the kind and the plugins)
//! and use the operations every server offers alike. [`ConnectionInfo`] holds the kind, the
//! plugins and the `GET /api/version` reply for display and diagnostics.

use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;

use crate::api::community::types as community;
use crate::api::official::types as official;
use crate::api::openscreeps::types as openscreeps;
use crate::api::plugins::screepsmod_admin_utils::types as admin_utils;

/// Renderer additions of server mods (`serverData.renderer`): metadata of extra object types and
/// extra textures. Every contract's `/api/version` reply holds this type (codegen re-exports the
/// official one from the others).
pub use crate::api::official::types::PlatformVersionServerDataRenderer as Renderer;

/// The feature an `OpenScreeps` server lists in `serverData.features` (`{name: "openscreeps",
/// version: 1}`); a server without it is not `OpenScreeps`.
pub const OPENSCREEPS_FEATURE: &str = "openscreeps";

/// The feature screepsmod-admin-utils lists in `serverData.features`.
pub const ADMIN_UTILS_FEATURE: &str = "screepsmod-admin-utils";

/// The contract a server speaks: which generated client talks to it. For display, diagnostics
/// and forcing only: callers ask for [`Subsystems`] instead of branching on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ServerKind {
    /// screeps.com and its seasonal world: the official contract, several shards.
    Official,
    /// An `OpenScreeps` server: the official contract composed with the standalone profile.
    OpenScreeps,
    /// The stock Screeps server (`screeps` with backend-local), possibly with plugins
    /// ([`Plugins`]).
    Community,
}

impl ServerKind {
    /// The kind of a server that is not official, from its `GET /api/version`: `OpenScreeps`
    /// when it lists [`OPENSCREEPS_FEATURE`], else the community server. Official servers are
    /// known by their address instead ([`ServerUrl::is_official`](crate::ServerUrl::is_official)),
    /// also behind a proxy.
    #[must_use]
    pub fn of(discovery: &Discovery) -> Self {
        if discovery.has_feature(OPENSCREEPS_FEATURE) {
            Self::OpenScreeps
        } else {
            Self::Community
        }
    }

    /// The kind's name for display: `official`, `OpenScreeps` or `community`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Official => "official",
            Self::OpenScreeps => "OpenScreeps",
            Self::Community => "community",
        }
    }
}

impl fmt::Display for ServerKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The optional plugins a community server runs, each with its own contract; none on the
/// other kinds, whose contracts include what they offer. For display, diagnostics and forcing
/// only: what they add is told by [`Subsystems`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Plugins {
    /// screepsmod-auth: password sign-in, registration, password changes, API tokens and
    /// lifting their rate limits. Its `GET /api/authmod` answers.
    pub screepsmod_auth: bool,
    /// screepsmod-admin-utils: room objects, an adapted first spawn and start room, and an
    /// extended `GET /api/version`. It lists [`ADMIN_UTILS_FEATURE`].
    pub screepsmod_admin_utils: bool,
    /// A room-history mod (for example screepsmod-history): it replaces the stock server's
    /// history provider, so `GET /room-history` answers other than the stock HTTP 500
    /// [`STOCK_ROOM_HISTORY`]. Such mods list no feature.
    pub room_history: bool,
}

impl Plugins {
    /// No plugin.
    pub const NONE: Self = Self {
        screepsmod_auth: false,
        screepsmod_admin_utils: false,
        room_history: false,
    };

    /// The names of the plugins that run, for display.
    pub fn names(self) -> impl Iterator<Item = &'static str> {
        [
            (self.screepsmod_auth, "screepsmod-auth"),
            (self.screepsmod_admin_utils, ADMIN_UTILS_FEATURE),
            (self.room_history, "room-history mod"),
        ]
        .into_iter()
        .filter_map(|(runs, name)| runs.then_some(name))
    }
}

/// The body of the stock community server's HTTP 500 reply to every `GET /room-history`: its
/// default history provider keeps none (backend-local `lib/game/server.js`).
pub const STOCK_ROOM_HISTORY: &str = "not implemented";

/// Something a server may or may not offer, named by what it does: the ways the contracts
/// (official, `OpenScreeps`, community and its plugins) differ in Overseer's operations. A
/// server's set is [`Subsystems`]; an operation of an absent subsystem fails without a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Subsystem {
    /// Signing in with an e-mail address (or a username) and a password
    /// (`POST /api/auth/signin`): screeps.com, `OpenScreeps`, screepsmod-auth.
    PasswordSignIn,
    /// Accounts of the server's own with a password, through its API: registration
    /// (`POST /api/register/submit`) and password changes (`POST /api/user/password`):
    /// `OpenScreeps`, screepsmod-auth. screeps.com does both on its website
    /// ([`Self::AccountWebsite`]).
    PasswordAccounts,
    /// Persistent API tokens of the account (`GET`/`POST /api/user/auth-token`,
    /// `POST /api/user/delete-auth-token`): screeps.com, screepsmod-auth.
    ApiTokens,
    /// Lifting the rate limits of an API token for a while (`noRatelimitUntil`): on a web page
    /// (screeps.com, a reCAPTCHA) or with one request (screepsmod-auth's
    /// `POST /api/user/noratelimit`).
    RateLimitLift,
    /// The signed-in user's username alone (`GET /api/user/name`), which restricted API tokens
    /// may read too: screeps.com, `OpenScreeps`, screepsmod-auth.
    UsernameLookup,
    /// A website with pages to register, recover a password and manage API tokens:
    /// screeps.com.
    AccountWebsite,
    /// Every object of a room over HTTP (`GET /api/game/room-objects`): screeps.com,
    /// `OpenScreeps`, screepsmod-admin-utils.
    RoomObjects,
    /// Room history chunks: screeps.com, `OpenScreeps`, community servers with a room-history
    /// mod. The stock community server keeps none (it answers every chunk with HTTP 500
    /// [`STOCK_ROOM_HISTORY`]).
    RoomHistory,
    /// The decorations players activated in a room (`GET /api/game/room-decorations`):
    /// screeps.com, `OpenScreeps`.
    RoomDecorations,
    /// Finding the room of an object by its id (`GET /api/game/find-object`): screeps.com,
    /// `OpenScreeps`.
    ObjectSearch,
    /// A random room of a novice area to start in (`GET /api/game/random-novice-room`):
    /// screeps.com, `OpenScreeps`.
    NoviceRooms,
}

impl Subsystem {
    /// Every subsystem, in declaration order.
    pub const ALL: [Self; 11] = [
        Self::PasswordSignIn,
        Self::PasswordAccounts,
        Self::ApiTokens,
        Self::RateLimitLift,
        Self::UsernameLookup,
        Self::AccountWebsite,
        Self::RoomObjects,
        Self::RoomHistory,
        Self::RoomDecorations,
        Self::ObjectSearch,
        Self::NoviceRooms,
    ];

    /// What the subsystem does, for messages: `password sign-in`, `room history`, ….
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::PasswordSignIn => "password sign-in",
            Self::PasswordAccounts => "password accounts",
            Self::ApiTokens => "API tokens",
            Self::RateLimitLift => "lifting rate limits",
            Self::UsernameLookup => "the username lookup",
            Self::AccountWebsite => "an account website",
            Self::RoomObjects => "room objects",
            Self::RoomHistory => "room history",
            Self::RoomDecorations => "room decorations",
            Self::ObjectSearch => "object search",
            Self::NoviceRooms => "random novice rooms",
        }
    }

    /// The subsystem's bit in [`Subsystems`].
    const fn bit(self) -> u16 {
        1 << self as u16
    }
}

impl fmt::Display for Subsystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The [`Subsystem`]s a server offers: what callers decide behaviour by, never the
/// [`ServerKind`] or the [`Plugins`] themselves.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Subsystems(u16);

impl Subsystems {
    /// No subsystem.
    pub const NONE: Self = Self(0);

    /// What every server of `kind` offers.
    const fn base(kind: ServerKind) -> Self {
        match kind {
            ServerKind::Official => Self::NONE
                .with(Subsystem::PasswordSignIn)
                .with(Subsystem::ApiTokens)
                .with(Subsystem::RateLimitLift)
                .with(Subsystem::UsernameLookup)
                .with(Subsystem::AccountWebsite)
                .with(Subsystem::RoomObjects)
                .with(Subsystem::RoomHistory)
                .with(Subsystem::RoomDecorations)
                .with(Subsystem::ObjectSearch)
                .with(Subsystem::NoviceRooms),
            ServerKind::OpenScreeps => Self::NONE
                .with(Subsystem::PasswordSignIn)
                .with(Subsystem::PasswordAccounts)
                .with(Subsystem::UsernameLookup)
                .with(Subsystem::RoomObjects)
                .with(Subsystem::RoomHistory)
                .with(Subsystem::RoomDecorations)
                .with(Subsystem::ObjectSearch)
                .with(Subsystem::NoviceRooms),
            ServerKind::Community => Self::NONE,
        }
    }

    /// What a server speaking `kind`'s contract offers with `plugins` (which only community
    /// servers run; ignored for the other kinds):
    ///
    /// - screeps.com: everything but [`Subsystem::PasswordAccounts`] (its website registers
    ///   accounts and changes passwords).
    /// - `OpenScreeps`: everything but [`Subsystem::ApiTokens`], [`Subsystem::RateLimitLift`]
    ///   and [`Subsystem::AccountWebsite`].
    /// - The stock community server: none; screepsmod-auth adds password sign-in, password
    ///   accounts, API tokens, lifting their rate limits and the username lookup,
    ///   screepsmod-admin-utils room objects, a room-history mod room history.
    #[must_use]
    pub const fn of(kind: ServerKind, plugins: Plugins) -> Self {
        let base = Self::base(kind);
        if !matches!(kind, ServerKind::Community) {
            return base;
        }
        let base = if plugins.screepsmod_auth {
            base.with(Subsystem::PasswordSignIn)
                .with(Subsystem::PasswordAccounts)
                .with(Subsystem::ApiTokens)
                .with(Subsystem::RateLimitLift)
                .with(Subsystem::UsernameLookup)
        } else {
            base
        };
        let base = if plugins.screepsmod_admin_utils {
            base.with(Subsystem::RoomObjects)
        } else {
            base
        };
        if plugins.room_history {
            base.with(Subsystem::RoomHistory)
        } else {
            base
        }
    }

    /// This set with `subsystem`.
    #[must_use]
    pub const fn with(self, subsystem: Subsystem) -> Self {
        Self(self.0 | subsystem.bit())
    }

    /// This set without `subsystem`.
    #[must_use]
    pub const fn without(self, subsystem: Subsystem) -> Self {
        Self(self.0 & !subsystem.bit())
    }

    /// Whether the set holds `subsystem`.
    #[must_use]
    pub const fn contains(self, subsystem: Subsystem) -> bool {
        self.0 & subsystem.bit() != 0
    }

    /// Whether the set is empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The subsystems of the set, in declaration order.
    pub fn iter(self) -> impl Iterator<Item = Subsystem> {
        Subsystem::ALL
            .into_iter()
            .filter(move |subsystem| self.contains(*subsystem))
    }
}

impl fmt::Debug for Subsystems {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

impl FromIterator<Subsystem> for Subsystems {
    fn from_iter<I: IntoIterator<Item = Subsystem>>(subsystems: I) -> Self {
        subsystems.into_iter().fold(Self::NONE, Self::with)
    }
}

/// What a client knows about its connection: the contract the server speaks, the plugins it
/// runs and its `GET /api/version`, discovered or forced.
///
/// For display and diagnostics only: callers must not branch on [`Self::kind`] or
/// [`Self::plugins`] for behaviour, but ask for [`Self::subsystems`] (or the subsystem
/// accessors of the client's API) and use the operations every server offers alike.
#[derive(Clone, Debug, PartialEq)]
pub struct ConnectionInfo {
    /// The contract the server speaks.
    pub kind: ServerKind,
    /// The plugins it runs (community servers).
    pub plugins: Plugins,
    /// Whether the caller forced the kind instead of having it discovered.
    pub forced: bool,
    /// What the server reports in `GET /api/version`.
    pub server: ServerInfo,
}

impl ConnectionInfo {
    /// The subsystems the server offers ([`Subsystems::of`] its kind and plugins).
    #[must_use]
    pub const fn subsystems(&self) -> Subsystems {
        Subsystems::of(self.kind, self.plugins)
    }
}

/// What a server reports about itself in `GET /api/version`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ServerInfo {
    /// Protocol version (14 on every current server).
    pub protocol: i64,
    /// Number of active players.
    pub users: i64,
    /// Build number of the official server (`package`); `None` elsewhere.
    pub package: Option<i64>,
    /// Version of the installed server package (`packageVersion`), when the server reports
    /// it.
    pub package_version: Option<String>,
    /// Ticks per room-history chunk (`serverData.historyChunkSize`); chunks start at multiples
    /// of it. At least 1, whatever the server reports.
    pub history_chunk_size: u64,
    /// Minimum milliseconds between two updates of a socket channel
    /// (`serverData.socketUpdateThrottle`), when reported.
    pub socket_update_throttle: Option<f64>,
    /// HTML greeting of the server (`serverData.welcomeText`), when it has one.
    pub welcome_text: Option<String>,
    /// Renderer additions of server mods (`serverData.renderer`).
    pub renderer: Renderer,
    /// Room object types added by server mods, keyed by type name; values are mod-defined
    /// descriptors (`serverData.customObjectTypes`).
    pub custom_object_types: BTreeMap<String, serde_json::Value>,
    /// The shards channels, room history and requests name, in the order the server lists them
    /// (`serverData.shards`). Official servers only: every other server serves one world and
    /// names no shard, whatever it lists (screepsmod-admin-utils lists its `shardName`
    /// setting, which only `Game.shard.name` reports), so it is empty for them.
    pub shards: Vec<String>,
    /// Names of the features the server lists (`serverData.features`), in its order: for
    /// example `market` and `auth`, [`OPENSCREEPS_FEATURE`], or the features of community
    /// server mods.
    pub features: Vec<String>,
}

impl ServerInfo {
    /// Whether the server lists the feature `name` ([`Self::features`]).
    #[must_use]
    pub fn has_feature(&self, name: &str) -> bool {
        self.features.iter().any(|feature| feature == name)
    }
}

/// The part of a `GET /api/version` reply that decides the [`ServerKind`] and the [`Plugins`]
/// it lists, read before the contract is known: the names of `serverData.features`. A missing
/// or `null` `serverData` or `features`, and features without a name, read as none; everything
/// else is ignored.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(from = "RawDiscovery")]
pub struct Discovery {
    features: Vec<String>,
}

impl Discovery {
    /// Reads a `GET /api/version` reply.
    ///
    /// # Errors
    ///
    /// Returns the [`serde_json::Error`] of a reply whose `serverData.features` names are not
    /// strings.
    pub fn from_reply(reply: &serde_json::Value) -> Result<Self, serde_json::Error> {
        Self::deserialize(reply)
    }

    /// The names of the features the server lists, in its order.
    #[must_use]
    pub fn features(&self) -> &[String] {
        &self.features
    }

    /// Whether the server lists the feature `name`.
    #[must_use]
    pub fn has_feature(&self, name: &str) -> bool {
        self.features.iter().any(|feature| feature == name)
    }
}

/// The wire shape [`Discovery`] reads.
#[derive(Deserialize)]
struct RawDiscovery {
    #[serde(rename = "serverData", default)]
    server_data: Option<RawServerData>,
}

/// `serverData`, of which [`Discovery`] reads the features.
#[derive(Deserialize)]
struct RawServerData {
    #[serde(default)]
    features: Option<Vec<RawFeature>>,
}

/// One entry of `serverData.features`.
#[derive(Deserialize)]
struct RawFeature {
    #[serde(default)]
    name: Option<String>,
}

impl From<RawDiscovery> for Discovery {
    fn from(raw: RawDiscovery) -> Self {
        let features = raw
            .server_data
            .and_then(|data| data.features)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|feature| feature.name)
            .collect();
        Self { features }
    }
}

/// Ticks per history chunk: at least 1, whatever the server reports.
fn chunk_size(reported: i64) -> u64 {
    u64::try_from(reported).unwrap_or(1).max(1)
}

impl ServerInfo {
    /// A community server's reply (the stock Screeps server without screepsmod-admin-utils):
    /// the community contract shares the official reply type, but a community server serves one
    /// world, so the shards it may list are dropped.
    #[must_use]
    pub fn from_community(version: community::PlatformVersion) -> Self {
        Self {
            shards: Vec::new(),
            ..Self::from(version)
        }
    }
}

impl From<official::PlatformVersion> for ServerInfo {
    /// screeps.com's reply, with the shards requests name. A community server's reply has this
    /// type too: read it with [`ServerInfo::from_community`].
    fn from(version: official::PlatformVersion) -> Self {
        let data = version.server_data;
        Self {
            protocol: version.protocol,
            users: version.users,
            package: version.package,
            package_version: version.package_version,
            history_chunk_size: chunk_size(data.history_chunk_size),
            socket_update_throttle: data.socket_update_throttle,
            welcome_text: data.welcome_text,
            renderer: data.renderer,
            custom_object_types: data.custom_object_types,
            shards: data.shards.unwrap_or_default(),
            features: data
                .features
                .unwrap_or_default()
                .into_iter()
                .map(|feature| feature.name)
                .collect(),
        }
    }
}

impl From<openscreeps::PlatformVersion> for ServerInfo {
    /// An `OpenScreeps` server's reply: one world, no shards.
    fn from(version: openscreeps::PlatformVersion) -> Self {
        let data = version.server_data;
        Self {
            protocol: version.protocol,
            users: version.users,
            package: version.package,
            package_version: version.package_version,
            history_chunk_size: chunk_size(data.history_chunk_size),
            socket_update_throttle: data.socket_update_throttle,
            welcome_text: data.welcome_text,
            renderer: data.renderer,
            custom_object_types: data.custom_object_types,
            shards: Vec::new(),
            features: data
                .features
                .into_iter()
                .map(|feature| feature.name)
                .collect(),
        }
    }
}

impl From<admin_utils::PlatformVersion> for ServerInfo {
    /// The reply of a community server with screepsmod-admin-utils: one world, whatever
    /// `shardName` it lists.
    fn from(version: admin_utils::PlatformVersion) -> Self {
        let data = version.server_data;
        Self {
            protocol: version.protocol,
            users: version.users,
            package: version.package,
            package_version: version.package_version,
            history_chunk_size: chunk_size(data.history_chunk_size),
            socket_update_throttle: data.socket_update_throttle,
            welcome_text: data.welcome_text,
            renderer: data.renderer,
            custom_object_types: data.custom_object_types,
            shards: Vec::new(),
            features: data
                .features
                .unwrap_or_default()
                .into_iter()
                .map(|feature| feature.name)
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ConnectionInfo, Discovery, Plugins, ServerInfo, ServerKind, Subsystem, Subsystems,
    };
    use crate::api::community::types as community;
    use crate::api::official::types as official;
    use crate::api::openscreeps::types as openscreeps;
    use crate::api::plugins::screepsmod_admin_utils::types as admin_utils;

    /// screeps.com's `GET /api/version` of 2026-10-16, with its HTML templates shortened.
    const OFFICIAL: &str = r#"{"ok":1,"package":247,"protocol":14,
        "serverData":{"customObjectTypes":{},"historyChunkSize":100,
            "renderer":{"resources":{},"metadata":{}},
            "features":[{"name":"auth","version":1},
                {"name":"market","version":1,"menuData":[{"section":0,"after":"World",
                    "item":{"label":"Market","routerLink":"/market","svg":"market"}}]},
                {"name":"inventory","version":1,"menuData":[{"section":0,"after":"World",
                    "item":{"id":"menu-item-inventory","label":"Inventory",
                        "routerLink":"/inventory","svg":"inventory"},
                    "module":"InventoryModule"}]},
                {"name":"persistent-world","version":1,
                    "seasonHint":"<p>The season entry fee is changed …</p>",
                    "menuData":[{"section":0,"start":0,"item":{
                            "conditions":["isNw","isSeasonAccessGranted"],
                            "class":"--season-linear-gradient","label":"Season world",
                            "svg":"season-world",
                            "config":{"API_URL":"https://screeps.com/season/api/",
                                "HISTORY_URL":"https://screeps.com/season/room-history/",
                                "WEBSOCKET_URL":"https://screeps.com/season/socket/"},
                            "redirectUrl":"/seasons/chronicle",
                            "templateBefore":"<div>Switch to</div>","template":"<div></div>"}},
                        {"section":0,"start":3,"item":{
                            "template":"<div class=\"--color-text-20\">Persistent world:</div>"}}]}],
            "shards":["shard0","shard1","shard2","shard3","shardX"]},
        "currentSeason":"11",
        "seasonAccessCost":{"1786752000":5,"1788220800":4,"1789430400":3},
        "decorationConvertationCost":400,"decorationPixelizationCost":500,"users":1802}"#;

    /// An `OpenScreeps` server's reply (openscreeps/server `c8edb29`, default configuration).
    const OPENSCREEPS: &str = r#"{"ok":1,"protocol":14,"useNativeAuth":false,"users":2,
        "packageVersion":"0.0.0",
        "serverData":{"welcomeText":"<h4>OpenScreeps</h4><p>A standalone Screeps world.</p>",
            "customObjectTypes":{},"historyChunkSize":20,"socketUpdateThrottle":200,
            "renderer":{"resources":{},"metadata":{}},
            "features":[{"name":"market","version":1,"menuData":[{"section":0,"after":"World",
                    "item":{"label":"Market","routerLink":"/market","svg":"market"}}]},
                {"name":"auth","version":1},{"name":"openscreeps","version":1}]}}"#;

    /// The stock Screeps server's reply (backend-local, no mods): no features, no shards.
    const STOCK: &str = r#"{"ok":1,"protocol":14,"useNativeAuth":false,"users":3,
        "serverData":{"welcomeText":"<h4>Welcome to your own Screeps private server!</h4>",
            "customObjectTypes":{},"historyChunkSize":20,"socketUpdateThrottle":100,
            "renderer":{"resources":{},"metadata":{}}},
        "packageVersion":"4.2.21"}"#;

    /// A community server's reply with screepsmod-admin-utils and screepsmod-auth: an unset
    /// `shardName` (`[null]`) and mod features with a package version or none.
    const ADMIN_UTILS: &str = r#"{"ok":1,"protocol":14,"useNativeAuth":false,"users":1,
        "serverData":{"shards":[null],"customObjectTypes":{"beacon":{"name":"beacon"}},
            "historyChunkSize":20,"socketUpdateThrottle":1000,"welcomeText":"<h4>Hi</h4>",
            "renderer":{"resources":{"beacon":"{ASSETS_URL}/beacon.png"},"metadata":{}},
            "features":[{"name":"auth","version":1},{"name":"auth-tokens"},
                {"name":"screepsmod-auth","version":"2.10.1","authTypes":["password"]},
                {"name":"screepsmod-admin-utils","version":"1.37.1"}]}}"#;

    /// Reads `reply` as a client does: decoded as JSON, then read before the contract is known.
    fn discovery(reply: &str) -> Discovery {
        Discovery::from_reply(&serde_json::from_str(reply).unwrap()).unwrap()
    }

    #[test]
    fn discovery_tells_openscreeps_by_its_marker_and_community_servers_otherwise() {
        assert_eq!(
            ServerKind::of(&discovery(OPENSCREEPS)),
            ServerKind::OpenScreeps
        );
        assert_eq!(
            discovery(OPENSCREEPS).features(),
            ["market", "auth", "openscreeps"]
        );
        let stock = discovery(STOCK);
        assert_eq!(ServerKind::of(&stock), ServerKind::Community);
        assert_eq!(stock.features(), [] as [String; 0]);
        assert!(!stock.has_feature(super::ADMIN_UTILS_FEATURE));
        let mods = discovery(ADMIN_UTILS);
        assert_eq!(ServerKind::of(&mods), ServerKind::Community);
        assert!(mods.has_feature(super::ADMIN_UTILS_FEATURE));
        // The official reply lists no marker either; official servers are known by address.
        assert_eq!(ServerKind::of(&discovery(OFFICIAL)), ServerKind::Community);
    }

    #[test]
    fn discovery_reads_what_no_contract_would() {
        for reply in [
            r#"{"ok":1}"#,
            r#"{"ok":1,"serverData":null}"#,
            r#"{"ok":1,"serverData":{"features":null}}"#,
            r#"{"ok":1,"serverData":{"features":[]}}"#,
        ] {
            assert_eq!(discovery(reply), Discovery::default(), "{reply}");
        }
        let odd = discovery(
            r#"{"serverData":{"features":[{"version":"x"},{"name":"openscreeps","version":"2"}]}}"#,
        );
        assert_eq!(odd.features(), ["openscreeps"]);
        assert_eq!(ServerKind::of(&odd), ServerKind::OpenScreeps);
    }

    #[test]
    fn official_reply_converts_with_its_shards_and_features() {
        let version: official::PlatformVersion = serde_json::from_str(OFFICIAL).unwrap();
        let info = ServerInfo::from(version);
        assert_eq!(info.protocol, 14);
        assert_eq!(info.users, 1802);
        assert_eq!(info.package, Some(247));
        assert_eq!(info.package_version, None);
        assert_eq!(info.history_chunk_size, 100);
        assert_eq!(info.socket_update_throttle, None);
        assert_eq!(info.welcome_text, None);
        assert_eq!(
            info.shards,
            ["shard0", "shard1", "shard2", "shard3", "shardX"]
        );
        assert_eq!(
            info.features,
            ["auth", "market", "inventory", "persistent-world"]
        );
        assert!(info.has_feature("market"));
        assert!(info.custom_object_types.is_empty());
        assert!(info.renderer.resources.is_empty());
    }

    #[test]
    fn openscreeps_reply_converts_without_shards() {
        let version: openscreeps::PlatformVersion = serde_json::from_str(OPENSCREEPS).unwrap();
        let info = ServerInfo::from(version);
        assert_eq!(info.protocol, 14);
        assert_eq!(info.users, 2);
        assert_eq!(info.package, None);
        assert_eq!(info.package_version.as_deref(), Some("0.0.0"));
        assert_eq!(info.history_chunk_size, 20);
        assert_eq!(info.socket_update_throttle, Some(200.0));
        assert_eq!(
            info.welcome_text.as_deref(),
            Some("<h4>OpenScreeps</h4><p>A standalone Screeps world.</p>")
        );
        assert_eq!(info.shards, [] as [String; 0]);
        assert_eq!(info.features, ["market", "auth", "openscreeps"]);
    }

    #[test]
    fn stock_community_reply_converts_without_features_or_shards() {
        let version: community::PlatformVersion = serde_json::from_str(STOCK).unwrap();
        let info = ServerInfo::from_community(version);
        assert_eq!(info.users, 3);
        assert_eq!(info.package_version.as_deref(), Some("4.2.21"));
        assert_eq!(info.history_chunk_size, 20);
        assert_eq!(info.socket_update_throttle, Some(100.0));
        assert_eq!(info.shards, [] as [String; 0]);
        assert_eq!(info.features, [] as [String; 0]);
        // Shards a community server lists (stock sends none) are no shards requests name.
        let listed = STOCK.replace(r#""serverData":{"#, r#""serverData":{"shards":["shard0"],"#);
        let version: community::PlatformVersion = serde_json::from_str(&listed).unwrap();
        assert_eq!(
            ServerInfo::from_community(version).shards,
            [] as [String; 0]
        );
    }

    #[test]
    fn admin_utils_reply_converts_without_its_shard_name() {
        // An unset (`null`) and a set `shardName`: neither is a shard requests name.
        for shards in ["[null]", r#"["shard0"]"#] {
            let reply = ADMIN_UTILS.replace("[null]", shards);
            let version: admin_utils::PlatformVersion = serde_json::from_str(&reply).unwrap();
            let info = ServerInfo::from(version);
            assert!(info.shards.is_empty(), "{shards}");
            assert_eq!(info.history_chunk_size, 20);
            assert_eq!(info.socket_update_throttle, Some(1000.0));
            assert_eq!(info.welcome_text.as_deref(), Some("<h4>Hi</h4>"));
            assert_eq!(
                info.features,
                [
                    "auth",
                    "auth-tokens",
                    "screepsmod-auth",
                    "screepsmod-admin-utils"
                ]
            );
            assert_eq!(
                info.renderer.resources.get("beacon").map(String::as_str),
                Some("{ASSETS_URL}/beacon.png")
            );
            assert!(info.custom_object_types.contains_key("beacon"));
        }
        // The community contract alone cannot read the mods' feature versions.
        assert!(serde_json::from_str::<community::PlatformVersion>(ADMIN_UTILS).is_err());
    }

    #[test]
    fn history_chunk_size_is_at_least_one() {
        for reported in [0, -5] {
            let reply = STOCK.replace(
                r#""historyChunkSize":20"#,
                &format!(r#""historyChunkSize":{reported}"#),
            );
            let version: community::PlatformVersion = serde_json::from_str(&reply).unwrap();
            assert_eq!(ServerInfo::from_community(version).history_chunk_size, 1);
        }
    }

    /// The subsystems of `kind` with `plugins`, as a sorted list.
    fn subsystems(kind: ServerKind, plugins: Plugins) -> Vec<Subsystem> {
        Subsystems::of(kind, plugins).iter().collect()
    }

    #[test]
    fn subsystems_follow_the_kind_and_the_plugins() {
        use Subsystem::{
            AccountWebsite, ApiTokens, NoviceRooms, ObjectSearch, PasswordAccounts, PasswordSignIn,
            RateLimitLift, RoomDecorations, RoomHistory, RoomObjects, UsernameLookup,
        };
        let auth = Plugins {
            screepsmod_auth: true,
            ..Plugins::NONE
        };
        let admin_utils = Plugins {
            screepsmod_admin_utils: true,
            ..Plugins::NONE
        };
        let history = Plugins {
            room_history: true,
            ..Plugins::NONE
        };
        let both = Plugins {
            screepsmod_auth: true,
            screepsmod_admin_utils: true,
            room_history: false,
        };
        let official = [
            PasswordSignIn,
            ApiTokens,
            RateLimitLift,
            UsernameLookup,
            AccountWebsite,
            RoomObjects,
            RoomHistory,
            RoomDecorations,
            ObjectSearch,
            NoviceRooms,
        ];
        let openscreeps = [
            PasswordSignIn,
            PasswordAccounts,
            UsernameLookup,
            RoomObjects,
            RoomHistory,
            RoomDecorations,
            ObjectSearch,
            NoviceRooms,
        ];
        let screepsmod_auth = [
            PasswordSignIn,
            PasswordAccounts,
            ApiTokens,
            RateLimitLift,
            UsernameLookup,
        ];
        // Plugins only extend community servers.
        for plugins in [Plugins::NONE, both] {
            assert_eq!(subsystems(ServerKind::Official, plugins), official);
            assert_eq!(subsystems(ServerKind::OpenScreeps, plugins), openscreeps);
        }
        assert_eq!(subsystems(ServerKind::Community, Plugins::NONE), []);
        assert_eq!(subsystems(ServerKind::Community, auth), screepsmod_auth);
        assert_eq!(
            subsystems(ServerKind::Community, admin_utils),
            [RoomObjects]
        );
        assert_eq!(subsystems(ServerKind::Community, history), [RoomHistory]);
        let mut with_both = screepsmod_auth.to_vec();
        with_both.push(RoomObjects);
        assert_eq!(subsystems(ServerKind::Community, both), with_both);
    }

    #[test]
    fn subsystem_sets_add_remove_and_list_in_order() {
        let set: Subsystems = [Subsystem::RoomHistory, Subsystem::PasswordSignIn]
            .into_iter()
            .collect();
        assert!(set.contains(Subsystem::RoomHistory));
        assert!(!set.contains(Subsystem::ApiTokens));
        assert_eq!(
            set.iter().collect::<Vec<_>>(),
            [Subsystem::PasswordSignIn, Subsystem::RoomHistory]
        );
        assert_eq!(format!("{set:?}"), "{PasswordSignIn, RoomHistory}");
        let set = set
            .without(Subsystem::RoomHistory)
            .without(Subsystem::PasswordSignIn);
        assert!(set.is_empty());
        assert_eq!(set, Subsystems::NONE);
        assert_eq!(
            Subsystem::ALL
                .into_iter()
                .collect::<Subsystems>()
                .iter()
                .count(),
            Subsystem::ALL.len()
        );
        assert_eq!(Subsystem::RateLimitLift.to_string(), "lifting rate limits");
    }

    #[test]
    fn connection_info_names_its_kind_and_plugins_and_derives_subsystems() {
        let info = ConnectionInfo {
            kind: ServerKind::Community,
            plugins: Plugins {
                screepsmod_auth: true,
                screepsmod_admin_utils: true,
                room_history: false,
            },
            forced: false,
            server: ServerInfo::default(),
        };
        assert_eq!(info.kind.to_string(), "community");
        assert_eq!(
            info.plugins.names().collect::<Vec<_>>(),
            ["screepsmod-auth", "screepsmod-admin-utils"]
        );
        assert_eq!(Plugins::NONE.names().count(), 0);
        assert_eq!(
            info.subsystems(),
            Subsystems::of(ServerKind::Community, info.plugins)
        );
        assert!(info.subsystems().contains(Subsystem::RoomObjects));
        assert!(!info.subsystems().contains(Subsystem::RoomHistory));
    }
}
