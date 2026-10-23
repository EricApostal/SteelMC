//! Vanilla `EraseMemoryIf`: drop a memory when the body matches a predicate.

use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStatus};

/// Vanilla `EraseMemoryIf.create(Predicate predicate, MemoryModuleType memoryType)`.
///
/// Vanilla passes the body to the predicate; Steel passes the [`BehaviorContext`]
/// so predicates can read other memories (the body's brain lock is already held
/// while behaviors run).
pub struct EraseMemoryIf {
    predicate: fn(&mut BehaviorContext<'_>) -> bool,
    memory_type: MemoryModuleTypeRef,
}

impl EraseMemoryIf {
    /// Vanilla `EraseMemoryIf.create(Predicate, MemoryModuleType)`.
    #[must_use]
    pub fn create(
        predicate: fn(&mut BehaviorContext<'_>) -> bool,
        memory_type: MemoryModuleTypeRef,
    ) -> OneShot<Self> {
        OneShot::new(Self {
            predicate,
            memory_type,
        })
        .with_required_memories(vec![memory_type])
    }
}

impl Trigger for EraseMemoryIf {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        // Vanilla entry condition: `present(memoryType)`.
        if !ctx.check_memory(self.memory_type, MemoryStatus::ValuePresent) {
            return false;
        }

        if !(self.predicate)(ctx) {
            return false;
        }

        ctx.memories_mut().erase_dyn(self.memory_type);
        true
    }
}
