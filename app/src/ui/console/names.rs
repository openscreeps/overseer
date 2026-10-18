//! The names that the console completes from the signed-in user's game ([`CompletionNames`]):
//! own creeps, spawns and rooms from the session's cache (`Console::names`), and all of
//! `Memory` from the [`Account`].
//!
//! Nothing loads until a completion needs it ([`CompletionNames::ensure`]). The library owns
//! what loads, when names are fresh, the pace of the requests and the cache that every console
//! of the session shares; this adapter only asks for the names of the account's shard and
//! shows the latest ones. `Memory` is all of `Memory` of the [`Account`]: the Memory tab reads
//! and changes the same value.

use std::sync::Arc;

use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::console::{COMPLETION_FRESHNESS, GameNames};
use overseer::memory::MemoryValue;
use serde_json::Value;

use super::complete::Needs;
use crate::ui::account::Account;

/// The names of the signed-in user's game, loaded on demand; `Copy`.
#[derive(Clone, Copy)]
pub struct CompletionNames {
    /// The session, its shard and its `Memory`.
    account: Account,
    /// The latest names from the session's cache, with the shard they were asked for.
    names: RwSignal<Option<(Option<String>, Arc<GameNames>)>>,
    /// A request for names runs.
    asking: StoredValue<bool>,
}

impl CompletionNames {
    /// No names yet, loaded from `account` on demand.
    pub fn new(account: Account) -> Self {
        Self {
            account,
            names: RwSignal::new(None),
            asking: StoredValue::new(false),
        }
    }

    /// The account that the names are of.
    pub const fn account(self) -> Account {
        self.account
    }

    /// Calls `f` with the names and all of `Memory` (`None` before it was read) on the
    /// account's shard. Tracks them.
    pub fn with<T>(self, f: impl FnOnce(&GameNames, Option<&Value>) -> T) -> T {
        let shard = self.account.shard.get();
        self.names.with(|held| {
            let none = GameNames::default();
            let names = held
                .as_ref()
                .filter(|(at, _)| *at == shard)
                .map_or(&none, |(_, names)| &**names);
            self.account.with_root(|root| {
                let memory = root.and_then(|root| match &root.value {
                    MemoryValue::Json(value) => Some(value),
                    MemoryValue::Undefined | MemoryValue::IncorrectPath => None,
                });
                f(names, memory)
            })
        })
    }

    /// Asks for what `needs` names on the account's shard; the library loads it unless it is
    /// fresh.
    pub fn ensure(self, needs: Needs) {
        if needs.objects {
            self.ask_objects();
        }
        if needs.memory {
            // A failure leaves the keys out; the next completion after the short freshness
            // asks again.
            self.account.refresh_root_stale(&COMPLETION_FRESHNESS);
        }
    }

    /// Shows the session's names of the account's shard at once if another console loaded
    /// them, and asks the library for fresh ones.
    fn ask_objects(self) {
        let shard = self.account.shard.get_untracked();
        let console = self.account.overseer().console();
        if let Some(names) = console.cached_names(shard.as_deref()) {
            self.show(shard.clone(), names);
        }
        if self.asking.get_value() {
            return;
        }
        self.asking.set_value(true);
        spawn_local(async move {
            let names = console.names(shard.as_deref()).await;
            if self.asking.try_set_value(false).is_some() {
                return;
            }
            // Without names (anonymous, closed, nothing loaded yet) the completion has none.
            if let Ok(Some(names)) = names {
                self.show(shard, names);
            }
        });
    }

    /// Shows `names` of `shard` unless they show already (the completion reads them again
    /// only when they change).
    fn show(self, shard: Option<String>, names: Arc<GameNames>) {
        let shown = self.names.with_untracked(|held| {
            held.as_ref()
                .is_some_and(|(at, held)| *at == shard && Arc::ptr_eq(held, &names))
        });
        if !shown {
            self.names.try_set(Some((shard, names)));
        }
    }
}
