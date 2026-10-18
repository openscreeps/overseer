//! Account: the signed-in user's profile (`GET /api/auth/me`), e-mail notification
//! preferences, persistent API tokens, registration, world status and start room, and the
//! statistics overview. The rules of a new account's fields are in [`check_username`],
//! [`check_email`] and [`check_password`]; the allocation of the account's CPU to the shards
//! is in [`cpu`].

pub mod cpu;
mod registration;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;

use crate::api::community::types as community;
use crate::api::official::types as official;
use crate::api::openscreeps::types as openscreeps;
use crate::api::plugins::screepsmod_admin_utils::types as admin_utils;
use crate::model::code::Modules;

pub use registration::{
    EmailError, PASSWORD_MAX_UNITS, PASSWORD_MIN_UNITS, PasswordError, RegistrationErrors,
    USERNAME_MIN, UsernameError, check_email, check_password, check_username,
};

/// Every contract's `notifyPrefs` and badge are this type (codegen re-exports the official
/// one from the others), and so is every type a badge holds.
pub use crate::api::official::types::{
    UserBadge, UserBadgeColor, UserBadgeCustomPaths, UserBadgeType, UserNotifyPrefs,
};

/// Every contract's new badge (`POST /api/user/badge`) is this type (codegen re-exports the
/// official one from the others), and so is every type it holds.
pub use crate::api::official::types::{UserBadgeInput, UserBadgeInputParam, UserBadgeInputType};

/// Every contract's world status (`GET /api/user/world-status`) is this type (codegen
/// re-exports the official one from the others).
pub use crate::api::official::types::UserWorldStatus;

/// Every contract's room statistic names, statistic totals and overview points are these
/// types (codegen re-exports the official ones from the others).
pub use crate::api::official::types::{UserOverviewPoint, UserRoomStatName, UserRoomStatValues};

/// The signed-in user's private profile (`GET /api/auth/me`).
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    /// The user's id (`_id`), the key of user channels.
    pub id: String,
    /// The username; `None` until the account has chosen one (after a Steam or GitHub
    /// sign-up).
    pub username: Option<String>,
    /// The confirmed e-mail address, lower-cased; `None` when none is set.
    pub email: Option<String>,
    /// An e-mail address awaiting confirmation (official server).
    pub email_dirty: Option<String>,
    /// Whether password sign-in is configured; `None` when the server does not say.
    pub has_password: Option<bool>,
    /// The user's badge, once chosen.
    pub badge: Option<UserBadge>,
    /// Credits.
    pub money: f64,
    /// CPU per tick available to the account, when reported.
    pub cpu: Option<f64>,
    /// The CPU limit of each shard, by shard name (`cpuShard`): screeps.com and its Season
    /// server. Empty when the server does not report it. Change it with
    /// `POST /api/user/cpu-shards`
    /// ([`Subsystem::CpuShards`](crate::model::platform::Subsystem::CpuShards)).
    pub cpu_shards: BTreeMap<String, f64>,
    /// When the CPU limits of the shards changed last, in milliseconds since the Unix epoch
    /// (`cpuShardUpdatedTime`), when reported. The server refuses a new change for 12 hours
    /// after it.
    pub cpu_shards_updated: Option<f64>,
    /// Lifetime control points (GCL progress), when reported.
    pub gcl: Option<f64>,
    /// Lifetime power processed (GPL progress), when reported.
    pub power: Option<f64>,
    /// E-mail notification preferences; defaults when none were saved.
    pub notify_prefs: UserNotifyPrefs,
}

/// `From` the `auth/me` reply of each contract. The replies differ only in fields Overseer does
/// not read (the Steam account's `ownership`); the community contract shares the official one.
macro_rules! profile_from {
    ($($reply:ty),+) => {$(
        impl From<$reply> for Profile {
            fn from(me: $reply) -> Self {
                Self {
                    id: me.id,
                    username: me.username,
                    email: me.email,
                    email_dirty: me.email_dirty,
                    has_password: me.password,
                    badge: me.badge,
                    money: me.money,
                    cpu: me.cpu,
                    cpu_shards: me.cpu_shard.unwrap_or_default(),
                    cpu_shards_updated: me.cpu_shard_updated_time,
                    gcl: me.gcl,
                    power: me.power,
                    notify_prefs: me.notify_prefs.unwrap_or_default(),
                }
            }
        }
    )+};
}

profile_from!(official::AuthMeResponse, openscreeps::AuthMeResponse);

/// One persistent API token of the account (`GET /api/user/auth-token`).
#[derive(Clone, PartialEq, Eq)]
pub struct AuthToken {
    /// The entry's id, which revokes the token.
    pub id: String,
    /// The description given at creation, if any.
    pub description: Option<String>,
    /// Whether the token may call every endpoint; otherwise it is limited to the endpoints,
    /// websockets and Memory segments chosen at creation.
    pub full: bool,
    /// Until when the token is exempt from rate limits, in milliseconds since the Unix epoch
    /// (screeps.com's `noRatelimitUntil`).
    pub no_ratelimit_until: Option<u64>,
    /// The token as listed: its first 8 characters on screeps.com.
    pub token: String,
}

impl fmt::Debug for AuthToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthToken")
            .field("id", &self.id)
            .field("description", &self.description)
            .field("full", &self.full)
            .field("no_ratelimit_until", &self.no_ratelimit_until)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// `From` one entry of the token list, which screeps.com and screepsmod-auth share.
impl From<official::UserAuthTokenListEntry> for AuthToken {
    fn from(entry: official::UserAuthTokenListEntry) -> Self {
        Self {
            id: entry.id,
            description: entry.description,
            full: entry.full,
            no_ratelimit_until: entry.no_ratelimit_until,
            token: entry.token,
        }
    }
}

/// The permissions of a new API token (`POST /api/user/auth-token`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthTokenRequest {
    /// The token type: `full` for every permission (the default of the official form).
    pub kind: Option<String>,
    /// Endpoint permissions keyed `METHOD /api/path`, for example `GET /api/user/memory`.
    pub endpoints: Option<BTreeMap<String, bool>>,
    /// Websocket permissions.
    pub websockets: Option<AuthTokenWebsockets>,
    /// The Memory segments the token may access, as entered (for example `0-9,20`).
    pub memory_segments: Option<String>,
}

/// The websocket permissions of a new API token.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AuthTokenWebsockets {
    /// The console channel.
    pub console: Option<bool>,
    /// The room channels.
    pub rooms: Option<bool>,
}

/// `From` an [`AuthTokenRequest`] into the request body screeps.com and screepsmod-auth share.
impl From<AuthTokenRequest> for official::UserAuthTokenCreateRequest<'_> {
    fn from(request: AuthTokenRequest) -> Self {
        Self {
            r#type: request.kind.map(Into::into),
            endpoints: request.endpoints.map(Cow::Owned),
            websockets: request.websockets.map(|websockets| {
                Cow::Owned(official::UserAuthTokenCreateRequestWebsockets {
                    console: websockets.console,
                    rooms: websockets.rooms,
                })
            }),
            memory_segments: request.memory_segments.map(Into::into),
        }
    }
}

/// A new account (`POST /api/register/submit`): `OpenScreeps`, and community servers with
/// screepsmod-auth. The account signs in afterwards with its username (or e-mail) and
/// password; registering issues no token.
#[derive(Clone, Default, PartialEq)]
pub struct Registration {
    /// The username: letters, digits, `_` and `-`, at least 3 characters (`OpenScreeps`;
    /// screepsmod-auth has no rules).
    pub username: String,
    /// The e-mail address.
    pub email: String,
    /// The password.
    pub password: String,
    /// The account's first code (its `default` branch); `None`: the server's default.
    pub modules: Option<Modules>,
}

impl fmt::Debug for Registration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Registration")
            .field("username", &self.username)
            .field("email", &self.email)
            .field("password", &"<redacted>")
            .field("modules", &self.modules)
            .finish()
    }
}

/// `From` a [`Registration`] into the request body of each contract offering registration:
/// `OpenScreeps`' and the official one, which screepsmod-auth shares.
macro_rules! registration_into {
    ($($module:ident),+) => {$(
        impl From<Registration> for $module::RegisterSubmitRequest<'_> {
            fn from(registration: Registration) -> Self {
                Self {
                    username: registration.username.into(),
                    email: registration.email.into(),
                    password: registration.password.into(),
                    modules: registration
                        .modules
                        .map(|modules| Cow::Owned(modules.into())),
                    ..Self::default()
                }
            }
        }
    )+};
}

registration_into!(openscreeps, official);

/// Whether a username or an e-mail address is free for a new account
/// (`GET /api/register/check-username`, `GET /api/register/check-email`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Availability {
    /// No account uses it.
    Available,
    /// An account uses it already.
    Taken,
}

/// The room to center the world map on (`GET /api/user/world-start-room`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartRoom {
    /// Shard of the room; `None` on single-world servers.
    pub shard: Option<String>,
    /// Room name, for example `W8N3`.
    pub room: String,
}

/// `From` the start-room reply every contract shares: its first room, `shard/room` when the
/// server names the shard (screeps.com asked without one), else a bare room without a shard
/// (screeps.com asked for a shard, which the caller knows, and single-world servers).
impl From<official::UserWorldStartRoomResponse> for Option<StartRoom> {
    fn from(reply: official::UserWorldStartRoomResponse) -> Self {
        reply
            .room
            .into_iter()
            .next()
            .map(|room| match room.split_once('/') {
                Some((shard, room)) => StartRoom {
                    shard: Some(shard.to_owned()),
                    room: room.to_owned(),
                },
                None => StartRoom { shard: None, room },
            })
    }
}

/// `From` screepsmod-admin-utils' start-room reply: its first room, single-world.
impl From<admin_utils::UserWorldStartRoomResponse> for Option<StartRoom> {
    fn from(reply: admin_utils::UserWorldStartRoomResponse) -> Self {
        reply
            .room
            .into_iter()
            .next()
            .map(|room| StartRoom { shard: None, room })
    }
}

/// The signed-in user's statistics overview (`GET /api/user/overview`): one statistic's series
/// per owned room, and the totals of every statistic.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overview {
    /// The rooms whose controller the user owns, highest level first within each shard, with
    /// their series.
    pub rooms: Vec<OverviewRoom>,
    /// The largest value of the series, when reported.
    pub stats_max: Option<f64>,
    /// The totals of each statistic over the interval, keyed by statistic name; empty where
    /// the server does not record statistics (community servers).
    pub totals: UserRoomStatValues,
}

/// One owned room of an [`Overview`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OverviewRoom {
    /// The room's shard; `None` on single-world servers.
    pub shard: Option<String>,
    /// The room name.
    pub room: String,
    /// The requested statistic over the interval, oldest slot first; empty when not recorded.
    pub series: Vec<UserOverviewPoint>,
}

impl OverviewRoom {
    /// The rooms of one shard (`None`: single-world) with their series from `stats`, appended
    /// to `into`; series of rooms `rooms` does not list follow them.
    fn collect_shard(
        into: &mut Vec<Self>,
        shard: Option<&str>,
        rooms: Vec<String>,
        mut stats: BTreeMap<String, Vec<UserOverviewPoint>>,
    ) {
        for room in rooms {
            let series = stats.remove(&room).unwrap_or_default();
            into.push(Self {
                shard: shard.map(str::to_owned),
                room,
                series,
            });
        }
        into.extend(stats.into_iter().map(|(room, series)| Self {
            shard: shard.map(str::to_owned),
            room,
            series,
        }));
    }
}

/// `From` the official overview, grouped by shard.
impl From<official::UserOverviewShardedResponse> for Overview {
    fn from(reply: official::UserOverviewShardedResponse) -> Self {
        let mut rooms = Vec::new();
        for (shard, overview) in reply.shards {
            OverviewRoom::collect_shard(
                &mut rooms,
                Some(&shard),
                overview.rooms.unwrap_or_default(),
                overview.stats.unwrap_or_default(),
            );
        }
        Self {
            rooms,
            stats_max: reply.stats_max,
            totals: reply.totals.unwrap_or_default(),
        }
    }
}

/// `From` the `OpenScreeps` overview, single-world.
impl From<openscreeps::UserOverviewFlatResponse> for Overview {
    fn from(reply: openscreeps::UserOverviewFlatResponse) -> Self {
        let mut rooms = Vec::new();
        OverviewRoom::collect_shard(&mut rooms, None, reply.rooms, reply.stats);
        Self {
            rooms,
            stats_max: reply.stats_max,
            totals: reply.totals,
        }
    }
}

/// `From` the community overview, single-world: only the rooms are implemented there (its
/// series and totals are empty); series and totals that do read as such are kept.
impl From<community::UserOverviewFlatResponse> for Overview {
    fn from(reply: community::UserOverviewFlatResponse) -> Self {
        let stats = reply
            .stats
            .into_iter()
            .filter_map(|(room, series)| Some((room, serde_json::from_value(series).ok()?)))
            .collect();
        let mut rooms = Vec::new();
        OverviewRoom::collect_shard(&mut rooms, None, reply.rooms, stats);
        Self {
            rooms,
            stats_max: None,
            totals: reply
                .totals
                .into_iter()
                .filter_map(|(name, total)| Some((name, total.as_f64()?)))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AuthToken, AuthTokenRequest, AuthTokenWebsockets, Overview, OverviewRoom, Profile,
        Registration, StartRoom, UserNotifyPrefs, UserOverviewPoint,
    };
    use crate::api::community::types as community;
    use crate::api::official::types as official;
    use crate::api::openscreeps::types as openscreeps;
    use crate::api::plugins::screepsmod_admin_utils::types as admin_utils;
    use crate::api::plugins::screepsmod_auth::types as screepsmod_auth;

    /// An official profile with a Steam account (`ownership` is a list of app ids there).
    const OFFICIAL: &str = r##"{"ok":1,"_id":"5a1b","username":"Alice","email":"alice@example.com",
        "password":true,"money":1234.5,"cpu":100,"cpuShard":{"shard0":20,"shard3":80},
        "cpuShardUpdatedTime":1700000000000,"gcl":3000000,"power":5000,
        "powerExperimentations":1,"badge":{"type":12,"color1":"#ff0000","color2":"#00ff00",
            "color3":"#0000ff","param":0,"flip":false},
        "notifyPrefs":{"disabled":false,"interval":60},
        "steam":{"id":"7656","displayName":"alice","ownership":[464350]}}"##;

    /// An `OpenScreeps` profile with a Steam account (`ownership` is a flag there).
    const OPENSCREEPS: &str = r#"{"ok":1,"_id":"u1","username":"Bob","money":0,
        "powerExperimentations":0,"password":false,"steam":{"id":"7656","ownership":true}}"#;

    #[test]
    fn profiles_convert_from_every_contract() {
        let me: official::AuthMeResponse = serde_json::from_str(OFFICIAL).unwrap();
        let profile = Profile::from(me);
        let badge = profile.badge.clone();
        assert!(badge.is_some());
        assert_eq!(
            profile,
            Profile {
                id: "5a1b".to_owned(),
                username: Some("Alice".to_owned()),
                email: Some("alice@example.com".to_owned()),
                email_dirty: None,
                has_password: Some(true),
                badge,
                money: 1234.5,
                cpu: Some(100.0),
                cpu_shards: [("shard0".to_owned(), 20.0), ("shard3".to_owned(), 80.0)]
                    .into_iter()
                    .collect(),
                cpu_shards_updated: Some(1_700_000_000_000.0),
                gcl: Some(3_000_000.0),
                power: Some(5000.0),
                notify_prefs: UserNotifyPrefs {
                    disabled: Some(false),
                    interval: Some(60),
                    ..UserNotifyPrefs::default()
                },
            }
        );

        // The community contract shares the official reply.
        let me: community::AuthMeResponse = serde_json::from_str(OFFICIAL).unwrap();
        assert_eq!(Profile::from(me), profile);

        // The official reply type cannot read `OpenScreeps`' Steam account.
        assert!(serde_json::from_str::<official::AuthMeResponse>(OPENSCREEPS).is_err());
        let me: openscreeps::AuthMeResponse = serde_json::from_str(OPENSCREEPS).unwrap();
        let profile = Profile::from(me);
        assert_eq!(profile.id, "u1");
        assert_eq!(profile.username.as_deref(), Some("Bob"));
        assert_eq!(profile.has_password, Some(false));
        assert_eq!(profile.email, None);
        assert_eq!(profile.badge, None);
        // No CPU allocation reads as an empty one.
        assert!(profile.cpu_shards.is_empty());
        assert_eq!(profile.cpu_shards_updated, None);
        // No saved preferences read as the defaults.
        assert_eq!(profile.notify_prefs, UserNotifyPrefs::default());
    }

    #[test]
    fn auth_tokens_convert_from_both_contracts() {
        let entry = r#"{"_id":"k1","token":"0123abcd","full":true,"noRatelimitUntil":17}"#;
        let official: official::UserAuthTokenListEntry = serde_json::from_str(entry).unwrap();
        let plugin: screepsmod_auth::UserAuthTokenListEntry = serde_json::from_str(entry).unwrap();
        let expected = AuthToken {
            id: "k1".to_owned(),
            description: None,
            full: true,
            no_ratelimit_until: Some(17),
            token: "0123abcd".to_owned(),
        };
        assert_eq!(AuthToken::from(official), expected);
        assert_eq!(AuthToken::from(plugin), expected);
        assert!(!format!("{expected:?}").contains("0123abcd"));
    }

    #[test]
    fn auth_token_requests_convert_into_both_contracts() {
        let request = AuthTokenRequest {
            kind: Some("full".to_owned()),
            websockets: Some(AuthTokenWebsockets {
                console: Some(true),
                rooms: None,
            }),
            ..AuthTokenRequest::default()
        };
        let official = official::UserAuthTokenCreateRequest::from(request.clone());
        let plugin = screepsmod_auth::UserAuthTokenCreateRequest::from(request);
        let expected = r#"{"type":"full","websockets":{"console":true}}"#;
        assert_eq!(serde_json::to_string(&official).unwrap(), expected);
        assert_eq!(serde_json::to_string(&plugin).unwrap(), expected);
    }

    #[test]
    fn start_rooms_read_the_shard_screeps_com_names() {
        let reply = |json: &str| -> Option<StartRoom> {
            serde_json::from_str::<official::UserWorldStartRoomResponse>(json)
                .unwrap()
                .into()
        };
        // screeps.com without a shard.
        assert_eq!(
            reply(r#"{"ok":1,"room":["shard3/W8N3"]}"#),
            Some(StartRoom {
                shard: Some("shard3".to_owned()),
                room: "W8N3".to_owned(),
            })
        );
        // screeps.com for a shard, and every single-world server.
        let bare = Some(StartRoom {
            shard: None,
            room: "W5N5".to_owned(),
        });
        assert_eq!(reply(r#"{"ok":1,"room":["W5N5"]}"#), bare);
        assert_eq!(reply(r#"{"ok":1,"room":[]}"#), None);
        let plugin: admin_utils::UserWorldStartRoomResponse =
            serde_json::from_str(r#"{"ok":1,"room":["W5N5"]}"#).unwrap();
        assert_eq!(Option::<StartRoom>::from(plugin), bare);
    }

    #[test]
    fn overviews_convert_from_the_sharded_and_the_flat_replies() {
        let point = |end_time, value| UserOverviewPoint { end_time, value };
        let official: official::UserOverviewShardedResponse = serde_json::from_str(
            r#"{"ok":1,"statsMax":9,"totals":{"energyHarvested":14},"shards":{
                "shard1":{"rooms":["W1N1"],"stats":{"W1N1":[{"endTime":1,"value":5}]},
                    "gameTimes":[100]},
                "shard3":{"rooms":["W2N2"],"stats":{"W2N2":[{"endTime":1,"value":9}]}}}}"#,
        )
        .unwrap();
        assert_eq!(
            Overview::from(official),
            Overview {
                rooms: vec![
                    OverviewRoom {
                        shard: Some("shard1".to_owned()),
                        room: "W1N1".to_owned(),
                        series: vec![point(1, 5.0)],
                    },
                    OverviewRoom {
                        shard: Some("shard3".to_owned()),
                        room: "W2N2".to_owned(),
                        series: vec![point(1, 9.0)],
                    },
                ],
                stats_max: Some(9.0),
                totals: [("energyHarvested".to_owned(), 14.0)].into(),
            }
        );

        let flat = r#"{"ok":1,"rooms":["W1N1"],"stats":{"W1N1":[{"value":5,"endTime":1}]},
            "statsMax":5,"totals":{"energyHarvested":5},"gametimes":[null,7]}"#;
        let expected = Overview {
            rooms: vec![OverviewRoom {
                shard: None,
                room: "W1N1".to_owned(),
                series: vec![point(1, 5.0)],
            }],
            stats_max: Some(5.0),
            totals: [("energyHarvested".to_owned(), 5.0)].into(),
        };
        let openscreeps: openscreeps::UserOverviewFlatResponse =
            serde_json::from_str(flat).unwrap();
        assert_eq!(Overview::from(openscreeps), expected);

        // The community server implements only the rooms.
        let community: community::UserOverviewFlatResponse = serde_json::from_str(
            r#"{"ok":1,"rooms":["W7N3","W8N3"],"stats":{},"totals":{},"gametimes":[]}"#,
        )
        .unwrap();
        let rooms: Vec<_> = Overview::from(community)
            .rooms
            .into_iter()
            .map(|room| (room.shard, room.room, room.series.len()))
            .collect();
        assert_eq!(
            rooms,
            [(None, "W7N3".to_owned(), 0), (None, "W8N3".to_owned(), 0)]
        );
    }

    #[test]
    fn registrations_convert_into_both_contracts() {
        let registration = Registration {
            username: "Alice".to_owned(),
            email: "alice@example.com".to_owned(),
            password: "secret-password".to_owned(),
            modules: None,
        };
        assert!(!format!("{registration:?}").contains("secret-password"));
        let expected =
            r#"{"email":"alice@example.com","password":"secret-password","username":"Alice"}"#;
        let native = openscreeps::RegisterSubmitRequest::from(registration.clone());
        let plugin = screepsmod_auth::RegisterSubmitRequest::from(registration);
        assert_eq!(serde_json::to_string(&native).unwrap(), expected);
        assert_eq!(serde_json::to_string(&plugin).unwrap(), expected);
    }
}
