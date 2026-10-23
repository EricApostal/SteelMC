//! Vanilla `LookAtTargetSink`: aim the head at the remembered look target.

use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStatus, module_type as mem};
use crate::entity::ai::control::{DEFAULT_LOOK_X_MAX_ROT_ANGLE, DEFAULT_LOOK_Y_MAX_ROT_SPEED};

/// Vanilla `LookAtTargetSink`.
pub struct LookAtTargetSink {
    state: BehaviorState,
}

impl LookAtTargetSink {
    /// Vanilla `new LookAtTargetSink(minDuration, maxDuration)`.
    #[must_use]
    pub fn new(min_duration: i32, max_duration: i32) -> Self {
        Self {
            state: BehaviorState::new(
                vec![(
                    &mem::LOOK_TARGET as MemoryModuleTypeRef,
                    MemoryStatus::ValuePresent,
                )],
                min_duration,
                max_duration,
            ),
        }
    }
}

impl Behavior for LookAtTargetSink {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.get_memory(&mem::LOOK_TARGET)
            .is_some_and(|target| target.is_visible_by(ctx.memories()))
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        ctx.erase_memory(&mem::LOOK_TARGET);
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        let Some(target) = ctx.get_memory(&mem::LOOK_TARGET) else {
            return;
        };
        let Some(mob) = ctx.body.as_mob() else {
            return;
        };

        mob.mob_base().controls().lock().look_control.set_look_at(
            target.current_position(),
            DEFAULT_LOOK_Y_MAX_ROT_SPEED,
            DEFAULT_LOOK_X_MAX_ROT_ANGLE,
        );
    }
}
