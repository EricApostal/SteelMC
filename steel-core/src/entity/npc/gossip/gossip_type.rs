//! Vanilla `GossipType`: the reputation categories villagers gossip about.

use std::fmt;

/// One gossip category (vanilla `GossipType`).
///
/// `weight` scales the stored value into reputation points, `max` caps that value per type, and
/// `decay_per_day`/`decay_per_transfer` are subtracted by [`super::GossipContainer::decay`] and
/// [`super::GossipContainer::transfer_from`] respectively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GossipType {
    /// Vanilla `GossipType.MAJOR_NEGATIVE`.
    MajorNegative,
    /// Vanilla `GossipType.MINOR_NEGATIVE`.
    MinorNegative,
    /// Vanilla `GossipType.MINOR_POSITIVE`.
    MinorPositive,
    /// Vanilla `GossipType.MAJOR_POSITIVE`.
    MajorPositive,
    /// Vanilla `GossipType.TRADING`.
    Trading,
}

impl GossipType {
    /// Vanilla `GossipType.REPUTATION_CHANGE_PER_EVENT`, used for hurt/killed events.
    pub const REPUTATION_CHANGE_PER_EVENT: i32 = 25;
    /// Vanilla `GossipType.REPUTATION_CHANGE_PER_EVERLASTING_MEMORY`, used for curing a zombie
    /// villager.
    pub const REPUTATION_CHANGE_PER_EVERLASTING_MEMORY: i32 = 20;
    /// Vanilla `GossipType.REPUTATION_CHANGE_PER_TRADE`.
    pub const REPUTATION_CHANGE_PER_TRADE: i32 = 2;

    /// All gossip types in vanilla `GossipType.values()` order.
    pub const ALL: [Self; 5] = [
        Self::MajorNegative,
        Self::MinorNegative,
        Self::MinorPositive,
        Self::MajorPositive,
        Self::Trading,
    ];

    /// The vanilla serialized name (the NBT `Type` field and the codec id).
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::MajorNegative => "major_negative",
            Self::MinorNegative => "minor_negative",
            Self::MinorPositive => "minor_positive",
            Self::MajorPositive => "major_positive",
            Self::Trading => "trading",
        }
    }

    /// Parses a vanilla serialized name.
    #[must_use]
    pub const fn from_id(id: &str) -> Option<Self> {
        match id.as_bytes() {
            b"major_negative" => Some(Self::MajorNegative),
            b"minor_negative" => Some(Self::MinorNegative),
            b"minor_positive" => Some(Self::MinorPositive),
            b"major_positive" => Some(Self::MajorPositive),
            b"trading" => Some(Self::Trading),
            _ => None,
        }
    }

    /// Reputation points per stored value.
    #[must_use]
    pub const fn weight(self) -> i32 {
        match self {
            Self::MajorNegative => -5,
            Self::MinorNegative => -1,
            Self::MinorPositive | Self::Trading => 1,
            Self::MajorPositive => 5,
        }
    }

    /// Upper bound for a stored value of this type.
    #[must_use]
    pub const fn max(self) -> i32 {
        match self {
            Self::MajorNegative => 100,
            Self::MinorNegative => 200,
            Self::MinorPositive | Self::Trading => 25,
            Self::MajorPositive => 20,
        }
    }

    /// Value subtracted per gossip decay, which runs every
    /// [`super::GOSSIP_DECAY_INTERVAL`] ticks.
    #[must_use]
    pub const fn decay_per_day(self) -> i32 {
        match self {
            Self::MajorNegative => 10,
            Self::MinorNegative => 20,
            Self::MinorPositive => 1,
            Self::MajorPositive => 0,
            Self::Trading => 2,
        }
    }

    /// Value subtracted from a gossip that is shared with another gossip container.
    #[must_use]
    pub const fn decay_per_transfer(self) -> i32 {
        match self {
            Self::MajorNegative => 10,
            Self::MinorNegative | Self::MajorPositive | Self::Trading => 20,
            Self::MinorPositive => 5,
        }
    }
}

impl fmt::Display for GossipType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}
