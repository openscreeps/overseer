//! The Objects tab: the live room's objects grouped by kind (groups with the signed-in user's
//! objects first), filterable, with count badges, and the [`Inspector`] of the selection.
//!
//! Selection is shared with the map both ways through [`Selection`]: picking an object in the
//! list selects its tile on the map (and centres the map's camera on it); a tile picked on a
//! map selects its topmost object ([`topmost`]) in the list. A selected creep is followed as
//! it moves.

use std::collections::HashMap;
use std::sync::Arc;

use leptos::prelude::*;
use overseer::RoomSnapshot;
use overseer::core::api::types::GameRoomObject;
use overseer::map::RoomCoord;

use crate::ui::icon::{Icon, IconKind};
use crate::ui::inspector::{Context, Inspector, kind_key, kind_label, title};

/// Kinds in list order; others follow alphabetically.
const KIND_ORDER: [&str; 31] = [
    "creep",
    "powerCreep",
    "spawn",
    "extension",
    "tower",
    "storage",
    "terminal",
    "link",
    "lab",
    "factory",
    "powerSpawn",
    "nuker",
    "observer",
    "extractor",
    "container",
    "controller",
    "source",
    "mineral",
    "deposit",
    "rampart",
    "constructedWall",
    "road",
    "constructionSite",
    "resource",
    "tombstone",
    "ruin",
    "invaderCore",
    "keeperLair",
    "powerBank",
    "portal",
    "nuke",
];

/// One object in the list.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Entry {
    /// The object id.
    pub id: String,
    /// Its name ([`title`]).
    pub title: String,
    /// Its owner's name.
    pub owner: Option<String>,
    /// Owned by the signed-in user.
    pub own: bool,
    /// Its tile.
    pub x: u8,
    /// Its tile.
    pub y: u8,
}

/// The objects of one kind.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Group {
    /// The kind ([`kind_key`]).
    pub key: String,
    /// What they are, plural.
    pub label: String,
    /// How many are the signed-in user's.
    pub own: usize,
    /// The objects: the user's first, then by owner, name and position.
    pub entries: Vec<Entry>,
}

/// Where `key` goes in the list.
fn rank(key: &str) -> usize {
    KIND_ORDER
        .iter()
        .position(|known| *known == key)
        .unwrap_or(KIND_ORDER.len())
}

/// `label` in the plural.
fn plural(label: &str) -> String {
    if let Some(stem) = label.strip_suffix('y')
        && !label.ends_with("ey")
    {
        return format!("{stem}ies");
    }
    if label.ends_with('s') {
        return format!("{label}es");
    }
    format!("{label}s")
}

/// Whether `entry` (of kind `label`) matches the lower-case `filter`: its name, kind, owner,
/// id or `x,y`.
fn matches(entry: &Entry, label: &str, filter: &str) -> bool {
    filter.is_empty()
        || entry.title.to_lowercase().contains(filter)
        || label.to_lowercase().contains(filter)
        || entry
            .owner
            .as_ref()
            .is_some_and(|owner| owner.to_lowercase().contains(filter))
        || entry.id.to_lowercase().contains(filter)
        || format!("{},{}", entry.x, entry.y) == filter
}

/// The tile coordinate of a room object (0 to 49).
fn coordinate(value: u64) -> u8 {
    u8::try_from(value.min(49)).unwrap_or(49)
}

/// `objects` matching `filter` (any case), grouped by kind: groups holding the user's objects
/// first, then in [`KIND_ORDER`]; empty groups are left out.
pub fn groups(objects: &[GameRoomObject], context: &Context<'_>, filter: &str) -> Vec<Group> {
    let filter = filter.trim().to_lowercase();
    let mut groups: Vec<Group> = Vec::new();
    for object in objects {
        let key = kind_key(object);
        let label = kind_label(key);
        let user = object.user();
        let entry = Entry {
            id: object.id().to_owned(),
            title: title(object),
            owner: user.map(|user| context.name(user)),
            own: context.is_me(user),
            x: coordinate(object.x()),
            y: coordinate(object.y()),
        };
        if !matches(&entry, &label, &filter) {
            continue;
        }
        let index = if let Some(index) = groups.iter().position(|group| group.key == key) {
            index
        } else {
            groups.push(Group {
                key: key.to_owned(),
                label: plural(&label),
                own: 0,
                entries: Vec::new(),
            });
            groups.len() - 1
        };
        let group = &mut groups[index];
        group.own += usize::from(entry.own);
        group.entries.push(entry);
    }
    for group in &mut groups {
        group.entries.sort_by(|a, b| {
            b.own
                .cmp(&a.own)
                .then_with(|| a.owner.cmp(&b.owner))
                .then_with(|| natural(&a.title, &b.title))
                .then_with(|| (a.y, a.x).cmp(&(b.y, b.x)))
                .then_with(|| a.id.cmp(&b.id))
        });
    }
    groups.sort_by(|a, b| {
        (b.own > 0)
            .cmp(&(a.own > 0))
            .then_with(|| rank(&a.key).cmp(&rank(&b.key)))
            .then_with(|| a.key.cmp(&b.key))
    });
    groups
}

/// Compares names with their digit runs as numbers (`c9` before `c10`).
fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    let key = |text: &str| -> Vec<(String, u64)> {
        let mut parts = Vec::new();
        let mut word = String::new();
        let mut digits = String::new();
        for character in text.chars() {
            if character.is_ascii_digit() {
                digits.push(character);
            } else {
                if !digits.is_empty() {
                    parts.push((
                        std::mem::take(&mut word),
                        digits.parse().unwrap_or(u64::MAX),
                    ));
                    digits.clear();
                }
                word.push(character);
            }
        }
        parts.push((word, digits.parse().unwrap_or(0)));
        parts
    };
    key(a).cmp(&key(b))
}

/// How high an object sits on its tile: creeps over structures over sites, containers,
/// ramparts, dropped resources and ruins, roads at the bottom.
fn layer(object: &GameRoomObject) -> u8 {
    match object {
        GameRoomObject::Creep(_) | GameRoomObject::PowerCreep(_) => 0,
        GameRoomObject::Resource(_)
        | GameRoomObject::Tombstone(_)
        | GameRoomObject::Ruin(_)
        | GameRoomObject::Nuke(_) => 3,
        GameRoomObject::ConstructionSite(_) => 4,
        GameRoomObject::Container(_) => 5,
        GameRoomObject::Rampart(_) => 6,
        GameRoomObject::Road(_) => 7,
        GameRoomObject::Other(_) => 8,
        _ => 1,
    }
}

/// The object a click on tile `(x, y)` selects: the topmost there.
pub fn topmost(objects: &[GameRoomObject], x: u8, y: u8) -> Option<&GameRoomObject> {
    objects
        .iter()
        .filter(|object| object.x() == u64::from(x) && object.y() == u64::from(y))
        .min_by_key(|object| layer(object))
}

/// The selection of a room control, shared with its map(s).
#[derive(Clone, Copy)]
pub struct Selection {
    /// The selected object id.
    pub id: RwSignal<Option<String>>,
    /// The selected tile (the map's highlight).
    pub tile: RwSignal<Option<(RoomCoord, u8, u8)>>,
    /// The tile this selection last wrote: a different one came from a map.
    written: StoredValue<Option<(RoomCoord, u8, u8)>>,
}

impl Selection {
    /// A selection on `tile`, following the room's objects in `snapshot`; `picked` runs when a
    /// map picks a tile.
    pub fn new(
        tile: RwSignal<Option<(RoomCoord, u8, u8)>>,
        room: Signal<Option<RoomCoord>>,
        snapshot: Signal<Option<Arc<RoomSnapshot>>>,
        picked: impl Fn() + 'static,
    ) -> Self {
        let selection = Self {
            id: RwSignal::new(None),
            tile,
            written: StoredValue::new(tile.get_untracked()),
        };
        // Another room: nothing of the last is selected.
        Effect::new(move |previous: Option<Option<RoomCoord>>| {
            let room = room.get();
            if previous.is_some_and(|previous| previous != room) {
                selection.id.set(None);
            }
            room
        });
        // A tile picked on a map selects its topmost object, once the room's objects are
        // known (a pick while the room is not live waits for them).
        let pending = StoredValue::new(false);
        Effect::new(move |_| {
            let tile = tile.get();
            let live = snapshot.with(Option::is_some);
            if selection.written.get_value() == tile && !pending.get_value() {
                return;
            }
            selection.written.set_value(tile);
            let Some((_, x, y)) = tile.filter(|(at, ..)| Some(*at) == room.get_untracked()) else {
                pending.set_value(false);
                selection.id.set(None);
                return;
            };
            if !live {
                pending.set_value(true);
                return;
            }
            pending.set_value(false);
            let id = snapshot.with_untracked(|snapshot| {
                topmost(&snapshot.as_ref()?.objects, x, y).map(|object| object.id().to_owned())
            });
            let found = id.is_some();
            selection.id.set(id);
            if found {
                picked();
            }
        });
        // The selection moves (a creep walks): the tile follows, unless a map picked another
        // tile that the effect above has not resolved yet (effects run in any order: the
        // previous object's place must not overwrite the pick).
        Effect::new(move |_| {
            let Some(id) = selection.id.get() else {
                return;
            };
            let Some(at) = room.get_untracked() else {
                return;
            };
            let place = snapshot.with(|snapshot| {
                let object = snapshot
                    .as_ref()?
                    .objects
                    .iter()
                    .find(|object| object.id() == id)?;
                Some((at, coordinate(object.x()), coordinate(object.y())))
            });
            let picking =
                pending.get_value() || tile.get_untracked() != selection.written.get_value();
            if let Some(place) = place
                && !picking
                && tile.get_untracked() != Some(place)
            {
                selection.write(Some(place));
            }
        });
        selection
    }

    /// Sets the tile without it counting as a pick.
    fn write(self, tile: Option<(RoomCoord, u8, u8)>) {
        self.written.set_value(tile);
        self.tile.set(tile);
    }

    /// Selects the object `id` on `(x, y)` of `room`.
    pub fn select(self, room: RoomCoord, id: String, x: u8, y: u8) {
        self.id.set(Some(id));
        self.write(Some((room, x, y)));
    }
}

/// The room's objects and the inspector of the selection.
#[component]
pub fn ObjectsPanel(
    /// The room shown.
    #[prop(into)]
    room: Signal<Option<RoomCoord>>,
    /// The room's live state.
    #[prop(into)]
    snapshot: Signal<Option<Arc<RoomSnapshot>>>,
    /// The selection.
    selection: Selection,
    /// The signed-in user id.
    user: Option<String>,
    /// Centres the map(s) on a tile picked in the list.
    #[prop(optional)]
    centre: Option<Callback<(RoomCoord, u8, u8)>>,
) -> impl IntoView {
    let filter = RwSignal::new(String::new());
    let me = StoredValue::new(user.clone());
    let pick = move |id: String, x: u8, y: u8| {
        let Some(room) = room.get_untracked() else {
            return;
        };
        selection.select(room, id, x, y);
        if let Some(centre) = centre {
            centre.run((room, x, y));
        }
    };
    let on_select = Callback::new(move |id: String| {
        let place = snapshot.with_untracked(|snapshot| {
            let object = snapshot
                .as_ref()?
                .objects
                .iter()
                .find(|object| object.id() == id)?;
            Some((coordinate(object.x()), coordinate(object.y())))
        });
        if let Some((x, y)) = place {
            pick(id, x, y);
        }
    });
    let listed = Memo::new(move |_| {
        let query = filter.get();
        snapshot.with(|snapshot| {
            snapshot.as_ref().map(|snapshot| {
                let user = me.get_value();
                let context = Context {
                    game_time: snapshot.game_time,
                    me: user.as_deref(),
                    users: &snapshot.users,
                };
                groups(&snapshot.objects, &context, &query)
            })
        })
    });
    let collapsed = RwSignal::new(Vec::<String>::new());
    let total =
        move || snapshot.with(|snapshot| snapshot.as_ref().map(|snapshot| snapshot.objects.len()));
    let empty = move || {
        let message = listed.with(|listed| match listed {
            None => Some(("The room is not live: its objects are unknown.", None)),
            Some(groups) if groups.is_empty() => {
                Some(("No objects match the filter.", Some("status")))
            }
            Some(_) => None,
        });
        message.map(|(text, role)| view! { <p class="empty-state" role=role>{text}</p> })
    };
    let on_pick = Callback::new(move |(id, x, y): (String, u8, u8)| pick(id, x, y));
    view! {
        <div class="objects-panel">
            <div class="objects-bar">
                <label class="filter">
                    <Icon kind=IconKind::Filter />
                    <input
                        type="search"
                        placeholder="Filter objects"
                        aria-label="Filter objects"
                        prop:value=move || filter.get()
                        on:input=move |event| filter.set(event_target_value(&event))
                    />
                </label>
                <span class="count" title="Objects in the room">
                    {move || total().map(|total| total.to_string()).unwrap_or_default()}
                </span>
            </div>
            <nav class="objects-list" aria-label="Objects">
                {empty}
                // Groups keyed by kind and rows by object id, updated in place: a walking creep
                // must not rebuild its row every tick (a click or the focus on it would be lost).
                <For
                    each=move || {
                        listed.with(|listed| {
                            let groups = listed.iter().flatten();
                            groups.map(|group| group.key.clone()).collect::<Vec<_>>()
                        })
                    }
                    key=Clone::clone
                    let(key)
                >
                    <GroupView kind=key groups=listed selected=selection.id collapsed on_pick />
                </For>
            </nav>
            <Inspector snapshot selected=selection.id user on_select />
        </div>
    }
}

/// One kind's objects, collapsible (`collapsed` holds the collapsed kinds).
#[component]
fn GroupView(
    /// The kind ([`Group::key`]).
    kind: String,
    /// Every group listed.
    groups: Memo<Option<Vec<Group>>>,
    selected: RwSignal<Option<String>>,
    collapsed: RwSignal<Vec<String>>,
    on_pick: Callback<(String, u8, u8)>,
) -> impl IntoView {
    let list_id = format!("objects-{kind}");
    let key = StoredValue::new(kind);
    let group = Memo::new(move |_| {
        key.with_value(|key| {
            groups.with(|groups| {
                groups
                    .iter()
                    .flatten()
                    .find(|group| group.key == *key)
                    .cloned()
            })
        })
    });
    let entries = Memo::new(move |_| {
        group.with(|group| {
            group
                .iter()
                .flat_map(|group| &group.entries)
                .map(|entry| (entry.id.clone(), entry.clone()))
                .collect::<HashMap<_, _>>()
        })
    });
    let label = group
        .with_untracked(|group| group.as_ref().map(|group| group.label.clone()))
        .unwrap_or_default();
    let count = move || group.with(|group| group.as_ref().map_or(0, |group| group.entries.len()));
    let open = move || key.with_value(|key| !collapsed.with(|collapsed| collapsed.contains(key)));
    let toggle = move |_| {
        let key = key.get_value();
        collapsed.update(|collapsed| {
            if let Some(index) = collapsed.iter().position(|known| *known == key) {
                collapsed.remove(index);
            } else {
                collapsed.push(key);
            }
        });
    };
    let ids = move || {
        group.with(|group| {
            group
                .iter()
                .flat_map(|group| &group.entries)
                .map(|entry| entry.id.clone())
                .collect::<Vec<_>>()
        })
    };
    view! {
        <section class="object-group">
            <h3>
                <button
                    type="button"
                    class="group-head"
                    aria-expanded=move || open().to_string()
                    aria-controls=list_id.clone()
                    on:click=toggle
                >
                    <span class="chevron" aria-hidden="true"><Icon kind=IconKind::ChevronDown /></span>
                    <span class="group-label">{label}</span>
                    <span class="count">{count}</span>
                </button>
            </h3>
            <ul id=list_id hidden=move || !open()>
                <For each=ids key=Clone::clone let(id)>
                    <ObjectRow id entries selected on_pick />
                </For>
            </ul>
        </section>
    }
}

/// One object's row, updated in place as the object changes (a creep walks).
#[component]
fn ObjectRow(
    /// The object id.
    id: String,
    /// The group's objects by id.
    entries: Memo<HashMap<String, Entry>>,
    selected: RwSignal<Option<String>>,
    on_pick: Callback<(String, u8, u8)>,
) -> impl IntoView {
    let id = StoredValue::new(id);
    let entry =
        Memo::new(move |_| id.with_value(|id| entries.with(|entries| entries.get(id).cloned())));
    let text = move |field: fn(&Entry) -> String| {
        move || entry.with(|entry| entry.as_ref().map(field).unwrap_or_default())
    };
    let current = move || {
        id.with_value(|id| selected.with(|selected| selected.as_deref() == Some(id.as_str())))
    };
    let own = move || entry.with(|entry| entry.as_ref().is_some_and(|entry| entry.own));
    // The user's own objects are marked by colour; others name their owner.
    let owner = move || {
        let owner = entry.with(|entry| {
            let other = entry.as_ref().filter(|entry| !entry.own);
            other.and_then(|entry| entry.owner.clone())
        });
        owner.map(|owner| view! { <span class="owner">{owner}</span> })
    };
    // Where the object is now, not where it was when the row was drawn.
    let pick = move |_| {
        let place = entry.with_untracked(|entry| entry.as_ref().map(|entry| (entry.x, entry.y)));
        if let Some((x, y)) = place {
            on_pick.run((id.get_value(), x, y));
        }
    };
    view! {
        <li>
            <button
                type="button"
                class="object-row"
                class:own=own
                aria-current=move || current().then_some("true")
                aria-label=text(|entry| format!("{} at {},{}", entry.title, entry.x, entry.y))
                on:click=pick
            >
                <span class="name">{text(|entry| entry.title.clone())}</span>
                {owner}
                <span class="pos">{text(|entry| format!("{},{}", entry.x, entry.y))}</span>
            </button>
        </li>
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use overseer::core::api::types::GameRoomObject;
    use overseer::core::socket::types::RoomUser;
    use serde_json::json;

    use super::{Context, groups, natural, plural, topmost};

    fn objects() -> Vec<GameRoomObject> {
        serde_json::from_value(json!([
            {"_id": "src", "type": "source", "room": "W1N1", "x": 11, "y": 44, "energy": 3000,
             "energyCapacity": 3000, "ticksToRegeneration": 300},
            {"_id": "c10", "type": "creep", "room": "W1N1", "x": 7, "y": 43, "name": "w10",
             "user": "u1", "hits": 100, "hitsMax": 100, "store": {}, "body": []},
            {"_id": "e1", "type": "creep", "room": "W1N1", "x": 30, "y": 30, "name": "enemy",
             "user": "u2", "hits": 100, "hitsMax": 100, "store": {}, "body": []},
            {"_id": "c9", "type": "creep", "room": "W1N1", "x": 9, "y": 38, "name": "w9",
             "user": "u1", "hits": 100, "hitsMax": 100, "store": {}, "body": []},
            {"_id": "k", "type": "controller", "room": "W1N1", "x": 21, "y": 39, "user": "u1",
             "level": 2},
            {"_id": "road", "type": "road", "room": "W1N1", "x": 18, "y": 36, "hits": 5000,
             "hitsMax": 5000, "nextDecayTime": 1, "notifyWhenAttacked": true},
            {"_id": "s", "type": "spawn", "room": "W1N1", "x": 18, "y": 36, "name": "Spawn1",
             "user": "u1", "hits": 5000, "hitsMax": 5000, "notifyWhenAttacked": true,
             "store": {}, "storeCapacityResource": {}},
            {"_id": "es", "type": "spawn", "room": "W1N1", "x": 40, "y": 40, "name": "Theirs",
             "user": "u2", "hits": 5000, "hitsMax": 5000, "notifyWhenAttacked": true,
             "store": {}, "storeCapacityResource": {}},
            {"_id": "m", "type": "mineral", "room": "W1N1", "x": 6, "y": 45, "density": 3,
             "mineralAmount": 1.0, "mineralType": "O"},
            {"_id": "r", "type": "energy", "room": "W1N1", "x": 18, "y": 36, "energy": 5},
            {"_id": "kl", "type": "keeperLair", "room": "W1N1", "x": 2, "y": 2},
            {"_id": "fa", "type": "factory", "room": "W1N1", "x": 3, "y": 3, "user": "u2",
             "hits": 1, "hitsMax": 1, "cooldown": 0, "notifyWhenAttacked": true, "store": {}}
        ]))
        .unwrap()
    }

    fn users() -> BTreeMap<String, RoomUser> {
        [("u1", "alice"), ("u2", "bob")]
            .into_iter()
            .map(|(id, name)| {
                let user = serde_json::from_value(json!({"_id": id, "username": name})).unwrap();
                (id.to_owned(), user)
            })
            .collect()
    }

    fn keys(groups: &[super::Group]) -> Vec<&str> {
        groups.iter().map(|group| group.key.as_str()).collect()
    }

    #[test]
    fn groups_by_kind_with_own_groups_first() {
        let objects = objects();
        let users = users();
        let context = Context {
            game_time: Some(1),
            me: Some("u1"),
            users: &users,
        };
        let groups = groups(&objects, &context, "");
        assert_eq!(
            keys(&groups),
            [
                "creep",
                "spawn",
                "controller",
                "factory",
                "source",
                "mineral",
                "road",
                "resource",
                "keeperLair"
            ]
        );
        let creeps = &groups[0];
        assert_eq!(creeps.label, "Creeps");
        assert_eq!(creeps.own, 2);
        let names: Vec<&str> = creeps
            .entries
            .iter()
            .map(|entry| entry.title.as_str())
            .collect();
        assert_eq!(
            names,
            ["w9", "w10", "enemy"],
            "own first, names in natural order"
        );
        assert_eq!(creeps.entries[2].owner.as_deref(), Some("bob"));
        assert!(!creeps.entries[2].own);
        let spawns: Vec<&str> = groups[1]
            .entries
            .iter()
            .map(|entry| entry.title.as_str())
            .collect();
        assert_eq!(spawns, ["Spawn1", "Theirs"]);
        assert_eq!(groups[3].label, "Factories");
        assert_eq!(groups[7].label, "Dropped resources");
        assert_eq!(groups[7].entries[0].title, "5 energy");
    }

    #[test]
    fn without_a_user_kinds_keep_their_order() {
        let objects = objects();
        let users = users();
        let context = Context {
            game_time: None,
            me: None,
            users: &users,
        };
        assert_eq!(
            keys(&groups(&objects, &context, "")),
            [
                "creep",
                "spawn",
                "factory",
                "controller",
                "source",
                "mineral",
                "road",
                "resource",
                "keeperLair"
            ]
        );
    }

    #[test]
    fn filters_by_name_kind_owner_id_and_position() {
        let objects = objects();
        let users = users();
        let context = Context {
            game_time: None,
            me: Some("u1"),
            users: &users,
        };
        let titles = |filter: &str| -> Vec<String> {
            groups(&objects, &context, filter)
                .into_iter()
                .flat_map(|group| group.entries.into_iter().map(|entry| entry.title))
                .collect()
        };
        assert_eq!(titles("W1"), ["w10"], "names, any case");
        assert_eq!(titles("  spawn "), ["Spawn1", "Theirs"], "kinds, trimmed");
        assert_eq!(titles("bob"), ["enemy", "Theirs", "Factory"], "owners");
        assert_eq!(titles("18,36"), ["Spawn1", "Road", "5 energy"], "positions");
        assert_eq!(titles("kl"), ["Keeper lair"], "ids");
        assert_eq!(titles("nothing"), [] as [&str; 0]);
    }

    #[test]
    fn a_tile_selects_its_topmost_object() {
        let objects = objects();
        assert_eq!(topmost(&objects, 18, 36).map(GameRoomObject::id), Some("s"));
        assert_eq!(
            topmost(&objects, 7, 43).map(GameRoomObject::id),
            Some("c10")
        );
        assert_eq!(topmost(&objects, 0, 0).map(GameRoomObject::id), None);
    }

    #[test]
    fn names_and_plurals() {
        assert_eq!(natural("c9", "c10"), std::cmp::Ordering::Less);
        assert_eq!(natural("b", "a10"), std::cmp::Ordering::Greater);
        assert_eq!(plural("Creep"), "Creeps");
        assert_eq!(plural("Factory"), "Factories");
        assert_eq!(plural("Keeper lair"), "Keeper lairs");
    }
}
