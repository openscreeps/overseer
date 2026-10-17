//! Where the user was: the last route, settings, and per server the last shard, the camera,
//! last room and world bounds of each shard, and the view mode.
//!
//! Persisted as one JSON document (`localStorage` key [`STORAGE_KEY`], written with
//! [`VERSION`]) by the main window only; popped-out windows read it and never write it.
//! Parsing never fails: a document of another version is read for the fields it shares,
//! missing fields default, and every entry (server, shard, camera, room) that does not
//! decode is dropped on its own.

use std::collections::BTreeMap;

use overseer::map::{RoomCoord, RoomRect};
use overseer_view::{CameraState, ViewMode};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::route::{Route, parse_room};

/// `localStorage` key of the document.
pub const STORAGE_KEY: &str = "overseer.state";
/// Version written into the document.
pub const VERSION: u32 = 1;

/// App settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The right drawer (room, objects, console) is open.
    pub drawer: bool,
    /// The drawer's width in CSS pixels (clamped to the window when shown).
    #[serde(deserialize_with = "lenient_width")]
    pub drawer_width: u16,
    /// The drawer's height as a bottom sheet (phone layout), in percent of the world screen
    /// (clamped to the sheet's limits when shown).
    #[serde(deserialize_with = "lenient_height")]
    pub drawer_height: u8,
    /// The drawer's tab.
    #[serde(deserialize_with = "lenient")]
    pub drawer_tab: DrawerTab,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            drawer: true,
            drawer_width: DRAWER_WIDTH,
            drawer_height: DRAWER_HEIGHT,
            drawer_tab: DrawerTab::Room,
        }
    }
}

/// The drawer's default width in CSS pixels.
pub const DRAWER_WIDTH: u16 = 384;
/// The bottom sheet's default height, in percent of the world screen.
pub const DRAWER_HEIGHT: u8 = 50;

/// A tab of the drawer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DrawerTab {
    /// The room in focus: preview and facts.
    #[default]
    Room,
    /// The room's objects and the inspector.
    Objects,
    /// The account's console.
    Console,
    /// The signed-in user's Memory and segments.
    Memory,
}

/// A tab that does not decode is the default one, leaving the other settings.
fn lenient<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    let value = Value::deserialize(deserializer)?;
    Ok(T::deserialize(value).unwrap_or_default())
}

/// A width that does not decode is [`DRAWER_WIDTH`], leaving the other settings.
fn lenient_width<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u16, D::Error> {
    let value = Value::deserialize(deserializer)?;
    Ok(u16::deserialize(value).unwrap_or(DRAWER_WIDTH))
}

/// A height that does not decode is [`DRAWER_HEIGHT`], leaving the other settings.
fn lenient_height<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u8, D::Error> {
    let value = Value::deserialize(deserializer)?;
    Ok(u8::deserialize(value).unwrap_or(DRAWER_HEIGHT))
}

/// How long known world bounds are trusted before the world is measured again, in
/// milliseconds (a day: private servers can grow their world).
pub const BOUNDS_MAX_AGE_MS: f64 = 24.0 * 60.0 * 60.0 * 1000.0;

/// The rooms of a shard's world as last measured.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    /// The rooms.
    pub rect: RoomRect,
    /// When they were measured, milliseconds since the epoch.
    pub at: f64,
}

/// What is remembered of one server's world.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Place {
    /// The shard last shown, as its route segment (`-` on single-world servers).
    pub shard: Option<String>,
    /// The view mode.
    pub mode: ViewMode,
    /// The camera of each shard, by route segment.
    pub cameras: BTreeMap<String, CameraState>,
    /// The room last focused on each shard, by route segment.
    pub rooms: BTreeMap<String, RoomCoord>,
    /// The world bounds of each shard, by route segment.
    pub bounds: BTreeMap<String, Bounds>,
}

/// The document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Saved {
    /// The main window's last route.
    pub route: Option<Route>,
    /// Settings.
    pub settings: Settings,
    /// Per server, by base URL.
    pub places: BTreeMap<String, Place>,
}

impl Saved {
    /// Reads the document leniently (see the module docs); `None` or invalid JSON give the
    /// defaults.
    pub fn parse(json: Option<&str>) -> Self {
        let Some(Value::Object(document)) = json.and_then(|json| serde_json::from_str(json).ok())
        else {
            return Self::default();
        };
        let route = document
            .get("route")
            .and_then(Value::as_str)
            .and_then(Route::parse);
        let settings = document
            .get("settings")
            .and_then(|settings| Settings::deserialize(settings).ok())
            .unwrap_or_default();
        let places = object(document.get("places"))
            .map(|places| {
                places
                    .iter()
                    .filter_map(|(url, place)| Some((url.clone(), parse_place(place)?)))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            route,
            settings,
            places,
        }
    }

    /// The document to save.
    pub fn to_json(&self) -> String {
        let places: Map<String, Value> = self
            .places
            .iter()
            .map(|(url, place)| (url.clone(), place_json(place)))
            .collect();
        let mut document = Map::new();
        document.insert("version".to_owned(), Value::from(VERSION));
        if let Some(route) = &self.route {
            document.insert("route".to_owned(), Value::from(route.hash()));
        }
        document.insert(
            "settings".to_owned(),
            serde_json::to_value(self.settings).unwrap_or(Value::Null),
        );
        document.insert("places".to_owned(), Value::Object(places));
        Value::Object(document).to_string()
    }

    /// The place of the server with base URL `url`, created empty if needed.
    pub fn place_mut(&mut self, url: &str) -> &mut Place {
        self.places.entry(url.to_owned()).or_default()
    }

    /// Forgets the server with base URL `url` (and the last route when it was about it).
    pub fn forget(&mut self, url: &str, slug: Option<&str>) {
        self.places.remove(url);
        if slug.is_some() && self.route.as_ref().and_then(Route::server) == slug {
            self.route = None;
        }
    }
}

/// `value` as a JSON object.
fn object(value: Option<&Value>) -> Option<&Map<String, Value>> {
    value.and_then(Value::as_object)
}

/// Decodes one place, keeping what decodes; `None` unless it is an object.
fn parse_place(value: &Value) -> Option<Place> {
    let place = value.as_object()?;
    let shard = place
        .get("shard")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mode = place
        .get("mode")
        .and_then(|mode| ViewMode::deserialize(mode).ok())
        .unwrap_or_default();
    let cameras = object(place.get("cameras"))
        .map(|cameras| {
            cameras
                .iter()
                .filter_map(|(shard, camera)| {
                    Some((shard.clone(), CameraState::deserialize(camera).ok()?))
                })
                .collect()
        })
        .unwrap_or_default();
    let rooms = object(place.get("rooms"))
        .map(|rooms| {
            rooms
                .iter()
                .filter_map(|(shard, room)| Some((shard.clone(), parse_room(room.as_str()?)?)))
                .collect()
        })
        .unwrap_or_default();
    let bounds = object(place.get("bounds"))
        .map(|bounds| {
            bounds
                .iter()
                .filter_map(|(shard, value)| Some((shard.clone(), parse_bounds(value)?)))
                .collect()
        })
        .unwrap_or_default();
    Some(Place {
        shard,
        mode,
        cameras,
        rooms,
        bounds,
    })
}

/// Decodes `{"min": "W10N10", "max": "W0N0", "at": 1.0}`.
fn parse_bounds(value: &Value) -> Option<Bounds> {
    let room = |key: &str| parse_room(value.get(key)?.as_str()?);
    Some(Bounds {
        rect: RoomRect::new(room("min")?, room("max")?),
        at: value.get("at")?.as_f64()?,
    })
}

/// Encodes one place.
fn place_json(place: &Place) -> Value {
    let cameras: Map<String, Value> = place
        .cameras
        .iter()
        .filter_map(|(shard, camera)| Some((shard.clone(), serde_json::to_value(camera).ok()?)))
        .collect();
    let rooms: Map<String, Value> = place
        .rooms
        .iter()
        .map(|(shard, room)| (shard.clone(), Value::from(room.to_string())))
        .collect();
    let mut json = Map::new();
    if let Some(shard) = &place.shard {
        json.insert("shard".to_owned(), Value::from(shard.clone()));
    }
    json.insert(
        "mode".to_owned(),
        serde_json::to_value(place.mode).unwrap_or(Value::Null),
    );
    let bounds: Map<String, Value> = place
        .bounds
        .iter()
        .map(|(shard, bounds)| {
            let value = serde_json::json!({
                "min": bounds.rect.min.to_string(),
                "max": bounds.rect.max.to_string(),
                "at": bounds.at,
            });
            (shard.clone(), value)
        })
        .collect();
    json.insert("cameras".to_owned(), Value::Object(cameras));
    json.insert("rooms".to_owned(), Value::Object(rooms));
    json.insert("bounds".to_owned(), Value::Object(bounds));
    Value::Object(json)
}

#[cfg(test)]
mod tests {
    use overseer_view::{CameraState, ViewMode};
    use serde_json::json;

    use overseer::map::RoomRect;

    use super::{Bounds, DRAWER_HEIGHT, DRAWER_WIDTH, DrawerTab, Place, Saved, Settings};
    use crate::route::Route;

    fn camera() -> CameraState {
        serde_json::from_value(json!({
            "centre": [-425.0, -175.0],
            "half_height": 30.0,
            "mode": "Perspective",
            "yaw": 0.5,
            "pitch": 0.9
        }))
        .unwrap()
    }

    #[test]
    fn round_trips() {
        let mut saved = Saved {
            route: Route::parse("#/screeps.com/shard3/W8N3"),
            settings: Settings {
                drawer: false,
                drawer_width: 512,
                drawer_height: 85,
                drawer_tab: DrawerTab::Console,
            },
            ..Saved::default()
        };
        let place = saved.place_mut("https://screeps.com/");
        place.shard = Some("shard3".to_owned());
        place.mode = ViewMode::Perspective;
        place.cameras.insert("shard3".to_owned(), camera());
        place
            .rooms
            .insert("shard3".to_owned(), "W8N3".parse().unwrap());
        let local = saved.place_mut("http://localhost:21025/");
        local.shard = Some("-".to_owned());
        local.bounds.insert(
            "-".to_owned(),
            Bounds {
                rect: RoomRect::new("W10N10".parse().unwrap(), "W0N0".parse().unwrap()),
                at: 1_792_000_000_000.0,
            },
        );
        assert_eq!(Saved::parse(Some(&saved.to_json())), saved);
    }

    #[test]
    fn defaults_for_missing_or_invalid_documents() {
        for json in [
            None,
            Some(""),
            Some("[]"),
            Some("{"),
            Some("{}"),
            Some("null"),
        ] {
            assert_eq!(Saved::parse(json), Saved::default(), "{json:?}");
        }
        let settings = Saved::default().settings;
        assert!(settings.drawer);
        assert_eq!(settings.drawer_width, DRAWER_WIDTH);
        assert_eq!(settings.drawer_height, DRAWER_HEIGHT);
        assert_eq!(settings.drawer_tab, DrawerTab::Room);
    }

    #[test]
    fn keeps_what_decodes() {
        let json = json!({
            "version": 9,
            "future": true,
            "route": "#/not a route",
            "settings": {
                "drawer": false,
                "unknown": 1,
                "drawer_width": -5,
                "drawer_height": 300,
                "drawer_tab": "maps"
            },
            "places": {
                "http://a/": {
                    "shard": "shard1",
                    "mode": "Sideways",
                    "cameras": {"shard1": {"centre": "bad"}, "shard2": camera()},
                    "rooms": {"shard1": "w1n1", "shard2": "sim", "shard3": 5},
                    "bounds": {
                        "shard1": {"min": "W2N2", "max": "E1S1", "at": 5},
                        "shard2": {"min": "W2N2", "at": 5},
                        "shard3": {"min": "W2N2", "max": "E1S1"}
                    }
                },
                "http://b/": "not an object",
                "http://c/": {}
            }
        });
        let saved = Saved::parse(Some(&json.to_string()));
        assert_eq!(saved.route, None);
        assert!(
            !saved.settings.drawer,
            "kept next to settings that do not decode"
        );
        assert_eq!(saved.settings.drawer_width, DRAWER_WIDTH);
        assert_eq!(saved.settings.drawer_height, DRAWER_HEIGHT);
        assert_eq!(saved.settings.drawer_tab, DrawerTab::Room);
        let tab = Saved::parse(Some(
            r#"{"settings": {"drawer_tab": "objects", "drawer_width": 300, "drawer_height": 30}}"#,
        ));
        assert_eq!(tab.settings.drawer_tab, DrawerTab::Objects);
        assert_eq!(tab.settings.drawer_width, 300);
        assert_eq!(tab.settings.drawer_height, 30);
        let place = &saved.places["http://a/"];
        assert_eq!(place.shard.as_deref(), Some("shard1"));
        assert_eq!(place.mode, ViewMode::TopDown);
        assert_eq!(place.cameras.keys().collect::<Vec<_>>(), ["shard2"]);
        assert_eq!(place.rooms.len(), 1);
        assert_eq!(place.rooms["shard1"].to_string(), "W1N1");
        assert_eq!(place.bounds.keys().collect::<Vec<_>>(), ["shard1"]);
        assert_eq!(place.bounds["shard1"].rect.len(), 25);
        assert!(!saved.places.contains_key("http://b/"));
        assert_eq!(saved.places["http://c/"], Place::default());
    }

    #[test]
    fn forgetting_a_server_drops_its_route() {
        let mut saved = Saved {
            route: Route::parse("#/a/-/W1N1"),
            ..Saved::default()
        };
        saved.place_mut("http://a/");
        saved.place_mut("http://b/");
        saved.forget("http://b/", Some("b"));
        assert!(saved.route.is_some());
        saved.forget("http://a/", Some("a"));
        assert_eq!(saved.route, None);
        assert!(saved.places.is_empty());
    }
}
