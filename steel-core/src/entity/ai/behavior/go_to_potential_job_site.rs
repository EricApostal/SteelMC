//! Vanilla `GoToPotentialJobSite`.

use super::poi::{self, PoiPredicate};
use super::utils;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;

/// Vanilla `GoToPotentialJobSite.TICKS_UNTIL_TIMEOUT`.
const TICKS_UNTIL_TIMEOUT: i32 = 1200;

/// Vanilla `GoToPotentialJobSite`: walks to the memorized potential job site.
///
/// Deviation: vanilla also gates `checkExtraStartConditions` on the brain's active
/// non-core activity being `IDLE`/`WORK`/`PLAY`. `BehaviorContext` cannot read the
/// active activity, so the gate is dropped here; the activity packages are the
/// closest reachable gate. Reported to Main.
pub struct GoToPotentialJobSite {
    state: BehaviorState,
    speed_modifier: f32,
}

impl GoToPotentialJobSite {
    /// Vanilla `GoToPotentialJobSite(speedModifier)`.
    #[must_use]
    pub fn new(speed_modifier: f32) -> Self {
        Self {
            state: BehaviorState::new(
                vec![(&mem::POTENTIAL_JOB_SITE, MemoryStatus::ValuePresent)],
                TICKS_UNTIL_TIMEOUT,
                TICKS_UNTIL_TIMEOUT,
            ),
            speed_modifier,
        }
    }
}

impl Behavior for GoToPotentialJobSite {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        if let Some(site) = ctx.get_memory(&mem::POTENTIAL_JOB_SITE) {
            let speed = self.speed_modifier;
            utils::set_walk_and_look_target_to_pos(ctx, site.pos, speed, 1);
        }
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.has_memory_value(&mem::POTENTIAL_JOB_SITE)
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        if let Some(site) = ctx.get_memory(&mem::POTENTIAL_JOB_SITE) {
            let level = ctx.level;
            if poi::exists(level, site.pos, PoiPredicate::Any) {
                poi::release(level, site.pos);
            }
            ctx.erase_memory(&mem::POTENTIAL_JOB_SITE);
        }
    }
}
