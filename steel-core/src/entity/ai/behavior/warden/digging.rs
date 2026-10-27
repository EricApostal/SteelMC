//! Vanilla `Digging`: dig into the ground and despawn.

use steel_registry::entity_data::EntityPose;
use steel_registry::sound_events;

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
            // Vanilla `Digging.start`: `setPose(DIGGING)` then `WARDEN_DIG` at volume 5.
            ctx.body.set_pose(EntityPose::Digging);
            ctx.body
                .play_sound(&sound_events::ENTITY_WARDEN_DIG, 5.0, 1.0);
        } else {
            // Vanilla plays `WARDEN_AGITATED` at volume 5 and digests into an
            // immediate stop, which discards the body.
            ctx.body
                .play_sound(&sound_events::ENTITY_WARDEN_AGITATED, 5.0, 1.0);
            self.stop(ctx);
        }
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        if ctx.body.removal_reason().is_none() {
            ctx.body.set_removed(RemovalReason::Discarded);
        }
    }
}
