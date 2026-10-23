//! Vanilla `CelebrateVillagersSurvivedRaid`.

use super::villager;
use crate::entity::ai::behavior::move_to_sky_seeing_spot::has_no_blocks_above;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::raid::RaidSnapshot;

/// Vanilla `CelebrateVillagersSurvivedRaid`: on raid victory, a villager under the
/// open sky celebrates.
pub struct CelebrateVillagersSurvivedRaid {
    state: BehaviorState,
    current_raid: Option<RaidSnapshot>,
}

impl CelebrateVillagersSurvivedRaid {
    /// Vanilla `CelebrateVillagersSurvivedRaid(minDuration, maxDuration)`.
    #[must_use]
    pub fn new(min_duration: i32, max_duration: i32) -> Self {
        Self {
            state: BehaviorState::new(Vec::new(), min_duration, max_duration),
            current_raid: None,
        }
    }
}

impl Behavior for CelebrateVillagersSurvivedRaid {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        let pos = ctx.body.block_position();
        let Some(raid) = ctx.level.get_raid_at(pos) else {
            return false;
        };

        raid.is_victory() && has_no_blocks_above(ctx.level, ctx.body, pos)
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        self.current_raid = ctx.level.get_raid_at(ctx.body.block_position());
    }

    fn can_still_use(&self, _ctx: &mut BehaviorContext<'_>) -> bool {
        self.current_raid.is_some_and(|raid| !raid.is_stopped())
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        self.current_raid = None;
        if let Some(mob) = ctx.body.as_mob()
            && let Some(brain) = mob.brain()
        {
            brain.lock().update_activity_from_schedule(ctx.level);
        }
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        if rand::random_range(0..100) == 0
            && let Some(villager) = villager::as_villager(ctx.body)
        {
            villager.play_celebrate_sound();
        }

        // Vanilla rolls 1/200 here and spawns a `FireworkRocketEntity`; Steel has no
        // projectile-spawn plumbing on `World` for behavior code. Reported to Main.
    }
}
