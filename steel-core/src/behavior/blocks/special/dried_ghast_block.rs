//! `DriedGhastBlock` behavior (`net.minecraft.world.level.block.DriedGhastBlock`).
//!
//! A horizontal-directional block holding a 0-3 hydration level. Waterlogged states gain hydration
//! (playing `DRIED_GHAST_TRANSITION`) until hydrated, then spawn a ghastling; dry states lose
//! hydration. Hydration ticks are scheduled from `random_tick` like vanilla's `randomTick`.
//!
//! Deviation: vanilla `DriedGhastBlock.animateTick` emits client-local ambient sounds and
//! `WHITE_SMOKE`/`HAPPY_VILLAGER` particles; Steel has no client block-animation hook.
//! Deviation: the fully hydrated state spawns its ghastling only once Steel registers a
//! `minecraft:happy_ghast` entity factory (owner: entity lane); until then the block stays in
//! place instead of being removed without a mob.

use std::sync::Arc;

use glam::DVec3;
use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{
    BlockStateProperties, BoolProperty, Direction, EnumProperty, IntProperty,
};
use steel_registry::fluid::FluidState;
use steel_registry::sound_events;
use steel_registry::vanilla_entities;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId};

use crate::behavior::block::place_simple_waterlogged_liquid;
use crate::behavior::{BlockBehavior, BlockPlaceContext, PlacementSource};
use crate::entity::ai::path::PathComputationType;
use crate::entity::{ENTITIES, add_spawned_entity, create_entity_instance};
use crate::world::{LevelAccessor, ScheduledTickAccess, World};

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

    /// Vanilla `DriedGhastBlock.spawnGhastling`: remove the block and spawn a baby happy ghast
    /// facing the block's `FACING`, then play `GHASTLING_SPAWN`.
    ///
    /// Vanilla creates the entity with `EntitySpawnReason.BREEDING` and plays the sound only when
    /// creation succeeded, so a missing factory leaves the block untouched here.
    fn spawn_ghastling(world: &Arc<World>, pos: BlockPos, state: BlockStateId) {
        if !ENTITIES.has_factory(&vanilla_entities::HAPPY_GHAST) {
            return;
        }

        let rotation = state.get_value(FACING).to_yaw();
        let spawn_at = DVec3::new(
            f64::from(pos.x()) + 0.5,
            f64::from(pos.y()),
            f64::from(pos.z()) + 0.5,
        );
        let Ok(ghastling) = create_entity_instance(world, &vanilla_entities::HAPPY_GHAST, spawn_at)
        else {
            return;
        };

        if let Some(ageable) = ghastling.as_ageable_mob() {
            ageable.set_baby(true);
        }
        if let Some(living) = ghastling.as_living_entity() {
            living.set_y_head_rot(rotation);
        }
        ghastling.snap_to(spawn_at, rotation, 0.0);
        let _ = add_spawned_entity(world, ghastling);

        world.remove_block(pos, false);
        world.play_block_sound(&sound_events::ENTITY_GHASTLING_SPAWN, pos, 1.0, 1.0, None);
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

    fn set_placed_by(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _source: &PlacementSource<'_>,
    ) {
        // Vanilla `DriedGhastBlock.setPlacedBy` announces the placed block, choosing the
        // underwater variant when the state is waterlogged.
        let sound = if state.get_value(WATERLOGGED) {
            &sound_events::BLOCK_DRIED_GHAST_PLACE_IN_WATER
        } else {
            &sound_events::BLOCK_DRIED_GHAST_PLACE
        };
        world.play_block_sound(sound, pos, 1.0, 1.0, None);
    }

    fn place_liquid(
        &self,
        level: &dyn LevelAccessor,
        pos: BlockPos,
        state: BlockStateId,
        fluid_state: FluidState,
    ) -> bool {
        // Vanilla `DriedGhastBlock.placeLiquid`: the shared waterlogged placement plus
        // `DRIED_GHAST_PLACE_IN_WATER`.
        if !place_simple_waterlogged_liquid(level, pos, state, fluid_state) {
            return false;
        }
        level.play_block_sound(
            &sound_events::BLOCK_DRIED_GHAST_PLACE_IN_WATER,
            pos,
            1.0,
            1.0,
            None,
        );
        true
    }

    fn is_pathfindable(
        &self,
        _state: BlockStateId,
        _computation_type: PathComputationType,
    ) -> bool {
        false
    }

    fn random_tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        // Vanilla `DriedGhastBlock.randomTick`.
        if (state.get_value(WATERLOGGED) || Self::hydration_level(state) > 0)
            && !world.has_scheduled_block_tick(pos, self.block)
        {
            world.schedule_block_tick_default(pos, self.block, HYDRATION_TICK_DELAY);
        }
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
                Self::spawn_ghastling(world, pos, state);
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
