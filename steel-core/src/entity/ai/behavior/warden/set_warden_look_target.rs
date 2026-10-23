//! Vanilla `SetWardenLookTarget`: look at the remembered target or disturbance.

use std::sync::Arc;

use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::tracker::{BlockPosTracker, PositionTracker};

/// Vanilla `SetWardenLookTarget.create`.
///
/// Vanilla prefers the `ROAR_TARGET` entity's block position and falls back to
/// the `DISTURBANCE_LOCATION`; the `ATTACK_TARGET` absence is tested explicitly
/// here rather than in a memory group.
pub struct SetWardenLookTarget;

impl SetWardenLookTarget {
    /// Vanilla `SetWardenLookTarget.create()`.
    #[must_use]
    pub fn create() -> OneShot<Self> {
        OneShot::new(Self).with_required_memories(vec![
            &mem::LOOK_TARGET,
            &mem::DISTURBANCE_LOCATION,
            &mem::ROAR_TARGET,
            &mem::ATTACK_TARGET,
        ])
    }
}

impl Trigger for SetWardenLookTarget {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if ctx.has_memory_value(&mem::ATTACK_TARGET) {
            return false;
        }

        let roar_pos = ctx
            .get_memory(&mem::ROAR_TARGET)
            .map(|target: SharedEntity| target.block_position());

        if let Some(pos) = roar_pos.or_else(|| ctx.get_memory(&mem::DISTURBANCE_LOCATION)) {
            let tracker: Arc<dyn PositionTracker> = Arc::new(BlockPosTracker::from_block_pos(pos));
            ctx.set_memory(&mem::LOOK_TARGET, tracker);
            true
        } else {
            false
        }
    }
}
