//! Copper chest behavior.
//!
//! Vanilla `CopperChestBlock` is a `ChestBlock` whose double-chest partner may be
//! any block in `minecraft:copper_chests` (not just the identical block) and
//! which normalizes a freshly placed pair to the least oxidized variant.
//! `WeatheringCopperChestBlock` adds oxidation, skipping the right half and any
//! chest that still has viewers.

use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_registry::REGISTRY;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{
    BlockStateProperties, BoolProperty, ChestType, EnumProperty,
};
use steel_registry::vanilla_block_entity_types;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_custom_stats;
use steel_utils::{BlockPos, BlockStateId, Direction};

use crate::behavior::InventoryAccess;
use crate::behavior::block::{
    BlockBehavior, BlockEntityCreation, schedule_water_tick_if_waterlogged,
};
use crate::behavior::blocks::building::{WeatherState, WeatheringCopper};
use crate::behavior::blocks::container::chest_block::{
    analog_signal, connected_direction, open_menu,
};
use crate::behavior::context::{BlockHitResult, BlockPlaceContext, InteractionResult};
use crate::behavior::waxables::get_normal_from_waxed_variant;
use crate::behavior::weathering::get_weather_state;
use crate::block_entity::BLOCK_ENTITIES;
use crate::block_entity::entities::ChestBlockEntity;
use crate::entity::ai::path::PathComputationType;
use crate::player::Player;
use crate::world::{LevelReader, ScheduledTickAccess, World};

const FACING: &EnumProperty<Direction> = &BlockStateProperties::FACING;
const CHEST_TYPE: &EnumProperty<ChestType> = &BlockStateProperties::CHEST_TYPE;
const WATERLOGGED: &BoolProperty = &BlockStateProperties::WATERLOGGED;

/// Copper chest behaviour.
///
/// Used by the waxed variants, which never oxidize. See
/// [`WeatheringCopperChestBlock`] for the unwaxed ones.
#[block_behavior]
pub struct CopperChestBlock {
    block: BlockRef,
}

/// Weathering copper chest behaviour.
#[block_behavior]
pub struct WeatheringCopperChestBlock {
    block: BlockRef,
    weathering: WeatheringCopper,
}

impl CopperChestBlock {
    /// Creates a new copper chest behaviour.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }
}

impl WeatheringCopperChestBlock {
    /// Creates a new weathering copper chest behaviour.
    #[must_use]
    pub fn new(block: BlockRef) -> Self {
        Self {
            block,
            weathering: WeatheringCopper::new(
                get_weather_state(block).unwrap_or(WeatherState::Unaffected),
            ),
        }
    }
}

/// Vanilla `ChestBlock.chestCanConnectTo` for copper chests: any block in the
/// `minecraft:copper_chests` tag that carries a `CHEST_TYPE` property.
fn chest_can_connect_to(state: BlockStateId) -> bool {
    state.get_block().has_tag(&BlockTag::COPPER_CHESTS) && state.try_get_value(CHEST_TYPE).is_some()
}

/// Vanilla `ChestBlock.candidatePartnerFacing`.
fn candidate_partner_facing(
    world: &Arc<World>,
    pos: BlockPos,
    direction: Direction,
) -> Option<Direction> {
    let neighbor_state = world.get_block_state(direction.relative(pos));
    if !chest_can_connect_to(neighbor_state)
        || neighbor_state.get_value(CHEST_TYPE) != ChestType::Single
    {
        return None;
    }
    Some(neighbor_state.get_value(FACING))
}

/// Vanilla `ChestBlock.getChestType`.
fn chest_type_for(world: &Arc<World>, pos: BlockPos, facing: Direction) -> ChestType {
    if candidate_partner_facing(world, pos, facing.rotate_y_clockwise()) == Some(facing) {
        ChestType::Left
    } else if candidate_partner_facing(world, pos, facing.rotate_y_counter_clockwise())
        == Some(facing)
    {
        ChestType::Right
    } else {
        ChestType::Single
    }
}

/// Vanilla `CopperChestBlock.getStateForPlacement`.
fn state_for_placement(block: BlockRef, context: &BlockPlaceContext<'_>) -> BlockStateId {
    let mut facing = context.horizontal_direction().opposite();
    let mut chest_type = ChestType::Single;

    if context.is_secondary_use_active()
        && context.clicked_face().is_horizontal()
        && let Some(neighbor_facing) = candidate_partner_facing(
            context.world,
            context.place_pos(),
            context.clicked_face().opposite(),
        )
        && neighbor_facing.axis() != context.clicked_face().axis()
    {
        facing = neighbor_facing;
        chest_type = if facing.rotate_y_counter_clockwise() == context.clicked_face().opposite() {
            ChestType::Right
        } else {
            ChestType::Left
        };
    }

    if chest_type == ChestType::Single && !context.is_secondary_use_active() {
        chest_type = chest_type_for(context.world, context.place_pos(), facing);
    }

    let state = block
        .default_state()
        .set_value(FACING, facing)
        .set_value(CHEST_TYPE, chest_type)
        .set_value(WATERLOGGED, context.is_water_source());

    least_oxidized_connected_chest(state, context.world, context.place_pos())
}

/// Vanilla `CopperChestBlock.getLeastOxidizedChestOfConnectedBlocks`.
fn least_oxidized_connected_chest(
    state: BlockStateId,
    world: &Arc<World>,
    pos: BlockPos,
) -> BlockStateId {
    if state.get_value(CHEST_TYPE) == ChestType::Single {
        return state;
    }

    let connected_pos = connected_direction(state).relative(pos);
    let connected_state = world.get_block_state(connected_pos);
    if !chest_can_connect_to(connected_state) {
        return state;
    }

    let own_waxed = get_normal_from_waxed_variant(state.get_block()).is_some();
    let connected_waxed = get_normal_from_waxed_variant(connected_state.get_block()).is_some();

    let mut updated_state = state;
    let mut connected_predicted_state = connected_state;
    if own_waxed != connected_waxed {
        if let Some(unwaxed) = get_normal_from_waxed_variant(state.get_block()) {
            updated_state = REGISTRY
                .blocks
                .copy_matching_properties(updated_state, unwaxed);
        }
        if let Some(unwaxed) = get_normal_from_waxed_variant(connected_state.get_block()) {
            connected_predicted_state = REGISTRY
                .blocks
                .copy_matching_properties(connected_predicted_state, unwaxed);
        }
    }

    let own_age = get_weather_state(state.get_block()).unwrap_or(WeatherState::Unaffected);
    let connected_age =
        get_weather_state(connected_state.get_block()).unwrap_or(WeatherState::Unaffected);
    let least_oxidized_block = if own_age <= connected_age {
        updated_state.get_block()
    } else {
        connected_predicted_state.get_block()
    };

    REGISTRY
        .blocks
        .copy_matching_properties(updated_state, least_oxidized_block)
}

/// Vanilla `ChestBlock.updateShape` plus the `CopperChestBlock` override that
/// adopts the connected half's block when the pair connects.
fn update_shape(
    state: BlockStateId,
    direction: Direction,
    neighbor_state: BlockStateId,
) -> BlockStateId {
    let mut state = state;
    if chest_can_connect_to(neighbor_state) && direction.is_horizontal() {
        let neighbor_type = neighbor_state.get_value(CHEST_TYPE);
        if state.get_value(CHEST_TYPE) == ChestType::Single
            && neighbor_type != ChestType::Single
            && state.get_value(FACING) == neighbor_state.get_value(FACING)
            && connected_direction(neighbor_state) == direction.opposite()
        {
            let opposite_type = match neighbor_type {
                ChestType::Left => ChestType::Right,
                ChestType::Right => ChestType::Left,
                ChestType::Single => ChestType::Single,
            };
            state = state.set_value(CHEST_TYPE, opposite_type);
        }
    } else if connected_direction(state) == direction {
        state = state.set_value(CHEST_TYPE, ChestType::Single);
    }

    if chest_can_connect_to(neighbor_state)
        && state.get_value(CHEST_TYPE) != ChestType::Single
        && connected_direction(state) == direction
    {
        return neighbor_state.with_properties_of(state);
    }

    state
}

fn place_new_block_entity(
    level: Weak<World>,
    pos: BlockPos,
    state: BlockStateId,
) -> BlockEntityCreation {
    BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
        &vanilla_block_entity_types::CHEST,
        level,
        pos,
        state,
    ))
}

impl BlockBehavior for CopperChestBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(state_for_placement(self.block, context))
    }

    fn update_shape(
        &self,
        state: BlockStateId,
        world: &dyn ScheduledTickAccess,
        pos: BlockPos,
        direction: Direction,
        _neighbor_pos: BlockPos,
        neighbor_state: BlockStateId,
    ) -> BlockStateId {
        schedule_water_tick_if_waterlogged(state, world, pos);
        update_shape(state, direction, neighbor_state)
    }

    fn use_without_item(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        open_menu(state, world, pos, player, &vanilla_custom_stats::OPEN_CHEST)
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        place_new_block_entity(level, pos, state)
    }

    fn tick(&self, _state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        // Vanilla `ChestBlock.tick` drives `ChestBlockEntity.recheckOpen`.
        if let Some(entity) = world.get_block_entity(pos)
            && let Some(viewers) = entity.container_viewers()
        {
            viewers.recheck_open();
        }
    }

    fn has_analog_output_signal(&self, _state: BlockStateId) -> bool {
        true
    }

    fn get_analog_output_signal(
        &self,
        state: BlockStateId,
        world: &dyn LevelReader,
        pos: BlockPos,
        _direction: Direction,
    ) -> i32 {
        analog_signal(state, world, pos)
    }

    fn is_pathfindable(
        &self,
        _state: BlockStateId,
        _computation_type: PathComputationType,
    ) -> bool {
        false
    }

    fn trigger_event(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        param_a: i32,
        param_b: i32,
    ) -> bool {
        let Some(block_entity) = world.get_block_entity(pos) else {
            return false;
        };
        block_entity.trigger_event(param_a, param_b)
    }
}

impl BlockBehavior for WeatheringCopperChestBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(state_for_placement(self.block, context))
    }

    fn update_shape(
        &self,
        state: BlockStateId,
        world: &dyn ScheduledTickAccess,
        pos: BlockPos,
        direction: Direction,
        _neighbor_pos: BlockPos,
        neighbor_state: BlockStateId,
    ) -> BlockStateId {
        schedule_water_tick_if_waterlogged(state, world, pos);
        update_shape(state, direction, neighbor_state)
    }

    fn use_without_item(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        open_menu(state, world, pos, player, &vanilla_custom_stats::OPEN_CHEST)
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        place_new_block_entity(level, pos, state)
    }

    fn tick(&self, _state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        if let Some(entity) = world.get_block_entity(pos)
            && let Some(viewers) = entity.container_viewers()
        {
            viewers.recheck_open();
        }
    }

    /// Vanilla `WeatheringCopperChestBlock.randomTick`: skip the right half and
    /// any chest that still has viewers.
    fn random_tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        if state.get_value(CHEST_TYPE) == ChestType::Right {
            return;
        }
        if ChestBlockEntity::open_count(world.as_ref(), pos) != 0 {
            return;
        }
        self.weathering.change_over_time(state, world, pos);
    }

    fn has_analog_output_signal(&self, _state: BlockStateId) -> bool {
        true
    }

    fn get_analog_output_signal(
        &self,
        state: BlockStateId,
        world: &dyn LevelReader,
        pos: BlockPos,
        _direction: Direction,
    ) -> i32 {
        analog_signal(state, world, pos)
    }

    fn is_pathfindable(
        &self,
        _state: BlockStateId,
        _computation_type: PathComputationType,
    ) -> bool {
        false
    }

    fn trigger_event(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        param_a: i32,
        param_b: i32,
    ) -> bool {
        let Some(block_entity) = world.get_block_entity(pos) else {
            return false;
        };
        block_entity.trigger_event(param_a, param_b)
    }
}
