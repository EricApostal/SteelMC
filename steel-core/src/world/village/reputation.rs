//! Level-level reputation dispatch (vanilla `ServerLevel.onReputationEvent`).
//!
//! Vanilla has two entry points around reputation events:
//! - `ServerLevel.onReputationEvent(type, source, target)` delegates to one
//!   `ReputationEventHandler`. Trades, hurts and cures go through it.
//! - `Villager.tellWitnessesThatIWasMurdered(murderer)` walks the dying villager's
//!   `NEAREST_VISIBLE_LIVING_ENTITIES` memory, keeps the witnesses implementing
//!   `ReputationEventHandler` and reports the kill to each of them.
//!
//! Steel resolves the capability through [`Entity::as_reputation_event_handler`], so the
//! witness scan is an ordinary entity query. The villager's memory-based witness scan
//! lives in the villager slice; [`World::report_reputation_event_to_witnesses`] is the
//! level-side query those callers use when they only have a position.

use std::sync::Arc;

use steel_utils::{BlockPos, WorldAabb};

use crate::entity::SharedEntity;
use crate::entity::npc::{ReputationEventHandler, ReputationEventType};
use crate::world::World;

/// Vanilla `Villager`'s witness scan radius for `tellWitnessesThatIWasMurdered` is the
/// villager's own `NEAREST_VISIBLE_LIVING_ENTITIES` memory; the level-side query uses
/// the vanilla follow range that memory is built from.
pub const WITNESS_SCAN_RADIUS: f64 = 16.0;

impl World {
    /// Applies a reputation event to one handler
    /// (vanilla `ServerLevel.onReputationEvent`).
    pub fn on_reputation_event(
        &self,
        event_type: ReputationEventType,
        source: &SharedEntity,
        target: &dyn ReputationEventHandler,
    ) {
        target.on_reputation_event_from(event_type, Arc::clone(source));
    }

    /// Reports `event_type` to every reputation handler within `radius` of `center`
    /// (vanilla `Villager.tellWitnessesThatIWasMurdered`).
    pub fn report_reputation_event_to_witnesses(
        &self,
        event_type: ReputationEventType,
        source: &SharedEntity,
        center: BlockPos,
        radius: f64,
    ) {
        let bounds = WorldAabb::new(
            f64::from(center.x()),
            f64::from(center.y()),
            f64::from(center.z()),
            f64::from(center.x() + 1),
            f64::from(center.y() + 1),
            f64::from(center.z() + 1),
        )
        .inflate(radius);

        for witness in self.get_entities_in_aabb(&bounds) {
            if let Some(handler) = witness.as_reputation_event_handler() {
                self.on_reputation_event(event_type, source, handler);
            }
        }
    }
}
