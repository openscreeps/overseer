//! Where the user looks: the three tiers every kind of world-map work is ordered by, and the
//! client's request budget they share.

use overseer_net::rt::Instant;
use overseer_net::{Client, Priority};

use crate::map::{Interest, RoomCoord, RoomRect};

/// Rooms around the visible ones fetched with them (each side), so that a short pan finds
/// them known.
pub const PREFETCH_RING: i32 = 3;

/// How urgent a room's work is; lower first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum Tier {
    /// A room viewed in detail ([`Interest::detail`]).
    Viewed,
    /// A visible room or one of the prefetch ring around them.
    Visible,
    /// Any other room of the world.
    Background,
}

impl Tier {
    /// The client's request priority of work of this tier.
    const fn priority(self) -> Priority {
        match self {
            Self::Viewed => Priority::Viewed,
            Self::Visible => Priority::Visible,
            Self::Background => Priority::Background,
        }
    }
}

/// How a server limits requests, now (see [`WorldMap`](super::WorldMap)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Server {
    /// No rate limit seen, or the token is exempt: in-flight caps only.
    Unlimited,
    /// It lists shards, reported a rate limit or refused a request with HTTP 429.
    Limited,
}

impl Server {
    /// How `client`'s server limits requests now.
    pub(super) fn of(client: &Client) -> Self {
        if client.has_rate_limits() && !client.is_rate_limit_exempt() {
            Self::Limited
        } else {
            Self::Unlimited
        }
    }
}

/// The client's request budget ([`Client::budget_ready_at`]) as one planning pass spends it:
/// requests it allows go out once the pass ends, so they count as pending meanwhile.
pub(super) struct Budget<'a> {
    client: &'a Client,
    granted: usize,
}

impl<'a> Budget<'a> {
    pub(super) const fn new(client: &'a Client) -> Self {
        Self { client, granted: 0 }
    }

    /// The client whose budget this is.
    pub(super) const fn client(&self) -> &'a Client {
        self.client
    }

    /// Takes one request of `tier`, or tells when one may go out.
    pub(super) fn take(&mut self, tier: Tier) -> Result<(), Instant> {
        match self.client.budget_ready_at(tier.priority(), self.granted) {
            None => {
                self.granted += 1;
                Ok(())
            }
            Some(at) => Err(at),
        }
    }
}
/// The rooms of each tier, from the latest interest.
#[derive(Clone, Debug)]
pub(super) struct Focus {
    /// The viewed rooms, most important first.
    viewed: Vec<RoomCoord>,
    /// The visible rooms and the prefetch ring around them, inside the world.
    visible: Option<RoomRect>,
    /// Visible and background rooms nearest it go first.
    centre: RoomCoord,
}

impl Focus {
    /// Nothing viewed or visible in `world`.
    pub(super) fn new(world: RoomRect) -> Self {
        Self {
            viewed: Vec::new(),
            visible: None,
            centre: world.center(),
        }
    }

    /// The tiers of `interest` in `world`: the viewed rooms are `viewed` (its detail rooms,
    /// unique); the visible ones are those of its overviews (else its portals, else the
    /// bounding box of its terrain) plus [`PREFETCH_RING`] rooms around them; the centre is
    /// its first terrain room (the renderer lists them nearest the camera first), else the
    /// visible rooms' centre.
    pub(super) fn of(interest: &Interest, viewed: Vec<RoomCoord>, world: RoomRect) -> Self {
        let view = interest
            .stats
            .or(interest.portals)
            .or_else(|| bounding(&interest.terrain))
            .or_else(|| bounding(&viewed));
        let visible = view.and_then(|rect| grow(rect, PREFETCH_RING).intersection(&world));
        let centre = interest
            .terrain
            .first()
            .or(viewed.first())
            .copied()
            .or_else(|| view.map(|rect| rect.center()))
            .unwrap_or_else(|| world.center());
        Self {
            viewed,
            visible,
            centre,
        }
    }

    /// The visible rooms and the prefetch ring, inside the world.
    pub(super) const fn visible(&self) -> Option<RoomRect> {
        self.visible
    }

    /// Visible and background rooms nearest it go first.
    pub(super) const fn centre(&self) -> RoomCoord {
        self.centre
    }

    /// The tier of `room`.
    pub(super) fn tier(&self, room: RoomCoord) -> Tier {
        if self.viewed.contains(&room) {
            Tier::Viewed
        } else if self.visible.is_some_and(|rect| rect.contains(room)) {
            Tier::Visible
        } else {
            Tier::Background
        }
    }

    /// The order of `room`'s work: by tier, the viewed rooms in their order, the others
    /// nearest the centre first (ties north-west first).
    pub(super) fn key(&self, room: RoomCoord) -> (Tier, usize, i64, i32, i32) {
        let tier = self.tier(room);
        let rank = if tier == Tier::Viewed {
            self.viewed
                .iter()
                .position(|&viewed| viewed == room)
                .unwrap_or(0)
        } else {
            0
        };
        (tier, rank, distance(room, self.centre), room.y, room.x)
    }
}

/// The squared distance between two rooms.
pub(super) fn distance(room: RoomCoord, centre: RoomCoord) -> i64 {
    let (dx, dy) = (
        i64::from(room.x) - i64::from(centre.x),
        i64::from(room.y) - i64::from(centre.y),
    );
    dx * dx + dy * dy
}

/// The smallest rectangle holding `rooms`; `None` when there are none.
fn bounding(rooms: &[RoomCoord]) -> Option<RoomRect> {
    let (first, rest) = rooms.split_first()?;
    Some(
        rest.iter()
            .fold(RoomRect::new(*first, *first), |rect, room| {
                RoomRect::new(
                    RoomCoord {
                        x: rect.min.x.min(room.x),
                        y: rect.min.y.min(room.y),
                    },
                    RoomCoord {
                        x: rect.max.x.max(room.x),
                        y: rect.max.y.max(room.y),
                    },
                )
            }),
    )
}

/// `rect` grown by `rooms` on each side.
const fn grow(rect: RoomRect, rooms: i32) -> RoomRect {
    RoomRect {
        min: RoomCoord {
            x: rect.min.x.saturating_sub(rooms),
            y: rect.min.y.saturating_sub(rooms),
        },
        max: RoomCoord {
            x: rect.max.x.saturating_add(rooms),
            y: rect.max.y.saturating_add(rooms),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{Focus, PREFETCH_RING, Tier};
    use crate::map::{Interest, RoomCoord, RoomRect};

    fn room(name: &str) -> RoomCoord {
        name.parse().unwrap()
    }

    #[test]
    fn tiers_are_viewed_then_visible_with_the_ring_then_the_rest() {
        let world = RoomRect::new(room("W19N19"), room("E19S19"));
        let interest = Interest {
            stats: Some(RoomRect::new(room("W19N19"), room("W15N15"))),
            terrain: vec![room("W17N17")],
            ..Interest::default()
        };
        let focus = Focus::of(&interest, vec![room("E10S10"), room("E1S1")], world);
        // The ring stays inside the world.
        assert_eq!(
            focus.visible(),
            Some(RoomRect::new(room("W19N19"), room("W12N12")))
        );
        assert_eq!(PREFETCH_RING, 3);
        assert_eq!(focus.centre(), room("W17N17"));
        assert_eq!(focus.tier(room("E1S1")), Tier::Viewed);
        assert_eq!(focus.tier(room("W12N12")), Tier::Visible);
        assert_eq!(focus.tier(room("W11N12")), Tier::Background);
        let mut rooms = [
            room("W0N0"),
            room("E1S1"),
            room("W12N12"),
            room("E10S10"),
            room("W17N17"),
        ];
        rooms.sort_by_key(|&room| focus.key(room));
        assert_eq!(
            rooms,
            [
                room("E10S10"),
                room("E1S1"),
                room("W17N17"),
                room("W12N12"),
                room("W0N0")
            ]
        );
        // Without a view, the viewed rooms' box is the view.
        let alone = Focus::of(&Interest::default(), vec![room("E1S1")], world);
        assert_eq!(alone.centre(), room("E1S1"));
        assert_eq!(alone.tier(room("E4S4")), Tier::Visible);
    }
}
