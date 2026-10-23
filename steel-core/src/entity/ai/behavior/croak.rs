//! Vanilla `Croak`.

use steel_registry::entity_data::EntityPose;

use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;

/// Vanilla `Croak.CROAK_TICKS`.
const CROAK_TICKS: i32 = 60;

/// Vanilla `Croak.TIME_OUT_DURATION`.
const TIME_OUT_DURATION: i32 = 100;

/// Vanilla `Croak`: keep the frog in the `CROAKING` pose for 60 ticks.
pub struct Croak {
    state: BehaviorState,
    croak_counter: i32,
}

impl Croak {
    /// Vanilla `new Croak()`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: BehaviorState::new(
                vec![(&mem::WALK_TARGET, MemoryStatus::ValueAbsent)],
                TIME_OUT_DURATION,
                TIME_OUT_DURATION,
            ),
            croak_counter: 0,
        }
    }
}

impl Default for Croak {
    fn default() -> Self {
        Self::new()
    }
}

impl Behavior for Croak {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.body.pose() == EntityPose::Standing
    }

    fn can_still_use(&self, _ctx: &mut BehaviorContext<'_>) -> bool {
        self.croak_counter < CROAK_TICKS
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        if !ctx.body.is_in_water() {
            ctx.body.set_pose(EntityPose::Croaking);
            self.croak_counter = 0;
        }
    }

    fn tick(&mut self, _ctx: &mut BehaviorContext<'_>) {
        self.croak_counter += 1;
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        ctx.body.set_pose(EntityPose::Standing);
    }
}
