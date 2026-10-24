//! Vanilla `SocializeAtBell`.

use std::sync::Arc;

use steel_registry::vanilla_entities;

use super::utils;
use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, module_type as mem};
use crate::entity::ai::brain::tracker::EntityTracker;

/// Vanilla `SocializeAtBell.SPEED_MODIFIER`.
const SPEED_MODIFIER: f32 = 0.3;
/// Vanilla `SocializeAtBell` meeting-radius check, `closerToCenterThan(..., 4.0)`.
const MEETING_RADIUS: f64 = 4.0;
/// Vanilla `SocializeAtBell` target-search radius, `distanceToSqr <= 32`.
const SEARCH_DISTANCE_SQR: f64 = 32.0;
/// Vanilla `SocializeAtBell`'s 1/100 start gate.
const START_CHANCE: i32 = 100;

/// Vanilla `SocializeAtBell`: near a meeting point, face and drift toward a nearby
/// villager.
pub struct SocializeAtBell;

impl SocializeAtBell {
    /// Vanilla `SocializeAtBell.create`.
    #[must_use]
    pub fn create() -> OneShot<Self> {
        OneShot::new(Self).with_required_memories(vec![
            &mem::WALK_TARGET as MemoryModuleTypeRef,
            &mem::LOOK_TARGET as MemoryModuleTypeRef,
            &mem::MEETING_POINT as MemoryModuleTypeRef,
            &mem::NEAREST_VISIBLE_LIVING_ENTITIES as MemoryModuleTypeRef,
            &mem::INTERACTION_TARGET as MemoryModuleTypeRef,
        ])
    }
}

impl Trigger for SocializeAtBell {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        let Some(meeting_point) = ctx.get_memory(&mem::MEETING_POINT) else {
            return false;
        };
        let Some(visible) = ctx.memories().get(&mem::NEAREST_VISIBLE_LIVING_ENTITIES) else {
            return false;
        };

        if rand::random_range(0..START_CHANCE) != 0 {
            return false;
        }

        if meeting_point.dimension != ctx.level.key
            || !meeting_point
                .pos
                .closer_to_center_than(ctx.body.position(), MEETING_RADIUS)
            || !visible.contains_where(|entity| entity.entity_type() == &vanilla_entities::VILLAGER)
        {
            return false;
        }

        if let Some(mob) = visible.find_closest(|entity| {
            entity.entity_type() == &vanilla_entities::VILLAGER
                && entity.distance_to_sqr(ctx.body.position()) <= SEARCH_DISTANCE_SQR
        }) {
            ctx.set_memory(&mem::INTERACTION_TARGET, SharedEntity::clone(&mob));
            utils::set_look_target(
                ctx,
                Arc::new(EntityTracker::new(SharedEntity::clone(&mob), true)),
            );
            utils::set_walk_target(
                ctx,
                Arc::new(EntityTracker::new(mob, false)),
                SPEED_MODIFIER,
                1,
            );
        }

        true
    }
}
