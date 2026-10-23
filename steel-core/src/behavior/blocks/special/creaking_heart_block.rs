//! `CreakingHeartBlock` behavior (`net.minecraft.world.level.block.CreakingHeartBlock`).
//!
//! The heart becomes dormant/awake only when both pale-oak logs along its axis match its own axis,
//! and it re-evaluates that on every neighbour shape update (scheduled one tick later). Ticking is
//! selected only while the state is not `uprooted`, exactly like vanilla's `getTicker`.
//!
//! TODO(Entities): `CreakingHeartBlockEntity` drives the linked `Creaking` mob (heart destruction,
//! protrusion, hit routing). Steel's entity lane has not landed `Creaking` yet; this block owns the
//! full block-state machine and the block entity persists the link.

use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{
    BlockStateProperties, BoolProperty, CreakingHeartState, Direction, EnumProperty,
};
use steel_registry::vanilla_block_entity_types;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_utils::{BlockPos, BlockStateId, axis::Axis};

use crate::behavior::{BlockBehavior, BlockEntityCreation, BlockPlaceContext};
use crate::block_entity::{BLOCK_ENTITIES, BlockEntityTicker};
use crate::world::{LevelReader, ScheduledTickAccess, World};

const AXIS: &EnumProperty<Axis> = &BlockStateProperties::AXIS;
const STATE: &EnumProperty<CreakingHeartState> = &BlockStateProperties::CREAKING_HEART_STATE;
const NATURAL: &BoolProperty = &BlockStateProperties::NATURAL;

/// `CreakingHeartBlock` behavior.
#[block_behavior]
pub struct CreakingHeartBlock {
    block: BlockRef,
}

impl CreakingHeartBlock {
    /// Creates a creaking-heart block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Vanilla `CreakingHeartBlock.hasRequiredLogs`: both neighbours along `axis` are
    /// `#pale_oak_logs` with the same axis as this block.
    #[must_use]
    pub fn has_required_logs(state: BlockStateId, world: &dyn LevelReader, pos: BlockPos) -> bool {
        let axis = state.get_value(AXIS);
        let directions = match axis {
            Axis::X => [Direction::East, Direction::West],
            Axis::Y => [Direction::Up, Direction::Down],
            Axis::Z => [Direction::North, Direction::South],
        };
        directions.iter().all(|direction| {
            let neighbour = world.get_block_state(pos.relative(*direction));
            neighbour.get_block().has_tag(&BlockTag::PALE_OAK_LOGS)
                && neighbour.try_get_value(&BlockStateProperties::AXIS) == Some(axis)
        })
    }

    /// Vanilla `CreakingHeartBlock.updateState`: an `uprooted` heart with valid logs becomes
    /// `awake` when the environment says the creaking is active, otherwise `dormant`.
    fn update_state(state: BlockStateId, world: &dyn LevelReader, pos: BlockPos) -> BlockStateId {
        if state.get_value(STATE) != CreakingHeartState::Uprooted
            || !Self::has_required_logs(state, world, pos)
        {
            return state;
        }
        state.set_value(STATE, CreakingHeartState::Dormant)
    }
}

impl BlockBehavior for CreakingHeartBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(
            self.block
                .default_state()
                .set_value(AXIS, context.clicked_face().get_axis()),
        )
    }

    fn update_shape(
        &self,
        state: BlockStateId,
        world: &dyn ScheduledTickAccess,
        pos: BlockPos,
        _direction: Direction,
        _neighbor_pos: BlockPos,
        _neighbor_state: BlockStateId,
    ) -> BlockStateId {
        // Vanilla always re-schedules a one-tick re-evaluation on any shape update.
        world.schedule_block_tick_default(pos, self.block, 1);
        state
    }

    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        let new_state = Self::update_state(state, world.as_ref(), pos);
        if new_state != state {
            world.set_block(pos, new_state, steel_utils::types::UpdateFlags::UPDATE_ALL);
        }
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
            &vanilla_block_entity_types::CREAKING_HEART,
            level,
            pos,
            state,
        ))
    }

    fn get_block_entity_ticker(
        &self,
        _world: &Arc<World>,
        state: BlockStateId,
        block_entity_type: BlockEntityTypeRef,
    ) -> Option<BlockEntityTicker> {
        // Vanilla returns a ticker only while the heart is not uprooted.
        if state.get_value(STATE) == CreakingHeartState::Uprooted {
            return None;
        }
        BlockEntityTicker::for_matching_entity_tick(
            block_entity_type,
            &vanilla_block_entity_types::CREAKING_HEART,
        )
    }

    fn has_analog_output_signal(&self, _state: BlockStateId) -> bool {
        false
    }
}
