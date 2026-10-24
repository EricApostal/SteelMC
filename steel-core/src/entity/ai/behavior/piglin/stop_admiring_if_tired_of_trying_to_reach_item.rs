//! Vanilla `StopAdmiringIfTiredOfTryingToReachItem`.

use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use steel_utils::types::InteractionHand;

/// Vanilla `StopAdmiringIfTiredOfTryingToReachItem`: gives up on an unreachable
/// wanted item and disables walking to admire items for a while.
pub struct StopAdmiringIfTiredOfTryingToReachItem {
    max_time_to_reach_item: i32,
    disable_time: i64,
}

impl StopAdmiringIfTiredOfTryingToReachItem {
    /// Vanilla `StopAdmiringIfTiredOfTryingToReachItem.create(int maxTimeToReachItem,
    /// int disableTime)`.
    #[must_use]
    pub fn create(max_time_to_reach_item: i32, disable_time: i32) -> OneShot<Self> {
        OneShot::new(Self {
            max_time_to_reach_item,
            disable_time: i64::from(disable_time),
        })
        .with_required_memories(vec![
            &mem::ADMIRING_ITEM,
            &mem::NEAREST_VISIBLE_WANTED_ITEM,
            &mem::TIME_TRYING_TO_REACH_ADMIRE_ITEM,
            &mem::DISABLE_WALK_TO_ADMIRE_ITEM,
        ])
    }
}

impl Trigger for StopAdmiringIfTiredOfTryingToReachItem {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if !ctx.check_memory(&mem::ADMIRING_ITEM, MemoryStatus::ValuePresent)
            || !ctx.check_memory(
                &mem::NEAREST_VISIBLE_WANTED_ITEM,
                MemoryStatus::ValuePresent,
            )
        {
            return false;
        }
        if ctx
            .body
            .as_living_entity()
            .is_some_and(|living| !living.get_item_in_hand(InteractionHand::OffHand).is_empty())
        {
            return false;
        }

        match ctx.get_memory(&mem::TIME_TRYING_TO_REACH_ADMIRE_ITEM) {
            None => ctx.set_memory(&mem::TIME_TRYING_TO_REACH_ADMIRE_ITEM, 0),
            Some(time) if time > self.max_time_to_reach_item => {
                ctx.erase_memory(&mem::ADMIRING_ITEM);
                ctx.erase_memory(&mem::TIME_TRYING_TO_REACH_ADMIRE_ITEM);
                ctx.set_memory_with_expiry(
                    &mem::DISABLE_WALK_TO_ADMIRE_ITEM,
                    true,
                    self.disable_time,
                );
            }
            Some(time) => ctx.set_memory(&mem::TIME_TRYING_TO_REACH_ADMIRE_ITEM, time + 1),
        }

        true
    }
}
