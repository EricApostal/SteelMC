//! Vanilla `Npc`: a village inhabitant.

use crate::entity::Mob;

/// Vanilla `Npc`, the marker interface shared by villagers and wandering
/// traders.
///
/// Vanilla declares no members; Steel keeps it as a capability trait so code can
/// ask "is this an NPC" without downcasting to a concrete entity type.
pub trait Npc: Mob {}
