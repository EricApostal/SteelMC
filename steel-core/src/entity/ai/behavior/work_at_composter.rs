//! Vanilla `WorkAtComposter`.

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::vanilla_blocks;

use super::villager;
use super::work_at_poi::WorkAtPoi;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::module_type as mem;

/// Vanilla `WorkAtComposter`: a farmer composts seeds and bakes wheat into bread at a
/// composter work station.
///
/// Deviation (foundation gaps): vanilla `useWorkstation` calls `makeBread` and
/// `compostItems`, which mutate the villager's `SimpleContainer` and the composter
/// block state. `BehaviorContext` exposes neither a mutable villager inventory nor a
/// `World` block mutation, so only the shared `WorkAtPoi` work half runs. Reported to
/// Main.
pub struct WorkAtComposter(WorkAtPoi);

impl WorkAtComposter {
    /// Vanilla `WorkAtComposter()` (registered for the farmer profession).
    #[must_use]
    pub fn new() -> Self {
        Self(WorkAtPoi::new())
    }
}

impl Default for WorkAtComposter {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkAtComposter {
    /// Vanilla `WorkAtComposter.useWorkstation`.
    #[expect(
        clippy::unused_self,
        reason = "kept as a method to mirror vanilla `WorkAtComposter.useWorkstation`"
    )]
    fn use_workstation(&self, ctx: &mut BehaviorContext<'_>) {
        let Some(site) = ctx.get_memory(&mem::JOB_SITE) else {
            return;
        };
        if ctx.level.get_block_state(site.pos).get_block().key == vanilla_blocks::COMPOSTER.key {
            // Vanilla `makeBread` (wheat -> bread, max 3) and `compostItems` (compostable
            // seed insertion, `ComposterBlock.LEVEL == 8` extraction) need mutable
            // inventory and block access; unavailable here.
        }
    }
}

impl Behavior for WorkAtComposter {
    fn state(&self) -> &BehaviorState {
        self.0.state()
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        self.0.state_mut()
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        self.0.check_extra_start_conditions(ctx)
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        self.0.start_common(ctx);
        self.use_workstation(ctx);
        if let Some(v) = villager::villager(ctx.level, ctx.body)
            && v.should_restock()
        {
            v.restock();
        }
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        self.0.can_still_use(ctx)
    }
}
