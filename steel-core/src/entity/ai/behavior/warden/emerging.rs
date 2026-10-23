//! Vanilla `Emerging`: rise out of the ground in the `EMERGING` pose.

use steel_registry::entity_data::EntityPose;

use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStatus};

/// Vanilla `Emerging`: hold the `EMERGING` pose for the full duration while
/// `IS_EMERGING` is remembered.
pub struct Emerging {
    state: BehaviorState,
}

impl Emerging {
    /// Vanilla `new Emerging(ticks)`, called with `WardenAi.EMERGE_DURATION`.
    #[must_use]
    pub fn new(ticks: i32) -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (
                        &mem::IS_EMERGING as MemoryModuleTypeRef,
                        MemoryStatus::ValuePresent,
                    ),
                    (
                        &mem::WALK_TARGET as MemoryModuleTypeRef,
                        MemoryStatus::ValueAbsent,
                    ),
                    (
                        &mem::LOOK_TARGET as MemoryModuleTypeRef,
                        MemoryStatus::Registered,
                    ),
                ],
                ticks,
                ticks,
            ),
        }
    }
}

impl Behavior for Emerging {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn can_still_use(&self, _ctx: &mut BehaviorContext<'_>) -> bool {
        true
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        // Vanilla plays `SoundEvents.WARDEN_EMERGE` here; the AI layer has no
        // server audio path.
        ctx.body.set_pose(EntityPose::Emerging);
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        if ctx.body.pose() == EntityPose::Emerging {
            ctx.body.set_pose(EntityPose::Standing);
        }
    }
}
