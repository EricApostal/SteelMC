//! Vanilla `CatSitOnBlockGoal` (`goal/CatSitOnBlockGoal.java`): a tamed, not-sitting cat
//! walks to a chest, lit furnace or bed and sits on it.
//!
//! Vanilla extends `MoveToBlockGoal`; Steel configures the landed [`MoveToBlockGoal`] with
//! vanilla's search parameters and validation predicate instead of subclassing.
//!
//! Deliberate difference: vanilla's chest validation is
//! `ChestBlockEntity.getOpenCount(level, pos) < 1`. Steel reads the same number from the
//! block entity's [`ContainerViewers`] capability, which is what `getOpenCount` reports, so
//! the goal needs no chest-specific downcast.

use std::ptr;

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BedPart, BlockStateProperties};
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_blocks;
use steel_utils::BlockPos;

use super::move_to_block::MoveToBlockGoal;
use super::selector::{Goal, GoalControls};
use crate::entity::{PathfinderMob, SharedEntity};
use crate::world::LevelReader;

/// Vanilla `CatSitOnBlockGoal(cat, speedModifier)`: `super(cat, speedModifier, 8)`.
const SEARCH_RANGE: i32 = 8;

/// Vanilla `CatSitOnBlockGoal`.
pub struct CatSitOnBlockGoal {
    move_to_block: MoveToBlockGoal,
}

impl CatSitOnBlockGoal {
    /// Vanilla `new CatSitOnBlockGoal(cat, speedModifier)`.
    #[must_use]
    pub fn new(speed_modifier: f64) -> Self {
        Self {
            move_to_block: MoveToBlockGoal::new(speed_modifier, SEARCH_RANGE, is_valid_target),
        }
    }
}

/// Vanilla `CatSitOnBlockGoal.isValidTarget`.
fn is_valid_target(level: &dyn LevelReader, pos: BlockPos) -> bool {
    if !level.get_block_state(pos.above()).is_air() {
        return false;
    }

    let block_state = level.get_block_state(pos);
    let block = block_state.get_block();
    if !block.has_tag(&BlockTag::CATS_CAN_SIT_ON) {
        return false;
    }

    if ptr::eq(block, &vanilla_blocks::CHEST) {
        let Some(block_entity) = level.get_block_entity(pos) else {
            return true;
        };
        return block_entity
            .container_viewers()
            .is_none_or(|viewers| viewers.open_count() < 1);
    }

    if ptr::eq(block, &vanilla_blocks::FURNACE) {
        return block_state.get_value(&BlockStateProperties::LIT);
    }

    if block.has_tag(&BlockTag::BEDS) {
        return block_state.get_value(&BlockStateProperties::BED_PART) != BedPart::Head;
    }

    true
}

impl Goal for CatSitOnBlockGoal {
    fn controls(&self) -> GoalControls {
        self.move_to_block.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(feline) = mob.as_feline() else {
            return false;
        };
        if !feline.is_tame() || feline.is_ordered_to_sit() {
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
            feline.set_in_sitting_pose(false);
        }
    }

    fn requires_update_every_tick(&self) -> bool {
        self.move_to_block.requires_update_every_tick()
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.move_to_block.tick(mob, entity);
        if let Some(feline) = mob.as_feline() {
            feline.set_in_sitting_pose(self.move_to_block.is_reached_target());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cat_sit_on_block_goal_uses_the_move_to_block_controls() {
        let goal = CatSitOnBlockGoal::new(1.2);

        assert_eq!(goal.controls(), GoalControls::MOVE | GoalControls::JUMP);
    }
}
