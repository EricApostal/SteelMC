//! Vanilla `BecomePassiveIfMemoryPresent`: pacify while another memory is set.

use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStatus};

/// Vanilla `BecomePassiveIfMemoryPresent`: sets `PACIFIED` for `pacifyDuration`
/// ticks and drops the current attack target whenever the pacifying memory is
/// present.
pub struct BecomePassiveIfMemoryPresent {
    pacifying_memory: MemoryModuleTypeRef,
    pacify_duration: i64,
}

impl BecomePassiveIfMemoryPresent {
    /// Vanilla `BecomePassiveIfMemoryPresent.create(MemoryModuleType<?>
    /// pacifyingMemory, int pacifyDuration)`.
    #[must_use]
    pub fn create(pacifying_memory: MemoryModuleTypeRef, pacify_duration: i32) -> OneShot<Self> {
        OneShot::new(Self {
            pacifying_memory,
            pacify_duration: i64::from(pacify_duration),
        })
        .with_required_memories(vec![
            &mem::ATTACK_TARGET,
            &mem::PACIFIED,
            pacifying_memory,
        ])
    }
}

impl Trigger for BecomePassiveIfMemoryPresent {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        // Vanilla entry conditions: `absent(PACIFIED)` and `present(pacifyingMemory)`.
        if ctx.has_memory_value(&mem::PACIFIED)
            || !ctx.check_memory(self.pacifying_memory, MemoryStatus::ValuePresent)
        {
            return false;
        }

        ctx.set_memory_with_expiry(&mem::PACIFIED, true, self.pacify_duration);
        ctx.erase_memory(&mem::ATTACK_TARGET);
        true
    }
}
