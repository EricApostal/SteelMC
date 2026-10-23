//! `DriedGhastBlock` behavior (`net.minecraft.world.level.block.DriedGhastBlock`).
//!
//! A horizontal-directional block holding a 0-3 hydration level. Waterlogged states gain hydration
//! (playing `DRIED_GHAST_TRANSITION`) until hydrated, then spawn a ghastling; dry states lose
//! hydration. Steel has no happy-ghast entity yet, so the full level cycle is implemented and the
//! spawn itself is a reported dependency.

use std::sync::Arc;

use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{
    BlockStateProperties, BoolProperty, Direction, EnumProperty, IntProperty,
};
use steel_registry::sound_events;
use steel_registry::vanilla_entities;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId};

use crate::behavior::{BlockBehavior, BlockPlaceContext};
use crate::world::{ScheduledTickAccess, World};

/// Vanilla `DriedGhastBlock.MAX_HYDRATION_LEVEL`.
pub const MAX_HYDRATION_LEVEL: u8 = 3;
/// Vanilla `DriedGhastBlock.HYDRATION_TICK_DELAY`.
pub const HYDRATION_TICK_DELAY: i32 = 5_000;

const FACING: &EnumProperty<Direction> = &BlockStateProperties::HORIZONTAL_FACING;
const HYDRATION_LEVEL: &IntProperty = &BlockStateProperties::DRIED_GHAST_HYDRATION_LEVELS;
const WATERLOGGED: &BoolProperty = &BlockStateProperties::WATERLOGGED;

/// `DriedGhastBlock` behavior.
#[block_behavior]
pub struct DriedGhastBlock {
    block: BlockRef,
}

impl DriedGhastBlock {
    /// Creates a dried-ghast block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Vanilla `DriedGhastBlock.getHydrationLevel`.
    #[must_use]
    pub fn hydration_level(state: BlockStateId) -> u8 {
        state.get_value(HYDRATION_LEVEL)
    }

    /// Vanilla `DriedGhastBlock.isReadyToSpawn`.
    #[must_use]
    pub fn is_ready_to_spawn(state: BlockStateId) -> bool {
        Self::hydration_level(state) == MAX_HYDRATION_LEVEL
    }
}

impl BlockBehavior for DriedGhastBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(
            self.block
                .default_state()
                .set_value(FACING, context.horizontal_direction().opposite())
                .set_value(WATERLOGGED, context.is_water_source()),
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
        crate::behavior::block::schedule_water_tick_if_waterlogged(state, world, pos);
        state
    }

    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        if state.get_value(WATERLOGGED) {
            if !Self::is_ready_to_spawn(state) {
                world.play_block_sound(
                    &sound_events::BLOCK_DRIED_GHAST_TRANSITION,
                    pos,
                    1.0,
                    1.0,
                    None,
                );
                world.set_block(
                    pos,
                    state.set_value(HYDRATION_LEVEL, Self::hydration_level(state) + 1),
                    UpdateFlags::UPDATE_CLIENTS,
                );
                world.game_event(
                    &steel_registry::vanilla_game_events::BLOCK_CHANGE,
                    pos,
                    &crate::world::game_event::GameEventContext::new(None, Some(state)),
                );
            } else {
                world.remove_block(pos, false);
                world.play_block_sound(&sound_events::ENTITY_GHASTLING_SPAWN, pos, 1.0, 1.0, None);
                // TODO(Entities): spawn a baby `HAPPY_GHAST` with the block's yaw once the entity
                // lane registers a factory for `vanilla_entities::HAPPY_GHAST` (vanilla
                // `DriedGhastBlock.spawnGhastling`).
                let _ = &vanilla_entities::HAPPY_GHAST;
            }
            return;
        }

        let hydration = Self::hydration_level(state);
        if hydration > 0 {
            world.set_block(
                pos,
                state.set_value(HYDRATION_LEVEL, hydration - 1),
                UpdateFlags::UPDATE_CLIENTS,
            );
            world.game_event(
                &steel_registry::vanilla_game_events::BLOCK_CHANGE,
                pos,
                &crate::world::game_event::GameEventContext::new(None, Some(state)),
            );
        }
    }
}
