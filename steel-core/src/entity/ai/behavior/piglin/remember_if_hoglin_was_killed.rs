//! Vanilla `RememberIfHoglinWasKilled`.

use steel_registry::vanilla_entities;

use super::ai;
use crate::entity::LivingEntity as _;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;

/// Vanilla `RememberIfHoglinWasKilled`: after a hoglin kill, the piglin (and its
/// group) stops hunting for a while.
pub struct RememberIfHoglinWasKilled;

impl RememberIfHoglinWasKilled {
    /// Vanilla `RememberIfHoglinWasKilled.create()`.
    #[must_use]
    pub fn create() -> OneShot<Self> {
        OneShot::new(Self).with_required_memories(vec![&mem::ATTACK_TARGET, &mem::HUNTED_RECENTLY])
    }
}

impl Trigger for RememberIfHoglinWasKilled {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if !ctx.check_memory(&mem::ATTACK_TARGET, MemoryStatus::ValuePresent) {
            return false;
        }

        let Some(target) = ctx.get_memory(&mem::ATTACK_TARGET) else {
            return false;
        };

        if target.entity_type().key == vanilla_entities::HOGLIN.key
            && target
                .as_living_entity()
                .is_some_and(LivingEntity::is_dead_or_dying)
        {
            let duration = i64::from(ai::sample(ai::TIME_BETWEEN_HUNTS));
            ctx.set_memory_with_expiry(&mem::HUNTED_RECENTLY, true, duration);
        }

        true
    }
}
