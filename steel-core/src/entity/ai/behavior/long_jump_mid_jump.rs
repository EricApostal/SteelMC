//! Vanilla `LongJumpMidJump`: keep the long-jump pose while airborne, then land.

use glam::DVec3;
use steel_registry::entity_data::EntityPose;
use steel_utils::value_providers::UniformIntProvider;

use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStatus, module_type as mem};

/// Vanilla `LongJumpMidJump.TIME_OUT_DURATION`.
pub const TIME_OUT_DURATION: i32 = 100;

/// Vanilla `LongJumpMidJump`.
pub struct LongJumpMidJump {
    state: BehaviorState,
    time_between_long_jumps: UniformIntProvider,
}

impl LongJumpMidJump {
    /// Vanilla `LongJumpMidJump(UniformInt timeBetweenLongJumps, SoundEvent landingSound)`.
    ///
    /// Steel has no per-entity sound registry handle on the behavior layer yet, so
    /// the landing sound is not carried here; the pose/memory half is complete.
    #[must_use]
    pub fn new(time_between_long_jumps: UniformIntProvider) -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (
                        &mem::LOOK_TARGET as MemoryModuleTypeRef,
                        MemoryStatus::Registered,
                    ),
                    (
                        &mem::LONG_JUMP_MID_JUMP as MemoryModuleTypeRef,
                        MemoryStatus::ValuePresent,
                    ),
                ],
                TIME_OUT_DURATION,
                TIME_OUT_DURATION,
            ),
            time_between_long_jumps,
        }
    }

    /// Vanilla `timeBetweenLongJumps.sample(level.getRandom())`.
    fn sample_time_between_long_jumps(&self) -> i32 {
        let mut random =
            steel_utils::random::legacy_random::LegacyRandom::from_seed(rand::random());
        self.time_between_long_jumps.sample(&mut random)
    }
}

impl Behavior for LongJumpMidJump {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        !ctx.body.on_ground()
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        if let Some(living) = ctx.body.as_living_entity() {
            living.set_discard_friction(true);
        }
        ctx.body.set_pose(EntityPose::LongJumping);
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        if ctx.body.on_ground() {
            let velocity = ctx.body.velocity();
            ctx.body
                .set_velocity(DVec3::new(velocity.x * 0.1, velocity.y, velocity.z * 0.1));
        }

        if let Some(living) = ctx.body.as_living_entity() {
            living.set_discard_friction(false);
        }
        ctx.body.set_pose(EntityPose::Standing);
        ctx.erase_memory(&mem::LONG_JUMP_MID_JUMP);
        ctx.set_memory(
            &mem::LONG_JUMP_COOLDOWN_TICKS,
            self.sample_time_between_long_jumps(),
        );
    }
}
