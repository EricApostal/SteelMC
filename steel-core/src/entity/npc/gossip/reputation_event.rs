//! Vanilla `ReputationEventType` and `ReputationEventHandler`.

use std::fmt;

use crate::entity::SharedEntity;

/// A game event that changes villager reputation (vanilla `ReputationEventType`).
///
/// Vanilla implements the five constants as anonymous instances of the interface; the Rust shape
/// is a value type wrapping the vanilla event name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReputationEventType(&'static str);

impl ReputationEventType {
    /// Vanilla `ReputationEventType.ZOMBIE_VILLAGER_CURED`.
    pub const ZOMBIE_VILLAGER_CURED: Self = Self("zombie_villager_cured");
    /// Vanilla `ReputationEventType.GOLEM_KILLED`.
    pub const GOLEM_KILLED: Self = Self("golem_killed");
    /// Vanilla `ReputationEventType.VILLAGER_HURT`.
    pub const VILLAGER_HURT: Self = Self("villager_hurt");
    /// Vanilla `ReputationEventType.VILLAGER_KILLED`.
    pub const VILLAGER_KILLED: Self = Self("villager_killed");
    /// Vanilla `ReputationEventType.TRADE`.
    pub const TRADE: Self = Self("trade");

    /// All event types, in vanilla declaration order.
    pub const ALL: [Self; 5] = [
        Self::ZOMBIE_VILLAGER_CURED,
        Self::GOLEM_KILLED,
        Self::VILLAGER_HURT,
        Self::VILLAGER_KILLED,
        Self::TRADE,
    ];

    /// The vanilla event name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for ReputationEventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// An entity that reacts to reputation events (vanilla `ReputationEventHandler`).
///
/// Wave 2 implements this for the villager: the event-to-gossip mapping is
/// `Villager.onReputationEventFrom` (Villager.java:864-880) and
/// `ServerLevel.onReputationEvent` (ServerLevel.java:1694-1696) is the dispatch entry point.
pub trait ReputationEventHandler {
    /// Applies the reputation change this event causes for `source`.
    fn on_reputation_event_from(&self, event_type: ReputationEventType, source: SharedEntity);
}
