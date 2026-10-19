//! The `poll` command keeps a first spawn of the account on the target shard.
//!
//! On a named shard, a placement needs fresh owned and reserved room lists.
//! The world status is account-wide, so a `normal` status does not stop a
//! placement on an empty target shard. The room lists do not prove that no
//! creeps exist. The server makes the last decision on a placement. On a
//! private server without shards, a placement needs the world status `empty`.
//!
//! A reset of a `lost` account needs all these conditions:
//!
//! - A fresh heartbeat on each shard counts no foothold.
//! - The heartbeats stay so for the time `CONFIRM`.
//! - A last revalidation of all shards passes.
//!
//! A spawn or an owned or reserved room on another shard always stops a
//! reset. The official world status is account-wide. Thus a reset needs a
//! heartbeat on every shard, and a shard without CPU cannot supply one.
//!
//! Without the execution gate (`--execute`), the client only reads. A failed
//! safety read never counts as empty or dead. A placement with an unknown
//! outcome stops all later placements in this process. `poll` does not
//! change CPU, code or branches.

mod clock;
#[cfg(test)]
mod mutation_tests;
mod safety;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use overseer::core::api::Refusal;
use overseer::map::RoomCoord;
use overseer::model::account::{Profile, UserWorldStatus};
use overseer::model::world::Terrain;
use overseer::{CodeTarget, MapStat, Overseer, net};

use crate::Error;
use crate::branch::{BranchName, GitError};
use crate::config::{Config, ProfileName, SecretError, Selected, ServerName, ShardChoice, Spawn};
use crate::connection;
use crate::envfile::EnvFile;
use crate::room::{self, Unfit};
use crate::selector::{self, Candidate, Choice};
use clock::{cooldown, now_ms, utc};
use safety::{footholds, recheck_lost, room_footholds, target_rooms, user_rooms};

/// The time that a `lost` account must stay without footholds before a reset.
const CONFIRM: Duration = Duration::from_secs(180);
/// The time that a plan without a placement stays valid. A plan has no
/// placement when the poll is read-only, when no candidate exists, when the
/// selector chooses no room, or when a check stops it. After this time, the
/// next poll reads the candidates again. The server permits one `map-stats`
/// call a minute.
const REPLAN: Duration = Duration::from_secs(600);
/// The name of the first spawn.
const SPAWN_NAME: &str = "Spawn1";

/// The arguments of `poll`.
#[derive(Debug, clap::Args)]
pub(crate) struct Poll {
    /// The profile to poll. Repeat the option, or separate the names with
    /// commas [default: every profile with a spawn table]
    #[arg(long = "profile", short, value_name = "NAME", value_delimiter = ',')]
    profiles: Vec<ProfileName>,
    /// Poll one time and stop. A reset needs polls over minutes, so --once
    /// never resets
    #[arg(long)]
    once: bool,
    /// Reset the account and place the spawn when all checks pass. Without
    /// this option, poll only reads and prints what it would do
    #[arg(long)]
    execute: bool,
}

/// The settings that all polls share.
struct Context<'a> {
    /// The `--env-file` variables.
    env: &'a EnvFile,
    /// The directory of the configuration. The selector runs in it.
    root: &'a Path,
    /// Whether the CLI may change the world (`--execute`).
    execute: bool,
    /// Whether this is the only poll.
    once: bool,
}

impl Poll {
    /// Polls the profiles of `config` one time or until the process stops.
    /// Uses the `--env-file` variables `env`. Prints each step to `out`.
    pub(crate) async fn run(
        self,
        config: &Config,
        env: &EnvFile,
        out: &mut dyn Write,
    ) -> Result<(), Error> {
        let targets = config.select_spawn(&self.profiles)?;
        let context = Context {
            env,
            root: config.root(),
            execute: self.execute,
            once: self.once,
        };
        writeln!(
            out,
            "poll {}: {}",
            if self.once { "once" } else { "until stopped" },
            if self.execute {
                "execute: resets and spawns when every check passes"
            } else {
                "read-only: nothing changes the world (--execute changes it)"
            }
        )?;
        let start = Instant::now();
        let mut reset_servers = BTreeSet::new();
        let mut watches: Vec<Watch<'_>> = targets
            .into_iter()
            .filter_map(|target| {
                let spawn = target.profile.spawn.as_ref()?;
                let mut watch = Watch::new(target, spawn, start);
                watch.reset_owner = reset_servers.insert(&target.profile.server);
                Some(watch)
            })
            .collect();
        let result = async {
            let mut failed = None;
            loop {
                let now = Instant::now();
                writeln!(out, "at {}", utc(SystemTime::now()))?;
                for watch in watches.iter_mut().filter(|watch| watch.due <= now) {
                    watch.due = now + watch.spawn.interval;
                    match watch.poll(&context, out).await {
                        Ok(()) => {}
                        Err(Failure::Output(error)) => return Err(Error::Output(error)),
                        Err(failure) if self.once && failed.is_none() => {
                            failed = Some(Error::Poll {
                                profile: watch.target.name.clone(),
                                source: failure,
                            });
                        }
                        Err(failure) => writeln!(out, "  error: {failure}")?,
                    }
                }
                if self.once {
                    return failed.map_or(Ok(()), Err);
                }
                let next = watches.iter().map(|watch| watch.due).min().unwrap_or(now);
                tokio::time::sleep(next.saturating_duration_since(Instant::now())).await;
            }
        }
        .await;
        for watch in &mut watches {
            if let Some(client) = watch.client.take() {
                client.shutdown().await;
            }
        }
        result
    }
}

/// The shards of a poll. `all` has every shard of the server, or one `None`
/// on a server without shards. `target` has the shard of the spawn, or the
/// reason that there is none.
struct Shards {
    all: Vec<Option<String>>,
    target: Result<Option<String>, String>,
}

/// The placement that the selector proposes. The CLI revalidates it
/// immediately before it places the spawn.
struct Placement<'a> {
    room: &'a str,
    x: u8,
    y: u8,
    shard: Option<&'a str>,
}

/// The confirmation of a `lost` account without footholds: the start time
/// and the lost shards.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Confirm {
    since: Instant,
    lost: Vec<Option<String>>,
}

impl Confirm {
    /// Returns how long `slot` has seen the shards `lost` without footholds.
    /// Records this observation at `now`. A different set of shards starts
    /// the confirmation again.
    fn observe(slot: &mut Option<Self>, lost: &[Option<String>], now: Instant) -> Duration {
        match slot {
            Some(confirm) if confirm.lost == lost => now.saturating_duration_since(confirm.since),
            _ => {
                *slot = Some(Self {
                    since: now,
                    lost: lost.to_vec(),
                });
                Duration::ZERO
            }
        }
    }
}

/// The state of one profile between polls.
struct Watch<'a> {
    target: Selected<'a>,
    spawn: &'a Spawn,
    /// The signed-in client. The watch keeps it between polls.
    client: Option<Overseer>,
    /// The confirmation of a lost account.
    confirm: Option<Confirm>,
    /// The time of the last reset from this process that the server can
    /// have applied.
    respawned: Option<SystemTime>,
    /// Whether this profile manages the reset of its account. Only the
    /// first selected profile of an account manages it.
    reset_owner: bool,
    /// When a poll last read the candidates and made a plan.
    planned: Option<Instant>,
    /// Whether a placement has an unknown outcome. The room lists cannot
    /// show if the placement occurred, so the CLI does not place again.
    placement_unknown: bool,
    /// When the next poll is due.
    due: Instant,
}

impl<'a> Watch<'a> {
    /// Makes the state of the profile `target` with the spawn table `spawn`.
    /// The first poll is due at `due`.
    fn new(target: Selected<'a>, spawn: &'a Spawn, due: Instant) -> Self {
        Self {
            target,
            spawn,
            client: None,
            confirm: None,
            respawned: None,
            reset_owner: true,
            planned: None,
            placement_unknown: false,
            due,
        }
    }

    /// Runs one poll. Signs in when the watch has no client. Drops the
    /// client when the server rejects the credentials or closes the
    /// connection. A failure starts the confirmation of a `lost` account
    /// again.
    async fn poll(&mut self, context: &Context<'_>, out: &mut dyn Write) -> Result<(), Failure> {
        writeln!(
            out,
            "profile {}: {}",
            self.target.name, self.target.server.url
        )?;
        let mut client = match self.client.take() {
            Some(client) => client,
            None => match self.sign_in(context).await {
                Ok(client) => client,
                Err(failure) => {
                    self.confirm = None;
                    return Err(failure);
                }
            },
        };
        let result = self.cycle(&mut client, context, out).await;
        let reconnect = match &result {
            Err(Failure::Api(overseer::Error::Closed)) => true,
            Err(Failure::Api(error)) => error.refusal() == Some(Refusal::Unauthorized),
            _ => false,
        };
        if reconnect {
            client.shutdown().await;
        } else {
            self.client = Some(client);
        }
        if result.is_err() {
            self.confirm = None;
        }
        result
    }

    /// Discovers the server and signs in through `connection::connect`. The
    /// execution gate in `reset` and `place` still guards all changes.
    async fn sign_in(&self, context: &Context<'_>) -> Result<Overseer, Failure> {
        let server = self.target.server;
        let credentials = server
            .auth
            .resolve(context.env)
            .map_err(|source| Failure::Secret {
                server: self.target.profile.server.clone(),
                source,
            })?;
        Ok(connection::connect(&server.url, credentials).await?)
    }

    /// Reads the world and acts on its status.
    async fn cycle(
        &mut self,
        client: &mut Overseer,
        context: &Context<'_>,
        out: &mut dyn Write,
    ) -> Result<(), Failure> {
        let me = client.account().refresh().await?;
        writeln!(
            out,
            "  account {}: cpu {}",
            me.username.as_deref().unwrap_or("(unnamed)"),
            me.cpu.unwrap_or(0.0)
        )?;
        let shards = self.shards(client, &me).await?;
        let shard = match &shards.target {
            Ok(shard) => shard.as_deref(),
            Err(blocker) => {
                self.confirm = None;
                writeln!(out, "  no spawn or reset: {blocker}")?;
                return Ok(());
            }
        };
        let status = client.account().world_status().await?;
        writeln!(out, "  world status: {} (account-wide)", status.as_str())?;
        let rooms = if let Some(shard) = shard {
            let rooms = user_rooms(client, &me).await?;
            if let Some(blocker) = target_rooms(&rooms, shard) {
                self.confirm = None;
                self.planned = None;
                writeln!(out, "  no spawn or reset: {blocker}")?;
                return Ok(());
            }
            Some(rooms)
        } else {
            None
        };
        if status == UserWorldStatus::Normal && shard.is_none() {
            self.confirm = None;
            self.planned = None;
            writeln!(out, "  the account has a spawn: nothing to do")?;
            return Ok(());
        }
        if status != UserWorldStatus::Lost {
            self.confirm = None;
            return self.place(client, &me, &shards, context, out).await;
        }
        self.planned = None;
        if let Some(rooms) = rooms
            && let Some(blocker) = room_footholds(&rooms, &shards.all)
        {
            self.confirm = None;
            writeln!(out, "  no reset: {blocker}")?;
            return Ok(());
        }
        let lost = shards.all.clone();
        self.reset(client, &shards, &lost, context, out).await
    }

    /// Returns the shards of the server and the shard of the spawn. The
    /// shard of the spawn is the configured shard. With `auto`, it is the
    /// only shard where the account has CPU.
    async fn shards(&self, client: &Overseer, me: &Profile) -> Result<Shards, Failure> {
        let Some(choice) = &self.spawn.shard else {
            return Ok(Shards {
                all: vec![None],
                target: Ok(None),
            });
        };
        let names: Vec<String> = client
            .shards_info()
            .await?
            .iter()
            .map(|info| info.name.clone())
            .collect();
        if names.is_empty() {
            return Err(Failure::Shards("the server lists no shard".to_owned()));
        }
        let target = match choice {
            ShardChoice::Named(name) => {
                if !names.iter().any(|shard| shard == name.as_str()) {
                    return Err(Failure::Shards(format!(
                        "no shard {name}; shards: {}",
                        names.join(", ")
                    )));
                }
                if me.assigned_cpu(Some(name.as_str())) > 0.0 {
                    Ok(Some(name.to_string()))
                } else {
                    Err(format!(
                        "no CPU on {name}; allocate CPU to it (poll does not)"
                    ))
                }
            }
            ShardChoice::Auto => {
                let with_cpu: Vec<&String> = names
                    .iter()
                    .filter(|shard| me.assigned_cpu(Some(shard.as_str())) > 0.0)
                    .collect();
                match with_cpu.as_slice() {
                    [only] => Ok(Some((*only).clone())),
                    [] => Err("no shard has CPU; allocate CPU to one (poll does not)".to_owned()),
                    several => {
                        let several: Vec<String> =
                            several.iter().map(ToString::to_string).collect();
                        return Err(Failure::Shards(format!(
                            "shards {} have CPU: choose one with shard = \"NAME\"",
                            several.join(", ")
                        )));
                    }
                }
            }
        };
        Ok(Shards {
            all: names.into_iter().map(Some).collect(),
            target,
        })
    }

    /// Returns the branch of the profile if it runs in the world. Otherwise,
    /// returns the reason.
    async fn branch(&self, client: &Overseer) -> Result<Result<BranchName, String>, Failure> {
        let branch = self.target.profile.branch.resolve(&mut None)?;
        let runs = client
            .code()
            .branches()
            .await?
            .iter()
            .any(|info| info.branch == branch.as_str() && CodeTarget::World.is_active(info));
        Ok(if runs {
            Ok(branch)
        } else {
            Err(format!(
                "branch {branch} does not run in the world; upload it with --activate world first (poll does not)"
            ))
        })
    }

    /// Resets the account that is `lost` on the shards `lost`. First,
    /// confirms over time that the bot has no foothold left. Then,
    /// revalidates immediately before the reset.
    async fn reset(
        &mut self,
        client: &mut Overseer,
        shards: &Shards,
        lost: &[Option<String>],
        context: &Context<'_>,
        out: &mut dyn Write,
    ) -> Result<(), Failure> {
        if !self.reset_owner {
            self.confirm = None;
            writeln!(
                out,
                "  no reset: another profile manages this account's reset"
            )?;
            return Ok(());
        }
        if let Some(blocker) = footholds(client, lost).await? {
            self.confirm = None;
            writeln!(out, "  no reset: {blocker}")?;
            return Ok(());
        }
        let held = Confirm::observe(&mut self.confirm, lost, Instant::now());
        let lost_text: Vec<&str> = lost.iter().map(|shard| label(shard.as_deref())).collect();
        if held < CONFIRM {
            writeln!(
                out,
                "  lost without footholds on {}: confirming, {} of {} s",
                lost_text.join(", "),
                held.as_secs(),
                CONFIRM.as_secs()
            )?;
            if context.once {
                writeln!(out, "  --once never resets: only `poll` confirms over time")?;
            }
            return Ok(());
        }
        writeln!(out, "  lost without footholds for {} s", held.as_secs())?;
        let branch = match self.branch(client).await? {
            Ok(branch) => branch,
            Err(blocker) => {
                writeln!(out, "  no reset: {blocker}")?;
                return Ok(());
            }
        };
        if !context.execute {
            writeln!(
                out,
                "  would reset the account; branch {branch} runs in the world (--execute resets)"
            )?;
            return Ok(());
        }
        if let Some(blocker) = recheck_lost(client, &shards.all, lost).await? {
            self.confirm = None;
            writeln!(out, "  no reset: {blocker}")?;
            return Ok(());
        }
        self.confirm = None;
        writeln!(out, "  resetting the account (user/respawn)")?;
        let result = client.account().respawn().await;
        self.finish_reset(result, SystemTime::now(), out)
    }

    /// Records a reset that the server can have applied, then returns the
    /// error. Keeps the earlier cooldown when the reset was not applied.
    fn finish_reset(
        &mut self,
        result: Result<(), overseer::Error>,
        now: SystemTime,
        out: &mut dyn Write,
    ) -> Result<(), Failure> {
        match result {
            Ok(()) => {
                self.respawned = Some(now);
                writeln!(
                    out,
                    "  reset: the spawn follows after the cooldown of the server"
                )?;
                Ok(())
            }
            Err(error) => {
                if error.outcome() == net::Outcome::Unknown {
                    self.respawned = Some(now);
                }
                Err(error.into())
            }
        }
    }

    /// Places the first spawn. The target shard has no owned or reserved
    /// rooms, or the world without shards is empty. Chooses a room,
    /// revalidates it, then tries the placement.
    async fn place(
        &mut self,
        client: &mut Overseer,
        me: &Profile,
        shards: &Shards,
        context: &Context<'_>,
        out: &mut dyn Write,
    ) -> Result<(), Failure> {
        if self.placement_unknown {
            writeln!(
                out,
                "  no spawn: previous placement outcome is unknown; inspect the world before restarting poll"
            )?;
            return Ok(());
        }
        let shard = match &shards.target {
            Ok(shard) => shard.clone(),
            Err(blocker) => {
                writeln!(out, "  no spawn: {blocker}")?;
                return Ok(());
            }
        };
        let shard = shard.as_deref();
        if me.assigned_cpu(shard) <= 0.0 {
            writeln!(
                out,
                "  no spawn: the account has no CPU (poll does not allocate it)"
            )?;
            return Ok(());
        }
        if let Some(left) = cooldown(me.last_respawn, self.respawned, SystemTime::now()) {
            writeln!(out, "  respawn cooldown: {} s left", left.as_secs())?;
            return Ok(());
        }
        if let Some(planned) = self.planned
            && planned.elapsed() < REPLAN
        {
            writeln!(
                out,
                "  planned {} s ago; the next plan in {} s",
                planned.elapsed().as_secs(),
                REPLAN.saturating_sub(planned.elapsed()).as_secs()
            )?;
            return Ok(());
        }
        let branch = self.branch(client).await?;
        let candidates = self.discover(client, shard, out).await?;
        self.planned = Some(Instant::now());
        if candidates.is_empty() {
            writeln!(out, "  no candidate room: nothing to place")?;
            return Ok(());
        }
        let selection = selector::choose(&self.spawn.selector, context.root, shard, &candidates)?;
        for line in selection
            .stderr
            .lines()
            .filter(|line| !line.trim().is_empty())
        {
            writeln!(out, "  selector: {line}")?;
        }
        let (room, x, y) = match selection.choice {
            Choice::Nothing { reason } => {
                writeln!(out, "  the selector chose no room: {reason}")?;
                return Ok(());
            }
            Choice::Room { room, x, y, reason } => {
                writeln!(out, "  the selector chose {room} at ({x}, {y}): {reason}")?;
                (room, x, y)
            }
        };
        let Some(candidate) = candidates.iter().find(|candidate| candidate.name == room) else {
            return Err(Failure::Unfit {
                room,
                x,
                y,
                reason: Unfit::Malformed("candidate"),
            });
        };
        if let Err(reason) = room::check_tile(&candidate.terrain, &candidate.objects, x, y) {
            return Err(Failure::Unfit { room, x, y, reason });
        }
        let branch = match branch {
            Ok(branch) => branch,
            Err(blocker) => {
                writeln!(out, "  no spawn: {blocker}")?;
                return Ok(());
            }
        };
        if !context.execute {
            writeln!(
                out,
                "  would place {SPAWN_NAME} in {room} at ({x}, {y}); branch {branch} runs in the world (--execute places it)"
            )?;
            return Ok(());
        }
        let spawn = Placement {
            room: &room,
            x,
            y,
            shard,
        };
        if let Some(blocker) = self
            .recheck_place(client, &me.id, &spawn, &candidate.terrain)
            .await?
        {
            writeln!(out, "  no spawn: {blocker}")?;
            return Ok(());
        }
        writeln!(
            out,
            "  placing {SPAWN_NAME} in {room} at ({x}, {y}) (game/place-spawn)"
        )?;
        let result = client
            .rooms()
            .place_spawn(&room, x, y, Some(SPAWN_NAME), shard)
            .await;
        self.finish_placement(result, out)
    }

    /// Records the result of a placement. A success clears the plan. An
    /// unknown outcome blocks all later placements of the profile, also
    /// after a new sign-in.
    fn finish_placement(
        &mut self,
        result: Result<(), overseer::Error>,
        out: &mut dyn Write,
    ) -> Result<(), Failure> {
        match result {
            Ok(()) => {
                self.planned = None;
                writeln!(out, "  placed")?;
                Ok(())
            }
            Err(error) => {
                if error.outcome() == net::Outcome::Unknown {
                    self.placement_unknown = true;
                }
                Err(error.into())
            }
        }
    }

    /// Reads the candidates of `shard`, at most `candidates` of them. A
    /// candidate is a room around the start room of the server, nearest
    /// first. It can have a controller, and it is not prohibited. On the
    /// map, its status is `normal` and it has no owner. Its objects and its
    /// status let a spawn in.
    async fn discover(
        &self,
        client: &Overseer,
        shard: Option<&str>,
        out: &mut dyn Write,
    ) -> Result<Vec<Candidate>, Failure> {
        let first = client.account().start_room(shard).await?;
        let start = first
            .as_ref()
            .filter(|start| room::of_shard(start.shard.as_deref(), shard))
            .and_then(|start| start.room.parse::<RoomCoord>().ok());
        let Some(start) = start else {
            let name = first.map_or_else(String::new, |start| match start.shard {
                Some(prefix) => format!("{prefix}/{}", start.room),
                None => start.room,
            });
            return Err(Failure::StartRoom(name));
        };
        let prohibited = prohibited(client, shard).await?;
        let names: Vec<String> = room::around(start, self.spawn.radius)
            .into_iter()
            .filter(|room| room.may_have_controller())
            .map(|room| room.to_string())
            .filter(|name| !prohibited.contains(name))
            .collect();
        let room_map = if names.is_empty() {
            BTreeMap::new()
        } else {
            client
                .map()
                .stats(names.iter().map(String::as_str), MapStat::Owner, shard)
                .await?
                .stats
        };
        let now = now_ms();
        let mut candidates = Vec::new();
        let mut read = 0;
        for name in &names {
            if candidates.len() >= self.spawn.candidates {
                break;
            }
            let Some(entry) = room_map.get(name) else {
                continue;
            };
            if room::check_map_status(entry, now).is_err() {
                continue;
            }
            read += 1;
            let objects = client.rooms().objects(name, shard).await?.objects;
            if room::check_objects(&objects).is_err() {
                continue;
            }
            let status = client.rooms().status(name, shard).await?;
            if room::check_status(&status, now).is_err() {
                continue;
            }
            let terrain = client.rooms().terrain(name, shard).await?;
            candidates.push(Candidate {
                name: name.clone(),
                terrain,
                objects,
                status,
            });
        }
        writeln!(
            out,
            "  candidates: {} of {read} rooms read, of {} within {} of start room {start} on {}",
            candidates.len(),
            names.len(),
            self.spawn.radius,
            label(shard)
        )?;
        Ok(candidates)
    }

    /// Revalidates `spawn` with fresh data. Returns the reason that the CLI
    /// may not place it, or `None`. The checks are:
    ///
    /// - the CPU of the account and the cooldown,
    /// - the account identity and the branch,
    /// - the room lists of the target shard, or the world status without
    ///   shards,
    /// - the prohibited rooms, the room status and the room objects,
    /// - the selected tile.
    async fn recheck_place(
        &self,
        client: &Overseer,
        identity: &str,
        spawn: &Placement<'_>,
        terrain: &Terrain,
    ) -> Result<Option<String>, Failure> {
        let Placement { room, x, y, shard } = *spawn;
        let me = client.account().refresh().await?;
        if me.assigned_cpu(shard) <= 0.0 {
            return Ok(Some("the account has no CPU".to_owned()));
        }
        if let Some(left) = cooldown(me.last_respawn, self.respawned, SystemTime::now()) {
            return Ok(Some(format!("respawn cooldown: {} s left", left.as_secs())));
        }
        if shard.is_some() && me.id != identity {
            return Ok(Some("the account identity changed".to_owned()));
        }
        if shard.is_none() {
            let status = client.account().world_status().await?;
            if status != UserWorldStatus::Empty {
                return Ok(Some(format!("the world status is now {}", status.as_str())));
            }
        }
        if let Err(blocker) = self.branch(client).await? {
            return Ok(Some(blocker));
        }
        if prohibited(client, shard).await?.contains(room) {
            return Ok(Some(format!("{room}: {}", Unfit::Prohibited)));
        }
        let status = client.rooms().status(room, shard).await?;
        let objects = client.rooms().objects(room, shard).await?.objects;
        let checked = room::check_status(&status, now_ms())
            .and_then(|()| room::check_objects(&objects))
            .and_then(|()| room::check_tile(terrain, &objects, x, y));
        if let Err(reason) = checked {
            return Ok(Some(format!("{room} ({x}, {y}): {reason}")));
        }
        if let Some(shard) = shard {
            return Ok(target_rooms(&user_rooms(client, &me).await?, shard));
        }
        Ok(None)
    }
}

/// The rooms of `shard` where the server prohibits a respawn. The server
/// gives one list for the account.
async fn prohibited(client: &Overseer, shard: Option<&str>) -> Result<BTreeSet<String>, Failure> {
    Ok(client
        .account()
        .respawn_prohibited_rooms()
        .await?
        .iter()
        .filter_map(|name| room::on_shard(name, shard))
        .map(str::to_owned)
        .collect())
}

/// The name of `shard` for output.
fn label(shard: Option<&str>) -> &str {
    shard.unwrap_or("the world")
}

/// A failed poll of a profile.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Failure {
    /// A secret of the server is not available.
    #[error("server {server}: {source}")]
    Secret {
        /// The server.
        server: ServerName,
        /// Why the secret is not available.
        source: SecretError,
    },
    /// An API call failed.
    #[error(transparent)]
    Api(#[from] overseer::Error),
    /// `auto` names no branch.
    #[error("branch auto: {0}")]
    Git(#[from] GitError),
    /// The selector failed.
    #[error("selector: {0}")]
    Selector(#[from] selector::Error),
    /// The shards of the server do not give one shard for the spawn.
    #[error("shards: {0}")]
    Shards(String),
    /// The account has no identity for the safety checks on a named shard.
    #[error("account identity: auth/me has no nonempty _id; refusing named-shard mutation")]
    Identity,
    /// The start room of the server is not a room of the shard.
    #[error("start room {0:?}: not a room of the shard")]
    StartRoom(String),
    /// The selector chose a tile that does not take a spawn.
    #[error("the selector chose {room} at ({x}, {y}), which does not take a spawn: {reason}")]
    Unfit {
        /// The room.
        room: String,
        /// The column.
        x: u8,
        /// The row.
        y: u8,
        /// Why the tile does not take a spawn.
        reason: Unfit,
    },
    /// The output is not writable.
    #[error("the output: {0}")]
    Output(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The confirmation counts while the same shards stay lost. It starts
    /// again for other shards.
    #[test]
    fn confirmation() {
        let start = Instant::now();
        let shard3 = vec![Some("shard3".to_owned())];
        let mut slot = None;
        assert_eq!(Confirm::observe(&mut slot, &shard3, start), Duration::ZERO);
        assert_eq!(
            Confirm::observe(&mut slot, &shard3, start + Duration::from_secs(120)),
            Duration::from_secs(120)
        );
        let both = vec![Some("shard2".to_owned()), Some("shard3".to_owned())];
        assert_eq!(
            Confirm::observe(&mut slot, &both, start + Duration::from_secs(200)),
            Duration::ZERO
        );
        assert_eq!(
            Confirm::observe(&mut slot, &both, start + Duration::from_secs(390)),
            Duration::from_secs(190)
        );
    }
}
