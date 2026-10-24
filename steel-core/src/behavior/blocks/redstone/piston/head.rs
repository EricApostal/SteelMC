//! Vanilla piston-head behavior.

use std::sync::Arc;

use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{
    BlockStateProperties, BoolProperty, Direction, EnumProperty, PistonType,
};
use steel_registry::item_stack::ItemStack;
use steel_registry::{vanilla_blocks, vanilla_items};
use steel_utils::{BlockPos, BlockStateId};

use crate::behavior::{BlockBehavior, BlockPlaceContext};
use crate::block_entity::SharedBlockEntity;
use crate::entity::ai::path::PathComputationType;
use crate::player::Player;
use crate::world::{LevelReader, ScheduledTickAccess, World};

#[block_behavior]
/// Vanilla piston-head block.
pub struct PistonHeadBlock;

const EXTENDED: &BoolProperty = &BlockStateProperties::EXTENDED;
const FACING: &EnumProperty<Direction> = &BlockStateProperties::FACING;
const PISTON_TYPE: &EnumProperty<PistonType> = &BlockStateProperties::PISTON_TYPE;
impl PistonHeadBlock {
    /// Creates piston-head behavior.
    #[must_use]
    pub const fn new(_block: BlockRef) -> Self {
        Self
    }

    fn is_fitting_base(arm_state: BlockStateId, potential_base: BlockStateId) -> bool {
        let base_block = match arm_state.get_value(PISTON_TYPE) {
            PistonType::Normal => &vanilla_blocks::PISTON,
            PistonType::Sticky => &vanilla_blocks::STICKY_PISTON,
        };
        potential_base.get_block() == base_block
            && potential_base.get_value(EXTENDED)
            && potential_base.get_value(FACING) == arm_state.get_value(FACING)
    }
}

impl BlockBehavior for PistonHeadBlock {
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        // Vanilla's piston head has no placement state (no item exists for it).
        None
    }

    fn player_will_destroy(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
    ) -> BlockStateId {
        if player.has_infinite_materials() {
            let base_pos = pos.relative(state.get_value(FACING).opposite());
            if Self::is_fitting_base(state, world.get_block_state(base_pos)) {
                world.destroy_block(base_pos, false);
            }
        }
        state
    }

    fn affect_neighbors_after_removal(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _moved_by_piston: bool,
    ) {
        let base_pos = pos.relative(state.get_value(FACING).opposite());
        if Self::is_fitting_base(state, world.get_block_state(base_pos)) {
            world.destroy_block(base_pos, true);
        }
    }

    fn update_shape(
        &self,
        state: BlockStateId,
        world: &dyn ScheduledTickAccess,
        pos: BlockPos,
        direction: Direction,
        _neighbor_pos: BlockPos,
        _neighbor_state: BlockStateId,
    ) -> BlockStateId {
        if direction.opposite() == state.get_value(FACING) && !self.can_survive(state, world, pos) {
            vanilla_blocks::AIR.default_state()
        } else {
            state
        }
    }

    fn can_survive(&self, state: BlockStateId, world: &dyn LevelReader, pos: BlockPos) -> bool {
        let facing = state.get_value(FACING);
        let base = world.get_block_state(pos.relative(facing.opposite()));
        Self::is_fitting_base(state, base)
            || (base.get_block() == &vanilla_blocks::MOVING_PISTON
                && base.get_value(FACING) == facing)
    }

    fn handle_neighbor_changed(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        source_block: BlockRef,
        _moved_by_piston: bool,
    ) {
        if self.can_survive(state, world.as_ref(), pos) {
            let base_pos = pos.relative(state.get_value(FACING).opposite());
            world.neighbor_changed(base_pos, source_block);
        }
    }

    fn get_clone_item_stack(
        &self,
        _block: BlockRef,
        state: BlockStateId,
        _block_entity: Option<SharedBlockEntity>,
        _include_data: bool,
    ) -> Option<ItemStack> {
        Some(ItemStack::new(match state.get_value(PISTON_TYPE) {
            PistonType::Normal => &vanilla_items::PISTON,
            PistonType::Sticky => &vanilla_items::STICKY_PISTON,
        }))
    }

    fn is_pathfindable(&self, _state: BlockStateId, _type: PathComputationType) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;
    use steel_utils::{ChunkPos, types::UpdateFlags};

    use super::*;
    use crate::behavior::init_behaviors;
    use crate::test_support::{fresh_test_world, insert_ready_full_chunk};

    /// Extended `PISTON` base facing east plus the matching normal piston head.
    fn head_and_base_states() -> (BlockStateId, BlockStateId) {
        (
            vanilla_blocks::PISTON_HEAD
                .default_state()
                .set_value(FACING, Direction::East)
                .set_value(PISTON_TYPE, PistonType::Normal),
            vanilla_blocks::PISTON
                .default_state()
                .set_value(FACING, Direction::East)
                .set_value(EXTENDED, true),
        )
    }

    #[test]
    fn head_survives_only_on_a_fitting_base() {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world("piston_head_survival");
        let world = &world_fixture.world;
        let head_pos = BlockPos::new(8, 64, 8);
        let base_pos = head_pos.west();
        let _holder = insert_ready_full_chunk(world, ChunkPos::from_block_pos(head_pos));
        let (head_state, base_state) = head_and_base_states();
        let behavior = PistonHeadBlock::new(&vanilla_blocks::PISTON_HEAD);
        let air = vanilla_blocks::AIR.default_state();

        assert!(world.set_block(head_pos, head_state, UpdateFlags::UPDATE_NONE));
        assert!(world.set_block(base_pos, base_state, UpdateFlags::UPDATE_NONE));
        assert!(behavior.can_survive(head_state, world.as_ref(), head_pos));
        assert_eq!(
            behavior.update_shape(
                head_state,
                world,
                head_pos,
                Direction::West,
                base_pos,
                base_state,
            ),
            head_state
        );

        // Losing the base drops the head through `updateShape`.
        assert!(world.set_block(base_pos, air, UpdateFlags::UPDATE_NONE));
        assert!(!behavior.can_survive(head_state, world.as_ref(), head_pos));
        assert_eq!(
            behavior.update_shape(head_state, world, head_pos, Direction::West, base_pos, air),
            air
        );
    }

    #[test]
    fn removing_the_head_destroys_its_fitting_base() {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world("piston_head_removal");
        let world = &world_fixture.world;
        let head_pos = BlockPos::new(8, 64, 8);
        let base_pos = head_pos.west();
        let _holder = insert_ready_full_chunk(world, ChunkPos::from_block_pos(head_pos));
        let (head_state, base_state) = head_and_base_states();

        assert!(world.set_block(head_pos, head_state, UpdateFlags::UPDATE_NONE));
        assert!(world.set_block(base_pos, base_state, UpdateFlags::UPDATE_NONE));

        assert!(world.remove_block(head_pos, false));

        assert!(world.get_block_state(head_pos).is_air());
        assert!(world.get_block_state(base_pos).is_air());
    }
}
