//! Vanilla `VillagerPanicTrigger`: react to remembered threats.

use crate::entity::ai::behavior::villager::villager;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::module_type as mem;

/// Vanilla `VillagerPanicTrigger`.
pub struct VillagerPanicTrigger {
    state: BehaviorState,
}

impl VillagerPanicTrigger {
    /// Vanilla `new VillagerPanicTrigger()`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: BehaviorState::with_entry_condition(Vec::new()),
        }
    }

    /// Vanilla `VillagerPanicTrigger.isHurt`.
    #[must_use]
    pub fn is_hurt(ctx: &BehaviorContext<'_>) -> bool {
        ctx.has_memory_value(&mem::HURT_BY)
    }

    /// Vanilla `VillagerPanicTrigger.hasHostile`.
    #[must_use]
    pub fn has_hostile(ctx: &BehaviorContext<'_>) -> bool {
        ctx.has_memory_value(&mem::NEAREST_HOSTILE)
    }
}

impl Default for VillagerPanicTrigger {
    fn default() -> Self {
        Self::new()
    }
}

impl Behavior for VillagerPanicTrigger {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        Self::is_hurt(ctx) || Self::has_hostile(ctx)
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        if !Self::is_hurt(ctx) && !Self::has_hostile(ctx) {
            return;
        }

        // Vanilla only clears these when `PANIC` is not already active. The active
        // activity is not readable here, so the clear is unconditional at panic
        // onset; the `PANIC` switch itself is applied centrally in `villager::tick`.
        ctx.erase_memory(&mem::PATH);
        ctx.erase_memory(&mem::WALK_TARGET);
        ctx.erase_memory(&mem::LOOK_TARGET);
        ctx.erase_memory(&mem::BREED_TARGET);
        ctx.erase_memory(&mem::INTERACTION_TARGET);
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        if ctx.timestamp % 100 != 0 {
            return;
        }

        if let Some(villager) = villager(ctx.level, ctx.body) {
            villager.spawn_golem_if_needed(3);
        }
    }
}
