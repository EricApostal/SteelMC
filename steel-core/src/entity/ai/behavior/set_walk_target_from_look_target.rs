//! Vanilla `SetWalkTargetFromLookTarget`.

use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::tracker::WalkTarget;

/// Vanilla `SetWalkTargetFromLookTarget`: turns the current look target into a walk
/// target.
pub struct SetWalkTargetFromLookTarget {
    speed_modifier: f32,
    close_enough_distance: i32,
}

impl SetWalkTargetFromLookTarget {
    /// Vanilla `SetWalkTargetFromLookTarget.create(speedModifier, closeEnoughDistance)`.
    #[must_use]
    pub fn create(speed_modifier: f32, close_enough_distance: i32) -> OneShot<Self> {
        OneShot::new(Self {
            speed_modifier,
            close_enough_distance,
        })
        .with_required_memories(vec![&mem::WALK_TARGET, &mem::LOOK_TARGET])
    }
}

impl Trigger for SetWalkTargetFromLookTarget {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if !ctx.check_memory(&mem::WALK_TARGET, MemoryStatus::ValueAbsent) {
            return false;
        }
        let Some(look_target) = ctx.get_memory(&mem::LOOK_TARGET) else {
            return false;
        };

        ctx.set_memory(
            &mem::WALK_TARGET,
            WalkTarget::new(look_target, self.speed_modifier, self.close_enough_distance),
        );
        true
    }
}
