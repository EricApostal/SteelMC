//! Vanilla `CountDownCooldownTicks`: tick down an int cooldown memory to zero.

use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::{MemoryModuleType, MemoryStatus};

/// Vanilla `CountDownCooldownTicks.timedOut` returns `false` unconditionally.
/// `BehaviorState` derives its timeout from a duration, so the largest duration
/// stands in for "never".
const NEVER_TIMES_OUT: i32 = i32::MAX;

/// Vanilla `CountDownCooldownTicks extends Behavior<LivingEntity>`: ticks down
/// the remembered cooldown until it reaches zero, erasing the memory on stop.
pub struct CountDownCooldownTicks {
    state: BehaviorState,
    cooldown_ticks: &'static MemoryModuleType<i32>,
}

impl CountDownCooldownTicks {
    /// Vanilla `new CountDownCooldownTicks(MemoryModuleType<Integer> cooldownTicks)`.
    #[must_use]
    pub fn new(cooldown_ticks: &'static MemoryModuleType<i32>) -> Self {
        Self {
            state: BehaviorState::new(
                vec![(cooldown_ticks, MemoryStatus::ValuePresent)],
                NEVER_TIMES_OUT,
                NEVER_TIMES_OUT,
            ),
            cooldown_ticks,
        }
    }
}

impl Behavior for CountDownCooldownTicks {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.get_memory(self.cooldown_ticks)
            .is_some_and(|ticks| ticks > 0)
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        if let Some(ticks) = ctx.get_memory(self.cooldown_ticks) {
            ctx.set_memory(self.cooldown_ticks, ticks - 1);
        }
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        ctx.erase_memory(self.cooldown_ticks);
    }
}
