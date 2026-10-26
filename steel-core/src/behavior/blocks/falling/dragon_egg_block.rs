//! Vanilla `DragonEggBlock` behavior.
//!
//! Deviation: Vanilla `DragonEggBlock.getDustColor` is a client-side rendering
//! hint for the falling entity; Steel has no server-side hook for it.

use std::sync::Arc;

use rand::random_range;
use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::blocks::properties::Direction;
use steel_registry::items::item::BlockHitResult;
use steel_registry::level_events;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId};

use crate::behavior::{
    BlockBehavior, BlockPlaceContext, Fallable, InteractionResult, InventoryAccess,
};
use crate::entity::ai::path::PathComputationType;
use crate::player::Player;
use crate::world::{LevelAccessor, ScheduledTickAccess, World};

use super::FallingBlock;

const DRAGON_EGG_FALL_DELAY: i32 = 5;
const DRAGON_EGG_MAX_TELEPORTS: u32 = 1000;
const DRAGON_EGG_MAX_X: i32 = 16;
const DRAGON_EGG_MAX_Y: i32 = 8;
const DRAGON_EGG_MAX_Z: i32 = 16;

/// Dragon egg block behavior.
#[block_behavior]
pub struct DragonEggBlock {
    falling: FallingBlock,
}

impl DragonEggBlock {
    /// Creates the server-side behavior for the dragon egg
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self {
            falling: FallingBlock::new(block),
        }
    }

    /// Vanilla `BlockUtil.packDifferenceInPosition`.
    const fn pack_difference_in_position(
        from: BlockPos,
        to: BlockPos,
        x_radius: i32,
        y_radius: i32,
        z_radius: i32,
    ) -> i32 {
        let x_diff = to.x() - from.x();
        let y_diff = to.y() - from.y();
        let z_diff = to.z() - from.z();
        ((x_diff + x_radius) & 0xFF) << 16
            | ((y_diff + y_radius) & 0xFF) << 8
            | ((z_diff + z_radius) & 0xFF)
    }

    fn teleport(state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        for _ in 0..DRAGON_EGG_MAX_TELEPORTS {
            let x = random_range(0..DRAGON_EGG_MAX_X) - random_range(0..DRAGON_EGG_MAX_X);
            let y = random_range(0..DRAGON_EGG_MAX_Y) - random_range(0..DRAGON_EGG_MAX_Y);
            let z = random_range(0..DRAGON_EGG_MAX_Z) - random_range(0..DRAGON_EGG_MAX_Z);

            let new_pos = BlockPos::new(pos.x() + x, pos.y() + y, pos.z() + z);

            if !world.get_block_state(new_pos).is_air() {
                continue;
            }

            if world.get_block_state(new_pos.below()).is_air() {
                continue;
            }

            if !world.is_block_within_world_border(new_pos) {
                continue;
            }

            if world.is_outside_build_height(new_pos.y()) {
                continue;
            }

            // Vanilla sends the teleport particles from the original position
            // before moving the block.
            let packed_diff = Self::pack_difference_in_position(
                pos,
                new_pos,
                DRAGON_EGG_MAX_X,
                DRAGON_EGG_MAX_Y,
                DRAGON_EGG_MAX_Z,
            );
            world.level_event(
                level_events::PARTICLES_DRAGON_EGG_TELEPORT,
                pos,
                packed_diff,
                None,
            );
            world.set_block_state(new_pos, state, UpdateFlags::UPDATE_CLIENTS);
            world.remove_block(pos, false);
            break;
        }
    }
}

impl BlockBehavior for DragonEggBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        self.falling.get_state_for_placement(context)
    }

    fn on_place(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _old_state: BlockStateId,
        _moved_by_piston: bool,
    ) {
        let _ = world.schedule_block_tick_default(pos, self.falling.block(), DRAGON_EGG_FALL_DELAY);
    }

    fn update_shape(
        &self,
        state: BlockStateId,
        ticks: &dyn ScheduledTickAccess,
        pos: BlockPos,
        _direction: Direction,
        _neighbor_pos: BlockPos,
        _neighbor_state: BlockStateId,
    ) -> BlockStateId {
        let _ = ticks.schedule_block_tick_default(pos, self.falling.block(), DRAGON_EGG_FALL_DELAY);
        state
    }

    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        BlockBehavior::tick(&self.falling, state, world, pos);
    }

    fn use_without_item(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _player: &Player,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        Self::teleport(state, world, pos);
        InteractionResult::Success
    }

    fn attack(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos, _player: &Player) {
        Self::teleport(state, world, pos);
    }

    fn is_pathfindable(
        &self,
        _state: BlockStateId,
        _computation_type: PathComputationType,
    ) -> bool {
        false
    }

    fn as_fallable(&self) -> Option<&dyn Fallable> {
        Some(&self.falling)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_difference_matches_vanilla_bit_layout() {
        let origin = BlockPos::new(10, 64, -5);
        let target = BlockPos::new(13, 62, -8);

        assert_eq!(
            DragonEggBlock::pack_difference_in_position(origin, target, 16, 8, 16),
            (19 << 16) | (6 << 8) | 0x0d
        );
    }
}
