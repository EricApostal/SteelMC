//! Behavior for vanilla conduit blocks (`net.minecraft.world.level.block.ConduitBlock`).
//!
//! Vanilla `ConduitBlock` is a `BaseEntityBlock` (`EntityBlock`): it owns a
//! [`ConduitBlockEntity`](crate::block_entity::entities::ConduitBlockEntity) and selects
//! `ConduitBlockEntity::serverTick` through `getTicker`. Steel expresses the same wiring with
//! [`BlockBehavior::new_block_entity`] and [`BlockBehavior::get_block_entity_ticker`]; the
//! entity's [`BlockEntity::tick`](crate::block_entity::BlockEntity::tick) performs the vanilla
//! `serverTick` work.
//!
//! The `Block.cube(6.0)` outline/collision shape and the `WATERLOGGED` default (`true`) come from
//! the extracted block data, so this behavior only owns placement, water ticking and the
//! block-entity wiring.
//!
//! Deviation: vanilla's `clientTick` (`animationTick` nautilus particles and the model's
//! `activeRotation`) is client-only; Steel has no client renderer (owner: client).

use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::blocks::{
    BlockRef,
    block_state_ext::BlockStateExt,
    properties::{BlockStateProperties, BoolProperty},
};
use steel_registry::vanilla_block_entity_types;
use steel_utils::{BlockPos, BlockStateId, Direction};

use crate::{
    behavior::{
        BlockBehavior, BlockPlaceContext,
        block::{BlockEntityCreation, schedule_water_tick_if_waterlogged},
    },
    block_entity::{BLOCK_ENTITIES, BlockEntityTicker},
    entity::ai::path::PathComputationType,
    fluid::{FluidStateExt as _, get_fluid_state},
    world::{ScheduledTickAccess, World},
};

/// Behavior for vanilla conduit blocks.
#[block_behavior]
pub struct ConduitBlock {
    block: BlockRef,
}

const WATERLOGGED: &BoolProperty = &BlockStateProperties::WATERLOGGED;

impl ConduitBlock {
    /// Creates a new conduit block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }
}

impl BlockBehavior for ConduitBlock {
    /// Vanilla `ConduitBlock.updateShape`: schedule the waterlogged water tick.
    fn update_shape(
        &self,
        state: BlockStateId,
        world: &dyn ScheduledTickAccess,
        pos: BlockPos,
        _direction: Direction,
        _neighbor_pos: BlockPos,
        _neighbor_state: BlockStateId,
    ) -> BlockStateId {
        schedule_water_tick_if_waterlogged(state, world, pos);

        state
    }

    /// Vanilla `ConduitBlock.getStateForPlacement`.
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        let replaced_fluid_state = get_fluid_state(context.world, context.place_pos());
        Some(self.block.default_state().set_value(
            WATERLOGGED,
            replaced_fluid_state.is_water() && replaced_fluid_state.is_full(),
        ))
    }

    /// Vanilla `ConduitBlock.isPathfindable` always returns `false`.
    fn is_pathfindable(
        &self,
        _state: BlockStateId,
        _computation_type: PathComputationType,
    ) -> bool {
        false
    }

    /// Vanilla `ConduitBlock.newBlockEntity`.
    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
            &vanilla_block_entity_types::CONDUIT,
            level,
            pos,
            state,
        ))
    }

    /// Vanilla `ConduitBlock.getTicker`: server side selects `ConduitBlockEntity.serverTick`.
    fn get_block_entity_ticker(
        &self,
        _world: &Arc<World>,
        _state: BlockStateId,
        block_entity_type: BlockEntityTypeRef,
    ) -> Option<BlockEntityTicker> {
        BlockEntityTicker::for_matching_entity_tick(
            block_entity_type,
            &vanilla_block_entity_types::CONDUIT,
        )
    }
}
