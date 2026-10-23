//! Vanilla `DoNothing`: run out the behavior clock with no actions.

use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};

/// Vanilla `DoNothing`.
pub struct DoNothing {
    state: BehaviorState,
}

impl DoNothing {
    /// Vanilla `new DoNothing(minDuration, maxDuration)`.
    #[must_use]
    pub fn new(min_duration: i32, max_duration: i32) -> Self {
        Self {
            state: BehaviorState::new(Vec::new(), min_duration, max_duration),
        }
    }
}

impl Behavior for DoNothing {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        !self.state.timed_out(ctx.timestamp)
    }
}
