//! Vanilla `CatLieOnBlockGoal` (`goal/CatLieOnBlockGoal.java`): a tamed, awake cat walks to
//! a bed head and lies on it.
//!
//! Vanilla extends `MoveToBlockGoal` with `verticalSearchStart = -2`, `MOVE`+`JUMP` flags and
//! a 40-tick start interval. Steel builds it from the landed [`MoveToBlockGoal`] with the
//! same parameters, which already carry those flags and the start-interval hook.

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_utils::BlockPos;

use super::move_to_block::MoveToBlockGoal;
use super::reduced_tick_delay;
use super::selector::{Goal, GoalControls};
use crate::entity::{PathfinderMob, SharedEntity};
use crate::world::LevelReader;

/// Vanilla `CatLieOnBlockGoal(cat, speedModifier, searchRange)`:
/// `super(cat, speedModifier, searchRange, 6)`: the move-to-target vertical range is 6.
const VERTICAL_SEARCH_RANGE: i32 = 6;
/// Vanilla `CatLieOnBlockGoal.tick` interval before the goal may start again.
const NEXT_START_TICK: i32 = 40;

/// Vanilla `CatLieOnBlockGoal`.
pub struct CatLieOnBlockGoal {
    move_to_block: MoveToBlockGoal,
}

impl CatLieOnBlockGoal {
    /// Vanilla `new CatLieOnBlockGoal(cat, speedModifier, searchRange)`.
    #[must_use]
    pub fn new(speed_modifier: f64, search_range: i32) -> Self {
        Self {
            move_to_block: MoveToBlockGoal::with_vertical_search_range(
                speed_modifier,
                search_range,
                VERTICAL_SEARCH_RANGE,
                is_valid_target,
            )
            .with_vertical_search_start(-2)
            .with_next_start_tick(|_| reduced_tick_delay(NEXT_START_TICK)),
        }
    }
}

/// Vanilla `CatLieOnBlockGoal.isValidTarget`.
fn is_valid_target(level: &dyn LevelReader, pos: BlockPos) -> bool {
    level.get_block_state(pos.above()).is_air()
        && level
            .get_block_state(pos)
            .get_block()
            .has_tag(&BlockTag::CATS_CAN_LIE_ON)
}

impl Goal for CatLieOnBlockGoal {
    fn controls(&self) -> GoalControls {
        self.move_to_block.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(feline) = mob.as_feline() else {
            return false;
        };
        if !feline.is_tame() || feline.is_ordered_to_sit() || feline.is_lying() {
            return false;
        }

        self.move_to_block.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.move_to_block.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.move_to_block.start(mob);
        if let Some(feline) = mob.as_feline() {
            feline.set_in_sitting_pose(false);
        }
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.move_to_block.stop(mob);
        if let Some(feline) = mob.as_feline() {
            feline.set_lying(false);
        }
    }

    fn requires_update_every_tick(&self) -> bool {
        self.move_to_block.requires_update_every_tick()
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.move_to_block.tick(mob, entity);
        let Some(feline) = mob.as_feline() else {
            return;
        };

        feline.set_in_sitting_pose(false);
        if !self.move_to_block.is_reached_target() {
            feline.set_lying(false);
        } else if !feline.is_lying() {
            feline.set_lying(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cat_lie_on_block_goal_uses_the_move_to_block_controls() {
        let goal = CatLieOnBlockGoal::new(1.0, 8);

        assert_eq!(goal.controls(), GoalControls::MOVE | GoalControls::JUMP);
    }
}
