//! Vanilla `VillagerCalmDown`: forget a threat once it is gone.

use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;

/// Vanilla `VillagerCalmDown.SAFE_DISTANCE_FROM_DANGER`.
const SAFE_DISTANCE_FROM_DANGER: f64 = 36.0;

/// Vanilla `VillagerCalmDown`.
///
/// Vanilla also asks the brain to re-sample the schedule here; Steel behaviors only
/// receive the memory store, so that refresh runs centrally in `villager::tick`
/// (see `villager/brain.rs`) on the same memories.
pub struct VillagerCalmDown;

impl VillagerCalmDown {
    /// Vanilla `VillagerCalmDown.create`.
    #[must_use]
    pub fn create() -> OneShot<Self> {
        OneShot::new(Self).with_required_memories(vec![
            &mem::HURT_BY,
            &mem::HURT_BY_ENTITY,
            &mem::NEAREST_HOSTILE,
        ])
    }
}

impl Trigger for VillagerCalmDown {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        let body = ctx.body;
        let feel_scared = ctx.has_memory_value(&mem::HURT_BY)
            || ctx.has_memory_value(&mem::NEAREST_HOSTILE)
            || ctx.get_memory(&mem::HURT_BY_ENTITY).is_some_and(|entity| {
                body.position().distance_squared(entity.position())
                    <= SAFE_DISTANCE_FROM_DANGER * SAFE_DISTANCE_FROM_DANGER
            });

        if !feel_scared {
            ctx.erase_memory(&mem::HURT_BY);
            ctx.erase_memory(&mem::HURT_BY_ENTITY);
        }

        true
    }
}
