//! End portal frame block implementation.
//!
//! Vanilla `EndPortalFrameBlock` has no `useItemOn`: eye insertion and the
//! 3x3 portal-completion scan live in `EnderEyeItem.useOn`, mirrored by
//! [`crate::behavior::EnderEyeItem`].

use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::blocks::properties::{
    BlockStateProperties, BoolProperty, Direction, EnumProperty,
};
use steel_utils::{BlockPos, BlockStateId};

use crate::behavior::block::BlockBehavior;
use crate::behavior::blocks::redstone::{MAX_REDSTONE_SIGNAL, MIN_REDSTONE_SIGNAL};
use crate::behavior::context::BlockPlaceContext;
use crate::entity::ai::path::PathComputationType;
use crate::world::LevelReader;

/// Behavior for end portal frame blocks.
#[block_behavior]
pub struct EndPortalFrameBlock {
    block: BlockRef,
}

const EYE: &BoolProperty = &BlockStateProperties::EYE;
const HORIZONTAL_FACING: &EnumProperty<Direction> = &BlockStateProperties::HORIZONTAL_FACING;

impl EndPortalFrameBlock {
    /// Creates a new end portal frame block behavior for the given block.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    const fn analog_output_signal(has_eye: bool) -> i32 {
        if has_eye {
            MAX_REDSTONE_SIGNAL
        } else {
            MIN_REDSTONE_SIGNAL
        }
    }
}

impl BlockBehavior for EndPortalFrameBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(
            self.block
                .default_state()
                .set_value(HORIZONTAL_FACING, context.horizontal_direction().opposite())
                .set_value(EYE, false),
        )
    }

    fn has_analog_output_signal(&self, _state: BlockStateId) -> bool {
        true
    }

    fn get_analog_output_signal(
        &self,
        state: BlockStateId,
        _world: &dyn LevelReader,
        _pos: BlockPos,
        _direction: Direction,
    ) -> i32 {
        Self::analog_output_signal(state.get_value(EYE))
    }

    fn is_pathfindable(
        &self,
        _state: BlockStateId,
        _computation_type: PathComputationType,
    ) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::blocks::block_state_ext::BlockStateExt;
    use steel_registry::blocks::properties::{BlockStateProperties, BoolProperty};
    use steel_registry::{init_vanilla_registry, vanilla_blocks};

    use super::{EndPortalFrameBlock, MAX_REDSTONE_SIGNAL};

    const EYE: &BoolProperty = &BlockStateProperties::EYE;

    #[test]
    fn end_portal_frame_analog_output_depends_on_eye() {
        init_vanilla_registry();

        let empty = vanilla_blocks::END_PORTAL_FRAME.default_state();
        let filled = empty.set_value(EYE, true);

        assert_eq!(
            EndPortalFrameBlock::analog_output_signal(empty.get_value(EYE)),
            0
        );
        assert_eq!(
            EndPortalFrameBlock::analog_output_signal(filled.get_value(EYE)),
            MAX_REDSTONE_SIGNAL
        );
    }
}
