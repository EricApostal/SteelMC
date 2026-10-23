//! Vanilla `SetLookAndInteract`: remember the closest visible entity of a type as
//! the interaction target and look at it.

use std::sync::Arc;

use steel_registry::entity_type::EntityTypeRef;

use super::utils;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::tracker::EntityTracker;

/// Vanilla `SetLookAndInteract.create(EntityType, interactionRange)`.
pub struct SetLookAndInteract {
    target_type: EntityTypeRef,
    interaction_range_sqr: f64,
}

impl SetLookAndInteract {
    #[must_use]
    pub fn new(target_type: EntityTypeRef, interaction_range: i32) -> Self {
        let interaction_range = f64::from(interaction_range);
        Self {
            target_type,
            interaction_range_sqr: interaction_range * interaction_range,
        }
    }

    /// Vanilla `SetLookAndInteract.create(EntityType, interactionRange)`.
    #[must_use]
    pub fn create(target_type: EntityTypeRef, interaction_range: i32) -> OneShot<Self> {
        OneShot::new(Self::new(target_type, interaction_range)).with_required_memories(vec![
            &mem::LOOK_TARGET,
            &mem::INTERACTION_TARGET,
            &mem::NEAREST_VISIBLE_LIVING_ENTITIES,
        ])
    }
}

impl Trigger for SetLookAndInteract {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if ctx.has_memory_value(&mem::INTERACTION_TARGET) {
            return false;
        }

        let Some(visible) = ctx.memories().get(&mem::NEAREST_VISIBLE_LIVING_ENTITIES) else {
            return false;
        };

        let body = ctx.body;
        let target_type = self.target_type;
        let interaction_range_sqr = self.interaction_range_sqr;
        let Some(closest) = visible.find_closest(|entity| {
            entity.distance_to_sqr(body.position()) <= interaction_range_sqr
                && entity.entity_type() == target_type
        }) else {
            return false;
        };

        ctx.set_memory(&mem::INTERACTION_TARGET, closest.clone());
        utils::set_look_target(ctx, Arc::new(EntityTracker::new(closest, true)));
        true
    }
}
