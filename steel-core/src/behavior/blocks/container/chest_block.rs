//! Chest and trapped-chest block behavior.
//!
//! Vanilla's `ChestBlock` picks a facing and a `ChestType` (single/left/right)
//! from the neighbor on placement and re-evaluates the type whenever a
//! neighbor changes. Right-clicking opens a 3-row single-chest menu or a 6-row
//! double-chest menu built from the two halves' containers.

use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::blocks::properties::{BlockStateProperties, ChestType, Direction};
use steel_registry::stat::custom::CustomStatRef;
use steel_registry::{vanilla_block_entity_types, vanilla_custom_stats};
use steel_utils::{BlockPos, BlockStateId, translations};
use text_components::TextComponent;

use crate::behavior::InventoryAccess;
use crate::behavior::block::{
    BlockBehavior, BlockEntityCreation, schedule_water_tick_if_waterlogged,
};
use crate::behavior::context::{BlockHitResult, BlockPlaceContext, InteractionResult};
use crate::block_entity::BLOCK_ENTITIES;
use crate::block_entity::entities::ChestBlockEntity;
use crate::entity::ai::path::PathComputationType;
use crate::inventory::container::{Container, calculate_redstone_signal_from_container};
use crate::inventory::lock::{ContainerLockGuard, ContainerRef};
use crate::inventory::menu::kinds::{chest, double_chest};
use crate::player::Player;
use crate::world::{
    LevelReader, ScheduledTickAccess, SignalQueryContext, World, is_redstone_conductor,
};

const FACING: &steel_registry::blocks::properties::EnumProperty<Direction> =
    &BlockStateProperties::FACING;
const CHEST_TYPE: &steel_registry::blocks::properties::EnumProperty<ChestType> =
    &BlockStateProperties::CHEST_TYPE;
const WATERLOGGED: &steel_registry::blocks::properties::BoolProperty =
    &BlockStateProperties::WATERLOGGED;

/// Behavior for chest blocks.
///
/// Forms double chests with same-block neighbors that face the same way.
#[block_behavior]
pub struct ChestBlock {
    block: BlockRef,
}

/// Behavior for trapped chest blocks.
///
/// Placement and menu behavior match [`ChestBlock`]; trapped chests additionally
/// emit redstone power while players hold them open.
#[block_behavior]
pub struct TrappedChestBlock {
    block: BlockRef,
}

impl ChestBlock {
    /// Creates a new chest block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }
}

impl TrappedChestBlock {
    /// Creates a new trapped chest block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }
}

/// Vanilla `ChestBlock.getConnectedDirection`: the direction of the other half.
pub(crate) fn connected_direction(state: BlockStateId) -> Direction {
    let facing = state.get_value(FACING);
    if state.get_value(CHEST_TYPE) == ChestType::Left {
        facing.rotate_y_clockwise()
    } else {
        facing.rotate_y_counter_clockwise()
    }
}

/// Vanilla `ChestBlock.candidatePartnerFacing`: the facing of the would-be
/// partner, or `None` when the neighbor cannot combine with this block.
fn candidate_partner_facing(
    world: &Arc<World>,
    block: BlockRef,
    pos: BlockPos,
    direction: Direction,
) -> Option<Direction> {
    let neighbor_pos = direction.relative(pos);
    let neighbor_state = world.get_block_state(neighbor_pos);
    if neighbor_state.get_block() != block {
        return None;
    }
    if neighbor_state.get_value(CHEST_TYPE) != ChestType::Single {
        return None;
    }
    Some(neighbor_state.get_value(FACING))
}

/// Vanilla `ChestBlock.getChestType`.
fn chest_type_for(
    world: &Arc<World>,
    block: BlockRef,
    pos: BlockPos,
    facing: Direction,
) -> ChestType {
    // A clockwise neighbor forms the left half of this chest (`ChestType.LEFT`
    // connects clockwise of facing, i.e. the neighbor points this way when
    // placed to the right of the player).
    if candidate_partner_facing(world, block, pos, facing.rotate_y_clockwise()) == Some(facing) {
        ChestType::Left
    } else if candidate_partner_facing(world, block, pos, facing.rotate_y_counter_clockwise())
        == Some(facing)
    {
        ChestType::Right
    } else {
        ChestType::Single
    }
}

fn state_for_placement(block: BlockRef, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
    let mut facing = context.horizontal_direction().opposite();
    let mut chest_type = ChestType::Single;

    // Vanilla secondary-use placement against a horizontal face borrows the
    // neighbor's facing, exactly like `ChestBlock.getStateForPlacement`.
    if context.is_secondary_use_active() && context.clicked_face().is_horizontal() {
        let neighbor_facing = candidate_partner_facing(
            context.world,
            block,
            context.place_pos(),
            context.clicked_face().opposite(),
        );
        if let Some(neighbor_facing) = neighbor_facing
            && neighbor_facing.axis() != context.clicked_face().axis()
        {
            facing = neighbor_facing;
            chest_type = if facing.rotate_y_counter_clockwise() == context.clicked_face().opposite()
            {
                ChestType::Right
            } else {
                ChestType::Left
            };
        }
    }

    if chest_type == ChestType::Single && !context.is_secondary_use_active() {
        chest_type = chest_type_for(context.world, block, context.place_pos(), facing);
    }

    Some(
        block
            .default_state()
            .set_value(FACING, facing)
            .set_value(CHEST_TYPE, chest_type)
            .set_value(WATERLOGGED, context.is_water_source()),
    )
}

fn update_shape(
    state: BlockStateId,
    direction: Direction,
    neighbor_state: BlockStateId,
    own_block: BlockRef,
) -> BlockStateId {
    // Vanilla `ChestBlock.updateShape` / `chestCanConnectTo`.
    let connects = neighbor_state.get_block() == own_block;
    let mut state = state;
    if connects && direction.is_horizontal() {
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

    state
}

/// Vanilla `ChestBlock.isChestBlockedAt`.
///
/// Steel has no cat entity yet, so only the "solid block above" half of
/// vanilla's check exists here; a sitting cat does not currently block a chest.
pub(crate) fn is_chest_blocked_at(world: &dyn LevelReader, pos: BlockPos) -> bool {
    let above = pos.above();
    is_redstone_conductor(world, world.get_block_state(above), above)
}

/// Resolves the menu containers for `pos`: the single container, or both halves
/// when this chest forms a double chest like vanilla's `DoubleBlockCombiner`.
///
/// Returns `None` when vanilla's blocked check rejects the chest, in which case
/// no menu is created at all.
pub(crate) fn combined_refs(
    world: &dyn LevelReader,
    pos: BlockPos,
    state: BlockStateId,
) -> Option<(ContainerRef, Option<ContainerRef>)> {
    if is_chest_blocked_at(world, pos) {
        return None;
    }

    let block_entity = world.get_block_entity(pos)?;
    let own = ContainerRef::from_block_entity(block_entity)?;
    if state.get_value(CHEST_TYPE) == ChestType::Single {
        return Some((own, None));
    }

    let partner_pos = connected_direction(state).relative(pos);
    if is_chest_blocked_at(world, partner_pos) {
        return None;
    }
    let partner_state = world.get_block_state(partner_pos);
    // The partner must be the same block, facing the same way, with the
    // complementary type: vanilla's `DoubleBlockCombiner.combineWithNeigbour`.
    if partner_state.get_block() != state.get_block()
        || partner_state.get_value(CHEST_TYPE) == ChestType::Single
        || partner_state.get_value(CHEST_TYPE) == state.get_value(CHEST_TYPE)
        || partner_state.get_value(FACING) != state.get_value(FACING)
    {
        return Some((own, None));
    }

    let Some(partner) = world
        .get_block_entity(partner_pos)
        .and_then(ContainerRef::from_block_entity)
    else {
        return Some((own, None));
    };
    // Vanilla canonical order: `first` of a LEFT pair is the other half.
    if state.get_value(CHEST_TYPE) == ChestType::Right {
        Some((own, Some(partner)))
    } else {
        Some((partner, Some(own)))
    }
}

pub(crate) fn open_menu(
    state: BlockStateId,
    world: &Arc<World>,
    pos: BlockPos,
    player: &Player,
    open_stat: CustomStatRef,
) -> InteractionResult {
    let Some((first, second)) = combined_refs(world.as_ref(), pos, state) else {
        return InteractionResult::Success;
    };

    let inventory = Arc::clone(&player.inventory);
    if let Some(second) = second {
        player.open_menu(
            TextComponent::translated(translations::CONTAINER_CHEST_DOUBLE.msg()),
            move |context| double_chest(inventory, context.container_id, first, second),
        );
    } else {
        player.open_menu(
            TextComponent::translated(translations::CONTAINER_CHEST.msg()),
            move |context| chest(inventory, context.container_id, first, 3),
        );
    }

    player.award_custom_stat(open_stat);
    InteractionResult::Success
}

/// Vanilla `AbstractContainerMenu.getRedstoneSignalFromContainer` over the
/// 54 slots of a double chest.
///
/// The shared single-container helper cannot describe two containers, so the
/// same weighted formula is applied over both halves' slots.
fn combined_signal(first: &dyn Container, second: &dyn Container) -> i32 {
    let size = first.get_container_size() + second.get_container_size();
    if size == 0 {
        return 0;
    }

    let mut total_percent: f32 = 0.0;
    for container in [first, second] {
        for slot in 0..container.get_container_size() {
            let item = container.get_item(slot);
            if !item.is_empty() {
                let max_stack = container.get_max_stack_size_for_item(item);
                total_percent += item.count() as f32 / max_stack as f32;
            }
        }
    }

    total_percent /= size as f32;
    (total_percent * 14.0).floor() as i32 + i32::from(total_percent > 0.0)
}

/// Vanilla `ChestBlock.getAnalogOutputSignal`, including the blocked check of
/// `getContainer(..., ignoreBeingBlocked = false)`.
pub(crate) fn analog_signal(state: BlockStateId, world: &dyn LevelReader, pos: BlockPos) -> i32 {
    let Some((first, second)) = combined_refs(world, pos, state) else {
        return 0;
    };

    if let Some(second) = second {
        let guard = ContainerLockGuard::lock_all(&[&first, &second]);
        let (Some(first), Some(second)) = (
            guard.get(first.container_id()),
            guard.get(second.container_id()),
        ) else {
            return 0;
        };
        combined_signal(first, second)
    } else {
        let guard = ContainerLockGuard::lock_all(&[&first]);
        guard.get(first.container_id()).map_or(0, |container| {
            calculate_redstone_signal_from_container(container)
        })
    }
}

fn place_new_block_entity(
    block_entity_type: BlockEntityTypeRef,
    level: Weak<World>,
    pos: BlockPos,
    state: BlockStateId,
) -> BlockEntityCreation {
    BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
        block_entity_type,
        level,
        pos,
        state,
    ))
}

impl BlockBehavior for ChestBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        state_for_placement(self.block, context)
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
        update_shape(state, direction, neighbor_state, self.block)
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
        place_new_block_entity(&vanilla_block_entity_types::CHEST, level, pos, state)
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
        // Vanilla forwards block events to the block entity (`BaseEntityBlock`),
        // which publishes the lid animation to clients.
        let Some(block_entity) = world.get_block_entity(pos) else {
            return false;
        };
        block_entity.trigger_event(param_a, param_b)
    }
}

impl BlockBehavior for TrappedChestBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        state_for_placement(self.block, context)
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
        update_shape(state, direction, neighbor_state, self.block)
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
        open_menu(
            state,
            world,
            pos,
            player,
            &vanilla_custom_stats::TRIGGER_TRAPPED_CHEST,
        )
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        place_new_block_entity(
            &vanilla_block_entity_types::TRAPPED_CHEST,
            level,
            pos,
            state,
        )
    }

    fn tick(&self, _state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
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

    fn is_signal_source(&self, _state: BlockStateId, _context: SignalQueryContext) -> bool {
        true
    }

    /// Vanilla `TrappedChestBlock.ownSignal`: the opener count, clamped to 15.
    fn get_own_signal(
        &self,
        _state: BlockStateId,
        world: &dyn LevelReader,
        pos: BlockPos,
        _context: SignalQueryContext,
    ) -> i32 {
        ChestBlockEntity::open_count(world, pos).clamp(0, 15)
    }

    /// Vanilla `TrappedChestBlock.getDirectSignal`: only upward.
    fn get_direct_signal(
        &self,
        state: BlockStateId,
        world: &dyn LevelReader,
        pos: BlockPos,
        direction: Direction,
        context: SignalQueryContext,
    ) -> i32 {
        if direction == Direction::Up {
            self.get_own_signal(state, world, pos, context)
        } else {
            0
        }
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
