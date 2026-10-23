//! Vanilla `ForceUnmount`: dismount before digging.

use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};

/// Vanilla `ForceUnmount`: the warden cannot dig while carrying a passenger.
pub struct ForceUnmount {
    state: BehaviorState,
}

impl ForceUnmount {
    /// Vanilla `new ForceUnmount()`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: BehaviorState::with_entry_condition(Vec::new()),
        }
    }
}

impl Default for ForceUnmount {
    fn default() -> Self {
        Self::new()
    }
}

impl Behavior for ForceUnmount {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.body.is_passenger()
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        ctx.body.stop_riding();
    }
}
