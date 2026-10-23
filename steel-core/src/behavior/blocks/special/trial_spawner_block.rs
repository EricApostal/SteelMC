//! `TrialSpawnerBlock` behavior (`net.minecraft.world.level.block.TrialSpawnerBlock`).

use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{
    BlockStateProperties, BoolProperty, EnumProperty, TrialSpawnerState,
};
use steel_registry::vanilla_block_entity_types;
use steel_utils::{BlockPos, BlockStateId};

use crate::behavior::{BlockBehavior, BlockEntityCreation, BlockPlaceContext};
use crate::block_entity::{BLOCK_ENTITIES, BlockEntityTicker};
use crate::world::World;

const OMINOUS: &BoolProperty = &BlockStateProperties::OMINOUS;
const STATE: &EnumProperty<TrialSpawnerState> = &BlockStateProperties::TRIAL_SPAWNER_STATE;

/// `TrialSpawnerBlock` behavior.
#[block_behavior]
pub struct TrialSpawnerBlock {
    block: BlockRef,
}

impl TrialSpawnerBlock {
    /// Creates a trial spawner block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }
}

impl BlockBehavior for TrialSpawnerBlock {
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(
            self.block
                .default_state()
                .set_value(STATE, TrialSpawnerState::Inactive)
                .set_value(OMINOUS, false),
        )
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
            &vanilla_block_entity_types::TRIAL_SPAWNER,
            level,
            pos,
            state,
        ))
    }

    fn get_block_entity_ticker(
        &self,
        _world: &Arc<World>,
        _state: BlockStateId,
        block_entity_type: BlockEntityTypeRef,
    ) -> Option<BlockEntityTicker> {
        BlockEntityTicker::for_matching_entity_tick(
            block_entity_type,
            &vanilla_block_entity_types::TRIAL_SPAWNER,
        )
    }
}
