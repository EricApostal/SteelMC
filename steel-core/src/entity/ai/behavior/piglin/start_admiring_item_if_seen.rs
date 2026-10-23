//! Vanilla `StartAdmiringItemIfSeen`.

use super::ai;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::entities::ItemEntity;
use steel_utils::Downcast as _;

/// Vanilla `StartAdmiringItemIfSeen`: a loved item on the ground starts the
/// admire-item activity.
pub struct StartAdmiringItemIfSeen {
    admire_duration: i64,
}

impl StartAdmiringItemIfSeen {
    /// Vanilla `StartAdmiringItemIfSeen.create(int admireDuration)`.
    #[must_use]
    pub fn create(admire_duration: i32) -> OneShot<Self> {
        OneShot::new(Self {
            admire_duration: i64::from(admire_duration),
        })
        .with_required_memories(vec![
            &mem::NEAREST_VISIBLE_WANTED_ITEM,
            &mem::ADMIRING_ITEM,
            &mem::ADMIRING_DISABLED,
            &mem::DISABLE_WALK_TO_ADMIRE_ITEM,
        ])
    }
}

impl Trigger for StartAdmiringItemIfSeen {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if !ctx.check_memory(
            &mem::NEAREST_VISIBLE_WANTED_ITEM,
            MemoryStatus::ValuePresent,
        ) || ctx.has_memory_value(&mem::ADMIRING_ITEM)
            || ctx.has_memory_value(&mem::ADMIRING_DISABLED)
            || ctx.has_memory_value(&mem::DISABLE_WALK_TO_ADMIRE_ITEM)
        {
            return false;
        }

        let Some(nearest_item) = ctx.get_memory(&mem::NEAREST_VISIBLE_WANTED_ITEM) else {
            return false;
        };
        let Some(item_entity) = nearest_item.downcast_ref::<ItemEntity>() else {
            return false;
        };
        if !ai::is_loved_item(&item_entity.get_item()) {
            return false;
        }

        ctx.set_memory_with_expiry(&mem::ADMIRING_ITEM, true, self.admire_duration);
        true
    }
}
