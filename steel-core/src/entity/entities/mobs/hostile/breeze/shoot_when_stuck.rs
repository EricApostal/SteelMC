//! Vanilla `monster/breeze/ShootWhenStuck`.
//!
//! The breeze cannot slide while it is a passenger, in water, or levitating, so
//! vanilla immediately arms the shoot memory and lets `Shoot` (activity priority 1)
//! take over on the next tick.

use steel_registry::vanilla_mob_effects;

use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::{MemoryStatus, module_type as mem};

/// Vanilla `ShootWhenStuck.start`'s `setMemoryWithExpiry(BREEZE_SHOOT, Unit, 60L)`.
const SHOOT_ARM_DURATION: i64 = 60;

/// Vanilla `ShootWhenStuck extends Behavior<Breeze>`.
pub struct ShootWhenStuck {
    state: BehaviorState,
}

impl ShootWhenStuck {
    /// Vanilla `new ShootWhenStuck()`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: BehaviorState::with_entry_condition(vec![
                (&mem::ATTACK_TARGET, MemoryStatus::ValuePresent),
                (&mem::BREEZE_JUMP_INHALING, MemoryStatus::ValueAbsent),
                (&mem::BREEZE_JUMP_TARGET, MemoryStatus::ValueAbsent),
                (&mem::WALK_TARGET, MemoryStatus::ValueAbsent),
                (&mem::BREEZE_SHOOT, MemoryStatus::ValueAbsent),
            ]),
        }
    }
}

impl Default for ShootWhenStuck {
    fn default() -> Self {
        Self::new()
    }
}

impl Behavior for ShootWhenStuck {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.body.is_passenger()
            || ctx.body.is_in_water()
            || ctx
                .body
                .as_living_entity()
                .is_some_and(|living| living.has_mob_effect(vanilla_mob_effects::LEVITATION))
    }

    fn can_still_use(&self, _ctx: &mut BehaviorContext<'_>) -> bool {
        // Vanilla `canStillUse` is `false`: the behavior is a one-tick arming step.
        false
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        ctx.set_memory_with_expiry(&mem::BREEZE_SHOOT, (), SHOOT_ARM_DURATION);
    }
}
