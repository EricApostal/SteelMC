//! Vanilla `WorkAtPoi` and `WorkAtComposter`.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use super::villager;
use crate::entity::ai::brain::behavior::{
    Behavior, BehaviorContext, BehaviorState, DEFAULT_DURATION,
};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::tracker::BlockPosTracker;

/// Vanilla `WorkAtPoi.CHECK_COOLDOWN`.
const CHECK_COOLDOWN: i64 = 300;
/// Vanilla `WorkAtPoi` work reach, the `1.73` passed to `closerToCenterThan`.
const WORK_DISTANCE: f64 = 1.73;

/// Vanilla `WorkAtPoi`: at the job site, remember the work tick and play the work
/// sound. `useWorkstation` is the subclass hook.
pub struct WorkAtPoi {
    state: BehaviorState,
    last_check: AtomicI64,
}

impl WorkAtPoi {
    /// Vanilla `WorkAtPoi()`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (&mem::JOB_SITE, MemoryStatus::ValuePresent),
                    (&mem::LOOK_TARGET, MemoryStatus::Registered),
                ],
                DEFAULT_DURATION,
                DEFAULT_DURATION,
            ),
            last_check: AtomicI64::new(0),
        }
    }

    /// Vanilla `WorkAtPoi.start` without the `useWorkstation` hook.
    #[expect(
        clippy::unused_self,
        reason = "kept as a method to mirror vanilla `WorkAtPoi.start`"
    )]
    pub(super) fn start_common(&self, ctx: &mut BehaviorContext<'_>) {
        ctx.set_memory(&mem::LAST_WORKED_AT_POI, ctx.timestamp);
        if let Some(site) = ctx.get_memory(&mem::JOB_SITE) {
            ctx.set_memory(
                &mem::LOOK_TARGET,
                Arc::new(BlockPosTracker::from_block_pos(site.pos)),
            );
        }
        if let Some(v) = villager::villager(ctx.level, ctx.body) {
            v.play_work_sound();
        }
    }

    /// Vanilla `WorkAtPoi.useWorkstation`: a no-op for plain job sites.
    #[expect(
        clippy::unused_self,
        reason = "kept as a method to mirror vanilla `WorkAtPoi.useWorkstation`"
    )]
    const fn use_workstation(&self, _ctx: &mut BehaviorContext<'_>) {}
}

impl Default for WorkAtPoi {
    fn default() -> Self {
        Self::new()
    }
}

impl Behavior for WorkAtPoi {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        if ctx.timestamp - self.last_check.load(Ordering::Relaxed) < CHECK_COOLDOWN {
            return false;
        }
        if rand::random::<bool>() {
            return false;
        }
        self.last_check.store(ctx.timestamp, Ordering::Relaxed);

        at_job_site(ctx)
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        self.start_common(ctx);
        self.use_workstation(ctx);
        // Vanilla `Villager.shouldRestock`/`restock` follow the workstation use.
        if let Some(v) = villager::villager(ctx.level, ctx.body)
            && v.should_restock()
        {
            v.restock();
        }
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        at_job_site(ctx)
    }
}

/// Vanilla `WorkAtPoi.canStillUse`'s target test.
pub(super) fn at_job_site(ctx: &BehaviorContext<'_>) -> bool {
    ctx.get_memory(&mem::JOB_SITE).is_some_and(|site| {
        site.dimension == ctx.level.key
            && site
                .pos
                .closer_to_center_than(ctx.body.position(), WORK_DISTANCE)
    })
}
