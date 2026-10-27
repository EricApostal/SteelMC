//! Vanilla `VillagerPanicTrigger`: react to remembered threats.

use crate::entity::ai::behavior::villager::villager;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::module_type as mem;

/// Vanilla `VillagerPanicTrigger`: the 3 agreeing villagers `spawnGolemIfNeeded` is
/// called with from the panic tick.
const PANIC_GOLEM_VOTES: usize = 3;

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
            // Vanilla `Villager.spawnGolemIfNeeded(level, gameTime, 3)` runs inline here,
            // but this behaviour executes inside the brain tick, which already holds the
            // villager's brain lock; the consensus helper locks the brains of every
            // participating villager, so calling it here self-deadlocks. Stage the
            // request instead — the villager's tick resolves it after the guard drops.
            villager.request_golem_spawn(PANIC_GOLEM_VOTES);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Weak};

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::*;
    use crate::entity::ai::behavior::villager::as_villager;
    use crate::entity::ai::brain::memory::MemoryStore;
    use crate::entity::entities::mobs::npc::villager::VillagerEntity;
    use crate::entity::{Mob, SharedEntity};
    use crate::test_support::fresh_test_world;
    use crate::world::World;

    /// Regression: this behaviour runs *inside* the brain tick, which already holds the
    /// villager's brain lock, and the golem consensus locks the brain of every
    /// participating villager. Calling it from `tick` therefore self-deadlocked the tick
    /// thread the moment a villager panicked (e.g. a player hits one).
    ///
    /// The guard below is held exactly as the brain tick holds it while a behaviour runs:
    /// before the fix this test hung, now the behaviour only stages the request.
    #[test]
    fn panic_tick_stages_the_golem_request_while_the_brain_lock_is_held() {
        init_vanilla_registry();
        let fixture = fresh_test_world("villager_panic_golem_request");
        let world = Arc::clone(&fixture.world);
        let villager: SharedEntity = Arc::new(VillagerEntity::new(
            &vanilla_entities::VILLAGER,
            1,
            DVec3::new(4.0, 64.0, 4.0),
            Weak::<World>::new(),
        ));
        let villager_ref = as_villager(&villager).expect("the body is a villager");

        let mut behavior = VillagerPanicTrigger::new();
        let mut memories = MemoryStore::new();

        let brain = Mob::brain(villager_ref).expect("villagers have a brain");
        let guard = brain.lock();

        let mut ctx = BehaviorContext::new(&mut memories, &world, &villager, 100);
        behavior.tick(&mut ctx);

        drop(guard);
        assert_eq!(
            villager_ref.take_golem_spawn_request(),
            Some(PANIC_GOLEM_VOTES),
            "the panic tick stages the vanilla 3-vote golem request for the villager tick"
        );
    }
}
