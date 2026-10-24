//! Vanilla `InteractWith`: walk to and look at the closest visible entity of a
//! type, remembering it in the interaction or breed target memory.

use std::sync::Arc;

use steel_registry::entity_type::EntityTypeRef;

use super::utils;
use super::villager;
use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::{MemoryModuleType, module_type as mem};
use crate::entity::ai::brain::tracker::EntityTracker;

/// Vanilla `InteractWith.of`'s `selfFilter`/`targetFilter` pair.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InteractFilter {
    /// Vanilla `mob -> true`.
    Any,
    /// Vanilla `AgeableMob::canBreed`, which villagers implement as
    /// `Villager.canBreed` (breeding food plus age and not sleeping).
    CanBreed,
}

impl InteractFilter {
    fn matches(self, entity: &SharedEntity) -> bool {
        match self {
            Self::Any => true,
            Self::CanBreed => {
                let Some(villager) = villager::as_villager(entity) else {
                    return false;
                };

                entity
                    .as_living_entity()
                    .is_some_and(|living| !living.is_baby())
                    && villager.can_breed_food()
            }
        }
    }
}

/// Vanilla `InteractWith.of(EntityType, interactionRange, memory, speed, stopDistance)`.
pub struct InteractWith {
    target_type: EntityTypeRef,
    interaction_range_sqr: f64,
    target_memory: &'static MemoryModuleType<SharedEntity>,
    speed_modifier: f32,
    stop_distance: i32,
    filter: InteractFilter,
}

impl InteractWith {
    #[must_use]
    pub fn new(
        target_type: EntityTypeRef,
        interaction_range: i32,
        target_memory: &'static MemoryModuleType<SharedEntity>,
        speed_modifier: f32,
        stop_distance: i32,
        filter: InteractFilter,
    ) -> Self {
        let interaction_range = f64::from(interaction_range);
        Self {
            target_type,
            interaction_range_sqr: interaction_range * interaction_range,
            target_memory,
            speed_modifier,
            stop_distance,
            filter,
        }
    }

    /// Vanilla `InteractWith.of(type, range, memory, speed, stop)`.
    #[must_use]
    pub fn of(
        target_type: EntityTypeRef,
        interaction_range: i32,
        target_memory: &'static MemoryModuleType<SharedEntity>,
        speed_modifier: f32,
        stop_distance: i32,
    ) -> OneShot<Self> {
        Self::create(
            target_type,
            interaction_range,
            target_memory,
            speed_modifier,
            stop_distance,
            InteractFilter::Any,
        )
    }

    /// Vanilla `InteractWith.of(type, range, AgeableMob::canBreed, AgeableMob::canBreed, memory, speed, stop)`.
    #[must_use]
    pub fn of_breeding(
        target_type: EntityTypeRef,
        interaction_range: i32,
        target_memory: &'static MemoryModuleType<SharedEntity>,
        speed_modifier: f32,
        stop_distance: i32,
    ) -> OneShot<Self> {
        Self::create(
            target_type,
            interaction_range,
            target_memory,
            speed_modifier,
            stop_distance,
            InteractFilter::CanBreed,
        )
    }

    fn create(
        target_type: EntityTypeRef,
        interaction_range: i32,
        target_memory: &'static MemoryModuleType<SharedEntity>,
        speed_modifier: f32,
        stop_distance: i32,
        filter: InteractFilter,
    ) -> OneShot<Self> {
        OneShot::new(Self::new(
            target_type,
            interaction_range,
            target_memory,
            speed_modifier,
            stop_distance,
            filter,
        ))
        .with_required_memories(vec![
            target_memory,
            &mem::LOOK_TARGET,
            &mem::WALK_TARGET,
            &mem::NEAREST_VISIBLE_LIVING_ENTITIES,
        ])
    }

    fn is_target_valid(&self, entity: &SharedEntity) -> bool {
        entity.entity_type() == self.target_type && self.filter.matches(entity)
    }
}

impl Trigger for InteractWith {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if ctx.has_memory_value(&mem::WALK_TARGET) {
            return false;
        }

        if !self.filter.matches(ctx.body) {
            return false;
        }

        let body = ctx.body;
        let interaction_range_sqr = self.interaction_range_sqr;
        let Some(visible) = ctx.memories().get(&mem::NEAREST_VISIBLE_LIVING_ENTITIES) else {
            return false;
        };

        if !visible.contains_where(|entity| self.is_target_valid(entity)) {
            return false;
        }

        let closest = visible.find_closest(|entity| {
            entity.distance_to_sqr(body.position()) <= interaction_range_sqr
                && self.is_target_valid(entity)
        });

        if let Some(closest) = closest {
            ctx.set_memory(self.target_memory, SharedEntity::clone(&closest));
            utils::set_look_target(
                ctx,
                Arc::new(EntityTracker::new(SharedEntity::clone(&closest), true)),
            );
            utils::set_walk_target(
                ctx,
                Arc::new(EntityTracker::new(closest, false)),
                self.speed_modifier,
                self.stop_distance,
            );
        }

        true
    }
}
