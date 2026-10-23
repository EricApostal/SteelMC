//! Vanilla `GoAndGiveItemsToTarget`: walk to a deposit spot and throw an item.

use std::sync::Arc;

use glam::DVec3;

use super::utils;
use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::{MemoryModuleType, MemoryStatus, module_type as mem};
use crate::entity::ai::brain::tracker::PositionTracker;
use crate::world::World;

/// Vanilla `GoAndGiveItemsToTarget.CLOSE_ENOUGH_DISTANCE_TO_TARGET`.
const CLOSE_ENOUGH_DISTANCE_TO_TARGET: i32 = 3;
/// Vanilla `GoAndGiveItemsToTarget.tick`'s `distanceToTarget < 3.0` throw radius.
const THROW_DISTANCE: f64 = 3.0;

/// Vanilla `Function<LivingEntity, Optional<PositionTracker>>` deposit source.
///
/// Vanilla resolves this against the body's brain; Steel hands the behavior's
/// [`BehaviorContext`] instead, because the brain is already locked while a
/// behavior ticks.
pub type TargetPositionGetter = fn(&BehaviorContext<'_>) -> Option<Arc<dyn PositionTracker>>;
/// Vanilla `Predicate<E>` "still holding something to give" test.
pub type HasItemPredicate = fn(&SharedEntity) -> bool;
/// Vanilla `GoAndGiveItemsToTarget.ItemThrower<E>`.
pub type ItemThrower = fn(&World, &SharedEntity, DVec3);

/// Vanilla `GoAndGiveItemsToTarget<E extends LivingEntity>`: walks to the supplied
/// deposit tracker and, once within three blocks of it, calls the item thrower and
/// starts the cooldown memory.
pub struct GoAndGiveItemsToTarget {
    state: BehaviorState,
    target_position_getter: TargetPositionGetter,
    speed_modifier: f32,
    item_thrower: ItemThrower,
    cooldown_memory: &'static MemoryModuleType<i32>,
    cooldown_duration: i32,
    has_item_predicate: HasItemPredicate,
}

impl GoAndGiveItemsToTarget {
    /// Vanilla `new GoAndGiveItemsToTarget(Function targetPositionGetter, float
    /// speedModifier, int timeoutDuration, ItemThrower itemThrower,
    /// MemoryModuleType<Integer> cooldownMemory, int cooldownDuration,
    /// Predicate hasItemPredicate)`.
    #[must_use]
    pub fn new(
        target_position_getter: TargetPositionGetter,
        speed_modifier: f32,
        timeout_duration: i32,
        item_thrower: ItemThrower,
        cooldown_memory: &'static MemoryModuleType<i32>,
        cooldown_duration: i32,
        has_item_predicate: HasItemPredicate,
    ) -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (&mem::LOOK_TARGET, MemoryStatus::Registered),
                    (&mem::WALK_TARGET, MemoryStatus::Registered),
                    (cooldown_memory, MemoryStatus::Registered),
                ],
                timeout_duration,
                timeout_duration,
            ),
            target_position_getter,
            speed_modifier,
            item_thrower,
            cooldown_memory,
            cooldown_duration,
            has_item_predicate,
        }
    }

    /// Vanilla `GoAndGiveItemsToTarget.canThrowItemToTarget`.
    fn can_throw_item_to_target(&self, ctx: &BehaviorContext<'_>) -> bool {
        (self.has_item_predicate)(ctx.body) && (self.target_position_getter)(ctx).is_some()
    }
}

impl Behavior for GoAndGiveItemsToTarget {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        self.can_throw_item_to_target(ctx)
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        self.can_throw_item_to_target(ctx)
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        let Some(position_tracker) = (self.target_position_getter)(ctx) else {
            return;
        };

        utils::set_walk_and_look_target(
            ctx,
            position_tracker,
            self.speed_modifier,
            CLOSE_ENOUGH_DISTANCE_TO_TARGET,
        );
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        let Some(deposit_target) = (self.target_position_getter)(ctx) else {
            return;
        };

        let body = ctx.body;
        let deposit_position = deposit_target.current_position();
        let eye_position = DVec3::new(body.position().x, body.get_eye_y(), body.position().z);
        if eye_position.distance(deposit_position) >= THROW_DISTANCE {
            return;
        }

        (self.item_thrower)(ctx.level, body, deposit_position);
        ctx.set_memory(self.cooldown_memory, self.cooldown_duration);
    }
}
