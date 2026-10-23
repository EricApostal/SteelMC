//! Vanilla `Digging`: dig into the ground and despawn.

use steel_registry::entity_data::EntityPose;

use crate::entity::RemovalReason;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStatus};

/// Vanilla `Digging`: hold the `DIGGING` pose for the full duration, then remove
/// the body from the world.
pub struct Digging {
    state: BehaviorState,
}

impl Digging {
    /// Vanilla `new Digging(ticks)`, called with `WardenAi.DIGGING_DURATION`.
    #[must_use]
    pub fn new(ticks: i32) -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (
                        &mem::ATTACK_TARGET as MemoryModuleTypeRef,
                        MemoryStatus::ValueAbsent,
                    ),
                    (
                        &mem::WALK_TARGET as MemoryModuleTypeRef,
                        MemoryStatus::ValueAbsent,
                    ),
                ],
                ticks,
                ticks,
            ),
        }
    }
}

impl Behavior for Digging {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.body.removal_reason().is_none()
    }

    /// Vanilla `Entity.isInLiquid` is `isInWater() || isInLava()`.
    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.body.on_ground() || ctx.body.is_in_water() || ctx.body.is_in_lava()
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        if ctx.body.on_ground() {
            // Vanilla plays `SoundEvents.WARDEN_DIG` here; the AI layer has no
            // server audio path.
            ctx.body.set_pose(EntityPose::Digging);
        } else {
            // Vanilla plays `SoundEvents.WARDEN_AGITATED` and digests into an
            // immediate stop, which discards the body.
            self.stop(ctx);
        }
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        if ctx.body.removal_reason().is_none() {
            ctx.body.set_removed(RemovalReason::Discarded);
        }
    }
}
