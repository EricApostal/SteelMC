//! Vanilla `StopAdmiringIfItemTooFarAway`.

use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use steel_utils::types::InteractionHand;

use crate::entity::{Entity as _, LivingEntity as _};

/// Vanilla `StopAdmiringIfItemTooFarAway`: ends the admire activity once the wanted
/// item is gone or beyond `maxDistanceToItem`.
pub struct StopAdmiringIfItemTooFarAway {
    max_distance_to_item: f64,
}

impl StopAdmiringIfItemTooFarAway {
    /// Vanilla `StopAdmiringIfItemTooFarAway.create(int maxDistanceToItem)`.
    #[must_use]
    pub fn create(max_distance_to_item: i32) -> OneShot<Self> {
        OneShot::new(Self {
            max_distance_to_item: f64::from(max_distance_to_item),
        })
        .with_required_memories(vec![&mem::ADMIRING_ITEM, &mem::NEAREST_VISIBLE_WANTED_ITEM])
    }
}

impl Trigger for StopAdmiringIfItemTooFarAway {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if !ctx.check_memory(&mem::ADMIRING_ITEM, MemoryStatus::ValuePresent) {
            return false;
        }
        if ctx
            .body
            .as_living_entity()
            .is_some_and(|living| !living.get_item_in_hand(InteractionHand::OffHand).is_empty())
        {
            return false;
        }

        if let Some(nearest) = ctx.get_memory(&mem::NEAREST_VISIBLE_WANTED_ITEM)
            && ctx.body.position().distance_squared(nearest.position())
                < self.max_distance_to_item * self.max_distance_to_item
        {
            return false;
        }

        ctx.erase_memory(&mem::ADMIRING_ITEM);
        true
    }
}
