//! Vanilla `GoToWantedItem`: walk to the remembered wanted item.

use std::sync::Arc;

use super::utils;
use crate::entity::Mob;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::tracker::{EntityTracker, WalkTarget};

/// Vanilla `GoToWantedItem.create(speedModifier, interruptOngoingWalk, maxDistToWalk)`.
///
/// Vanilla only navigates: the actual collection happens in `Mob.aiStep`'s looting
/// loop, which Steel implements as `Mob::tick_looting` with the same
/// `wantsToPickUp`/`canPickUpLoot`/`mobGriefing` gates.
pub struct GoToWantedItem {
    speed_modifier: f32,
    interrupt_ongoing_walk: bool,
    max_dist_to_walk: f64,
}

impl GoToWantedItem {
    #[must_use]
    pub fn new(speed_modifier: f32, interrupt_ongoing_walk: bool, max_dist_to_walk: i32) -> Self {
        Self {
            speed_modifier,
            interrupt_ongoing_walk,
            max_dist_to_walk: f64::from(max_dist_to_walk),
        }
    }

    /// Vanilla `GoToWantedItem.create(speedModifier, interruptOngoingWalk, maxDistToWalk)`.
    #[must_use]
    pub fn create(
        speed_modifier: f32,
        interrupt_ongoing_walk: bool,
        max_dist_to_walk: i32,
    ) -> OneShot<Self> {
        OneShot::new(Self::new(
            speed_modifier,
            interrupt_ongoing_walk,
            max_dist_to_walk,
        ))
        .with_required_memories(vec![
            &mem::LOOK_TARGET,
            &mem::WALK_TARGET,
            &mem::NEAREST_VISIBLE_WANTED_ITEM,
            &mem::ITEM_PICKUP_COOLDOWN_TICKS,
        ])
    }
}

impl Trigger for GoToWantedItem {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        let walk_target_registered = ctx.has_memory_value(&mem::WALK_TARGET);
        if !self.interrupt_ongoing_walk && walk_target_registered {
            return false;
        }

        if ctx.get_memory(&mem::ITEM_PICKUP_COOLDOWN_TICKS).is_some() {
            return false;
        }

        let Some(item) = ctx.get_memory(&mem::NEAREST_VISIBLE_WANTED_ITEM) else {
            return false;
        };

        let body = ctx.body;
        if body.position().distance_squared(item.position())
            >= self.max_dist_to_walk * self.max_dist_to_walk
        {
            return false;
        }

        if !ctx
            .level
            .is_block_within_world_border(item.block_position())
        {
            return false;
        }

        if !body.as_mob().is_some_and(Mob::can_pick_up_loot) {
            return false;
        }

        utils::set_look_target(ctx, Arc::new(EntityTracker::new(item.clone(), true)));
        ctx.set_memory(
            &mem::WALK_TARGET,
            WalkTarget::new(
                Arc::new(EntityTracker::new(item, false)),
                self.speed_modifier,
                0,
            ),
        );
        true
    }
}
