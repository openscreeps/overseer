//! How each kind of object is drawn: shape, size, colour and stacking layer.

use overseer::core::api::types::GameRoomObject;

use crate::mesh::Shape;

/// User id of the Invader NPC.
pub const INVADER: &str = "2";
/// User id of the Source Keeper NPC.
pub const SOURCE_KEEPER: &str = "3";

/// Owner colours (sRGB): the signed-in user, other players and the two NPCs.
const MINE: u32 = 0x4c_af_50;
const OTHER: u32 = 0xe5_39_35;
const INVADER_COLOR: u32 = 0xff_98_00;
const KEEPER_COLOR: u32 = 0xab_47_bc;

/// The drawing of one kind of object.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    /// Shape drawn.
    pub shape: Shape,
    /// Footprint width (tiles) and height.
    pub width: f32,
    /// Height above the tile.
    pub height: f32,
    /// Colour (sRGB) when unowned, or of kinds never coloured by owner.
    pub color: u32,
    /// Whether an owner colours the object.
    pub owned: bool,
    /// Stacking layer: objects of a higher layer are on top (listed first when picked).
    pub layer: u8,
}

/// Who owns an object, relative to the signed-in user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// No owner.
    None,
    /// The signed-in user.
    Mine,
    /// Another player.
    Other,
    /// The Invader NPC (user `2`).
    Invader,
    /// The Source Keeper NPC (user `3`).
    SourceKeeper,
}

impl Owner {
    /// The owner class of `user` when `me` is signed in.
    pub fn of(user: Option<&str>, me: Option<&str>) -> Self {
        match user {
            None => Self::None,
            Some(INVADER) => Self::Invader,
            Some(SOURCE_KEEPER) => Self::SourceKeeper,
            Some(user) if Some(user) == me => Self::Mine,
            Some(_) => Self::Other,
        }
    }

    /// The owner's colour (sRGB); `None` without an owner.
    pub const fn color(self) -> Option<u32> {
        match self {
            Self::None => None,
            Self::Mine => Some(MINE),
            Self::Other => Some(OTHER),
            Self::Invader => Some(INVADER_COLOR),
            Self::SourceKeeper => Some(KEEPER_COLOR),
        }
    }
}

impl Style {
    /// The style of an object.
    pub fn of(object: &GameRoomObject) -> Self {
        use GameRoomObject as O;
        use Shape::{Box, Cylinder, Ring};
        let (shape, width, height, color, owned, layer) = match object {
            O::Creep(_) => (Cylinder, 0.7, 0.55, OTHER, true, 9),
            O::PowerCreep(_) => (Cylinder, 0.75, 0.75, OTHER, true, 9),
            O::Resource(_) => (Cylinder, 0.3, 0.08, resource(object.kind()), false, 8),
            O::Nuke(_) => (Cylinder, 0.35, 1.1, 0xff_57_22, false, 8),
            O::Tombstone(_) => (Box, 0.3, 0.25, 0x9e_9e_9e, false, 7),
            O::Ruin(_) => (Box, 0.6, 0.12, 0x6d_5d_4b, false, 7),
            O::Rampart(_) => (Ring, 1.0, 0.3, OTHER, true, 6),
            O::Spawn(_) => (Cylinder, 0.9, 0.45, OTHER, true, 5),
            O::Extension(_) => (Cylinder, 0.5, 0.25, OTHER, true, 5),
            O::Controller(_) => (Box, 0.85, 0.35, 0x70_70_70, true, 5),
            O::Source(_) => (Box, 0.75, 0.45, 0xff_f2_46, false, 5),
            O::Mineral(_) => (Box, 0.75, 0.45, 0xc5_ca_e9, false, 5),
            O::Deposit(_) => (Box, 0.8, 0.35, 0x6e_9e_c4, false, 5),
            O::ConstructedWall(_) => (Box, 0.96, 0.5, 0x4a_4a_4a, false, 5),
            O::Link(_) => (Box, 0.4, 0.4, OTHER, true, 5),
            O::Storage(_) => (Box, 0.85, 0.6, OTHER, true, 5),
            O::Tower(_) => (Cylinder, 0.55, 0.65, OTHER, true, 5),
            O::Observer(_) => (Cylinder, 0.35, 0.35, OTHER, true, 5),
            O::PowerBank(_) => (Box, 0.85, 0.55, 0xff_4f_a0, false, 5),
            O::PowerSpawn(_) => (Cylinder, 0.85, 0.5, OTHER, true, 5),
            O::Extractor(_) => (Ring, 0.9, 0.2, OTHER, true, 5),
            O::Lab(_) => (Cylinder, 0.55, 0.45, OTHER, true, 5),
            O::Terminal(_) => (Box, 0.75, 0.55, OTHER, true, 5),
            O::Container(_) => (Box, 0.5, 0.35, 0xb4_9b_6b, false, 5),
            O::Nuker(_) => (Box, 0.6, 0.85, OTHER, true, 5),
            O::Factory(_) => (Box, 0.8, 0.5, OTHER, true, 5),
            O::InvaderCore(_) => (Box, 0.8, 0.55, INVADER_COLOR, true, 5),
            O::KeeperLair(_) => (Box, 0.85, 0.45, KEEPER_COLOR, true, 5),
            O::Portal(_) => (Cylinder, 0.9, 0.12, 0x5a_d1_ff, false, 5),
            O::ConstructionSite(_) => (Box, 0.45, 0.06, OTHER, true, 3),
            O::Road(_) => (Box, 0.45, 0.03, 0x5e_5e_5e, false, 2),
            O::Other(_) => (Box, 0.5, 0.3, 0xbd_bd_bd, false, 1),
        };
        Self {
            shape,
            width,
            height,
            color,
            owned,
            layer,
        }
    }

    /// The colour (sRGB) of an object of this style owned by `owner`.
    pub const fn color(&self, owner: Owner) -> u32 {
        if !self.owned {
            return self.color;
        }
        match owner.color() {
            Some(color) => color,
            None => self.color,
        }
    }
}

/// The colour (sRGB) of a dropped resource.
fn resource(kind: &str) -> u32 {
    match kind {
        "energy" => 0xff_e5_6d,
        "power" => 0xff_2a_5a,
        _ => 0xe0_e0_e0,
    }
}
