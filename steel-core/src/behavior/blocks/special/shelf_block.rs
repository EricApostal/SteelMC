//! `ShelfBlock` behavior (`net.minecraft.world.level.block.ShelfBlock`).
//!
//! Includes the vanilla `SideChainPartBlock` chaining used by powered shelves:
//! adjacent powered wooden shelves connect into chains of up to three parts, and
//! a powered shelf swaps the player's hotbar across the whole chain.

use std::mem;
use std::sync::{Arc, Weak};

use glam::DVec3;
use steel_macros::block_behavior;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{
    BlockStateProperties, BoolProperty, Direction, EnumProperty, SideChainPart,
};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEvent;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::{REGISTRY, sound_events, vanilla_block_entity_types, vanilla_game_events};
use steel_utils::types::{InteractionHand, UpdateFlags};
use steel_utils::{BlockPos, BlockStateId, Downcast as _};

use crate::behavior::block::schedule_water_tick_if_waterlogged;
use crate::behavior::{
    BlockBehavior, BlockEntityCreation, BlockHitResult, BlockPlaceContext, InteractionResult,
    InventoryAccess,
};
use crate::block_entity::entities::ShelfBlockEntity;
use crate::block_entity::{BLOCK_ENTITIES, BlockEntityTicker};
use crate::inventory::container::Container as _;
use crate::player::Player;
use crate::player::player_inventory::PlayerInventory;
use crate::world::game_event::GameEventContext;
use crate::world::{LevelReader, ScheduledTickAccess, SignalGetter as _, World};

const FACING: &EnumProperty<Direction> = &BlockStateProperties::HORIZONTAL_FACING;
const POWERED: &BoolProperty = &BlockStateProperties::POWERED;
const SIDE_CHAIN_PART: &EnumProperty<SideChainPart> = &BlockStateProperties::SIDE_CHAIN_PART;
const WATERLOGGED: &BoolProperty = &BlockStateProperties::WATERLOGGED;

/// Columns in a shelf (vanilla `ShelfBlock.getColumns`).
const SHELF_COLUMNS: i32 = 3;
const PIXELS_PER_BLOCK_EDGE: f32 = 16.0;
/// Vanilla `ShelfBlock.getMaxChainLength`.
const MAX_CHAIN_LENGTH: i32 = 3;

/// Vanilla `SideChainPart.isConnected`.
const fn part_is_connected(part: SideChainPart) -> bool {
    !matches!(part, SideChainPart::Unconnected)
}

/// Vanilla `SideChainPart.isConnectionTowards`.
const fn part_connects_towards(part: SideChainPart, end: SideChainPart) -> bool {
    matches!(
        (part, end),
        (SideChainPart::Center, _)
            | (SideChainPart::Right, SideChainPart::Right)
            | (SideChainPart::Left, SideChainPart::Left)
    )
}

/// Vanilla `SideChainPart.isChainEnd`.
const fn part_is_chain_end(part: SideChainPart) -> bool {
    !matches!(part, SideChainPart::Center)
}

/// Vanilla `SideChainPart.whenConnectedToTheRight`.
const fn part_connected_to_right(part: SideChainPart) -> SideChainPart {
    match part {
        SideChainPart::Unconnected | SideChainPart::Left => SideChainPart::Left,
        SideChainPart::Right | SideChainPart::Center => SideChainPart::Center,
    }
}

/// Vanilla `SideChainPart.whenConnectedToTheLeft`.
const fn part_connected_to_left(part: SideChainPart) -> SideChainPart {
    match part {
        SideChainPart::Unconnected | SideChainPart::Right => SideChainPart::Right,
        SideChainPart::Center | SideChainPart::Left => SideChainPart::Center,
    }
}

/// Vanilla `SideChainPart.whenDisconnectedFromTheRight`.
const fn part_disconnected_from_right(part: SideChainPart) -> SideChainPart {
    match part {
        SideChainPart::Unconnected | SideChainPart::Left => SideChainPart::Unconnected,
        SideChainPart::Right | SideChainPart::Center => SideChainPart::Right,
    }
}

/// Vanilla `SideChainPart.whenDisconnectedFromTheLeft`.
const fn part_disconnected_from_left(part: SideChainPart) -> SideChainPart {
    match part {
        SideChainPart::Unconnected | SideChainPart::Right => SideChainPart::Unconnected,
        SideChainPart::Center | SideChainPart::Left => SideChainPart::Left,
    }
}

/// One step of a shelf side chain (vanilla `SideChainPartBlock.Neighbor`).
#[derive(Clone, Copy)]
enum ChainNeighbor {
    /// Vanilla `SideChainPartBlock.EmptyNeighbor`.
    Empty(BlockPos),
    /// Vanilla `SideChainPartBlock.SideChainNeighbor`.
    Side { pos: BlockPos, part: SideChainPart },
}

impl ChainNeighbor {
    const fn pos(self) -> BlockPos {
        match self {
            Self::Empty(pos) | Self::Side { pos, .. } => pos,
        }
    }

    const fn is_connectable(self) -> bool {
        matches!(self, Self::Side { .. })
    }

    const fn is_unconnectable_or_chain_end(self) -> bool {
        match self {
            Self::Empty(_) => true,
            Self::Side { part, .. } => part_is_chain_end(part),
        }
    }

    const fn connects_towards(self, end_part: SideChainPart) -> bool {
        match self {
            Self::Empty(_) => false,
            Self::Side { part, .. } => part_connects_towards(part, end_part),
        }
    }
}

/// `ShelfBlock` behavior.
#[block_behavior]
pub struct ShelfBlock {
    block: BlockRef,
}

impl ShelfBlock {
    /// Creates a shelf block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    fn section(relative_coordinate: f64, section_count: i32) -> i32 {
        let targeted_pixel = relative_coordinate as f32 * PIXELS_PER_BLOCK_EDGE;
        let section_size = PIXELS_PER_BLOCK_EDGE / section_count as f32;
        ((targeted_pixel / section_size).floor() as i32).clamp(0, section_count - 1)
    }

    /// Vanilla `SelectableSlotContainer.getHitSlot` for a one-row shelf.
    fn hit_slot(state: BlockStateId, hit_result: &BlockHitResult) -> Option<usize> {
        let facing = state.get_value(FACING);
        if facing != hit_result.direction {
            return None;
        }

        let hit_face_origin = hit_result.direction.relative(hit_result.block_pos);
        let relative_hit = hit_result.location
            - DVec3::new(
                f64::from(hit_face_origin.x()),
                f64::from(hit_face_origin.y()),
                f64::from(hit_face_origin.z()),
            );

        let relative_x = match hit_result.direction {
            Direction::North => 1.0 - relative_hit.x,
            Direction::South => relative_hit.x,
            Direction::West => relative_hit.z,
            Direction::East => 1.0 - relative_hit.z,
            Direction::Down | Direction::Up => return None,
        };

        Some(Self::section(relative_x, SHELF_COLUMNS) as usize)
    }

    fn play_sound(world: &Arc<World>, pos: BlockPos, sound: &'static SoundEvent) {
        world.play_block_sound(sound, pos, 1.0, 1.0, None);
    }

    /// Vanilla `SideChainPartBlock.isConnectable`.
    fn is_connectable(state: BlockStateId) -> bool {
        REGISTRY
            .blocks
            .by_state_id(state)
            .is_some_and(|block| block.has_tag(&BlockTag::WOODEN_SHELVES))
            && state.try_get_value(POWERED) == Some(true)
    }

    /// Reads one chain neighbor at `steps` in the clockwise (`left`) or
    /// counter-clockwise (`right`) direction from `center`.
    fn neighbor_at(
        world: &dyn LevelReader,
        center: BlockPos,
        facing: Direction,
        clockwise: bool,
        steps: i32,
    ) -> ChainNeighbor {
        let direction = if clockwise {
            facing.rotate_y_clockwise()
        } else {
            facing.rotate_y_counter_clockwise()
        };
        let pos = center.relative_n(direction, steps);
        let state = world.get_block_state(pos);
        let connectable =
            Self::is_connectable(state) && state.try_get_value(FACING) == Some(facing);
        if connectable {
            ChainNeighbor::Side {
                pos,
                part: state.get_value(SIDE_CHAIN_PART),
            }
        } else {
            ChainNeighbor::Empty(pos)
        }
    }

    /// Vanilla `SideChainPartBlock.getAllBlocksConnectedTo`.
    fn all_blocks_connected_to(world: &dyn LevelReader, pos: BlockPos) -> Vec<BlockPos> {
        let state = world.get_block_state(pos);
        if !Self::is_connectable(state) {
            return Vec::new();
        }
        let facing = state.get_value(FACING);

        let mut results = vec![pos];
        Self::add_blocks_connecting_towards(
            world,
            pos,
            facing,
            true,
            SideChainPart::Left,
            &mut results,
            true,
        );
        Self::add_blocks_connecting_towards(
            world,
            pos,
            facing,
            false,
            SideChainPart::Right,
            &mut results,
            false,
        );
        results
    }

    fn add_blocks_connecting_towards(
        world: &dyn LevelReader,
        center: BlockPos,
        facing: Direction,
        clockwise: bool,
        end_part: SideChainPart,
        results: &mut Vec<BlockPos>,
        prepend: bool,
    ) {
        for steps in 1..MAX_CHAIN_LENGTH {
            let neighbor = Self::neighbor_at(world, center, facing, clockwise, steps);
            if neighbor.connects_towards(end_part) {
                if prepend {
                    results.insert(0, neighbor.pos());
                } else {
                    results.push(neighbor.pos());
                }
            }
            if neighbor.is_unconnectable_or_chain_end() {
                break;
            }
        }
    }

    /// Vanilla `SideChainPartBlock.setPart`.
    fn set_part(world: &Arc<World>, pos: BlockPos, new_part: SideChainPart) {
        let state = world.get_block_state(pos);
        if state.get_value(SIDE_CHAIN_PART) != new_part {
            world.set_block(
                pos,
                state.set_value(SIDE_CHAIN_PART, new_part),
                UpdateFlags::UPDATE_ALL_IMMEDIATE,
            );
        }
    }

    /// Vanilla `SideChainPartBlock.updateNeighborsAfterPoweringDown`.
    fn update_neighbors_after_powering_down(
        world: &Arc<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) {
        let facing = state.get_value(FACING);
        let left = Self::neighbor_at(world, pos, facing, true, 1);
        if let ChainNeighbor::Side {
            pos: neighbor_pos,
            part,
        } = left
        {
            Self::set_part(world, neighbor_pos, part_disconnected_from_right(part));
        }
        let right = Self::neighbor_at(world, pos, facing, false, 1);
        if let ChainNeighbor::Side {
            pos: neighbor_pos,
            part,
        } = right
        {
            Self::set_part(world, neighbor_pos, part_disconnected_from_left(part));
        }
    }

    /// Vanilla `SideChainPartBlock.updateSelfAndNeighborsOnPoweringUp`.
    fn update_self_and_neighbors_on_powering_up(
        world: &Arc<World>,
        pos: BlockPos,
        state: BlockStateId,
        old_state: BlockStateId,
    ) {
        if !Self::is_connectable(state) || Self::is_being_updated_by_neighbor(state, old_state) {
            return;
        }

        let facing = state.get_value(FACING);
        let left = Self::neighbor_at(world, pos, facing, true, 1);
        let right = Self::neighbor_at(world, pos, facing, false, 1);
        let left_chain = if left.is_connectable() {
            Self::all_blocks_connected_to(world, left.pos()).len()
        } else {
            0
        };
        let right_chain = if right.is_connectable() {
            Self::all_blocks_connected_to(world, right.pos()).len()
        } else {
            0
        };

        let mut new_part = SideChainPart::Unconnected;
        let mut current_chain_length = 1usize;
        if Self::can_connect(left_chain, current_chain_length) {
            new_part = part_connected_to_left(new_part);
            if let ChainNeighbor::Side {
                pos: left_pos,
                part,
            } = left
            {
                Self::set_part(world, left_pos, part_connected_to_right(part));
            }
            current_chain_length += left_chain;
        }
        if Self::can_connect(right_chain, current_chain_length)
            && let ChainNeighbor::Side {
                pos: right_pos,
                part,
            } = right
        {
            new_part = part_connected_to_right(new_part);
            Self::set_part(world, right_pos, part_connected_to_left(part));
        }

        Self::set_part(world, pos, new_part);
    }

    /// Vanilla `SideChainPartBlock.canConnect`.
    const fn can_connect(new_blocks_to_connect_to: usize, current_chain_length: usize) -> bool {
        new_blocks_to_connect_to > 0
            && current_chain_length + new_blocks_to_connect_to <= MAX_CHAIN_LENGTH as usize
    }

    /// Vanilla `SideChainPartBlock.isBeingUpdatedByNeighbor`.
    fn is_being_updated_by_neighbor(state: BlockStateId, old_state: BlockStateId) -> bool {
        let is_getting_connected = part_is_connected(state.get_value(SIDE_CHAIN_PART));
        let has_been_connected_before = Self::is_connectable(old_state)
            && part_is_connected(old_state.get_value(SIDE_CHAIN_PART));
        is_getting_connected || has_been_connected_before
    }

    /// Vanilla `ShelfBlock.swapHotbar`.
    fn swap_hotbar(world: &Arc<World>, pos: BlockPos, inv: &mut InventoryAccess) -> bool {
        let connected = Self::all_blocks_connected_to(world, pos);
        if connected.is_empty() {
            return false;
        }

        let mut any_swapped = false;
        inv.with_inventory(|inventory| {
            for (part_index, shelf_pos) in connected.iter().enumerate() {
                let Some(block_entity) = world.get_block_entity(*shelf_pos) else {
                    continue;
                };
                let Some(shelf) = block_entity.downcast_ref::<ShelfBlockEntity>() else {
                    continue;
                };

                for slot in 0..SHELF_COLUMNS as usize {
                    let inventory_slot =
                        9 - (connected.len() - part_index) * SHELF_COLUMNS as usize + slot;
                    if inventory_slot >= PlayerInventory::INVENTORY_SIZE {
                        continue;
                    }

                    let placed = inventory.get_item(inventory_slot).clone();
                    let replaced = shelf.swap_item(slot, placed.clone());
                    if !placed.is_empty() || !replaced.is_empty() {
                        inventory.set_item(inventory_slot, replaced);
                        any_swapped = true;
                    }
                }
                shelf.mark_changed(world, Some(&vanilla_game_events::ENTITY_INTERACT));
            }
        });
        any_swapped
    }
}

impl BlockBehavior for ShelfBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(
            self.block
                .default_state()
                .set_value(FACING, context.horizontal_direction().opposite())
                .set_value(
                    POWERED,
                    context.world.has_neighbor_signal(context.place_pos()),
                )
                .set_value(SIDE_CHAIN_PART, SideChainPart::Unconnected)
                .set_value(WATERLOGGED, context.is_full_water()),
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
        schedule_water_tick_if_waterlogged(state, world, pos);
        state
    }

    /// Vanilla `ShelfBlock.onPlace`: connect or disconnect the side chain.
    fn on_place(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        old_state: BlockStateId,
        _moved_by_piston: bool,
    ) {
        if state.get_value(POWERED) {
            Self::update_self_and_neighbors_on_powering_up(world, pos, state, old_state);
        } else {
            Self::update_neighbors_after_powering_down(world, pos, state);
        }
    }

    /// Vanilla `ShelfBlock.affectNeighborsAfterRemoval`: disconnect the chain
    /// around a removed shelf.
    fn affect_neighbors_after_removal(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _moved_by_piston: bool,
    ) {
        Self::update_neighbors_after_powering_down(world, pos, state);
    }

    fn handle_neighbor_changed(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _source_block: BlockRef,
        _moved_by_piston: bool,
    ) {
        let signal = world.has_neighbor_signal(pos);
        if state.get_value(POWERED) == signal {
            return;
        }

        let mut new_state = state.set_value(POWERED, signal);
        if !signal {
            new_state = new_state.set_value(SIDE_CHAIN_PART, SideChainPart::Unconnected);
        }
        world.set_block(pos, new_state, UpdateFlags::UPDATE_ALL_IMMEDIATE);
        world.play_block_sound(
            if signal {
                &sound_events::BLOCK_SHELF_ACTIVATE
            } else {
                &sound_events::BLOCK_SHELF_DEACTIVATE
            },
            pos,
            1.0,
            1.0,
            None,
        );
        world.game_event(
            if signal {
                &vanilla_game_events::BLOCK_ACTIVATE
            } else {
                &vanilla_game_events::BLOCK_DEACTIVATE
            },
            pos,
            &GameEventContext::new(None, Some(new_state)),
        );
    }

    fn use_item_on(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        hand: InteractionHand,
        hit_result: &BlockHitResult,
        inv: &mut InventoryAccess,
    ) -> InteractionResult {
        if hand == InteractionHand::OffHand {
            return InteractionResult::Pass;
        }
        let Some(block_entity) = world.get_block_entity(pos) else {
            return InteractionResult::Pass;
        };
        let Some(shelf_entity) = block_entity.downcast_ref::<ShelfBlockEntity>() else {
            return InteractionResult::Pass;
        };
        let Some(slot) = Self::hit_slot(state, hit_result) else {
            return InteractionResult::Pass;
        };

        if state.get_value(POWERED) {
            let any_swapped = Self::swap_hotbar(world, pos, inv);
            if !any_swapped {
                return InteractionResult::Consume;
            }
            Self::play_sound(world, pos, &sound_events::BLOCK_SHELF_MULTI_SWAP);
            return InteractionResult::SuccessServer;
        }

        let removed = inv.with_item(|held| {
            let placed = mem::replace(held, ItemStack::empty());
            let removed = shelf_entity.swap_item(slot, placed.copy_with_count(placed.count()));
            *held = if player.has_infinite_materials() && removed.is_empty() {
                placed
            } else {
                removed.copy_with_count(removed.count())
            };
            removed
        });

        let held_is_empty = inv.with_item(|held| held.is_empty());
        if removed.is_empty() {
            if held_is_empty {
                return InteractionResult::Pass;
            }
            world.play_block_sound(&sound_events::BLOCK_SHELF_PLACE_ITEM, pos, 1.0, 1.0, None);
        } else {
            world.play_block_sound(
                if held_is_empty {
                    &sound_events::BLOCK_SHELF_TAKE_ITEM
                } else {
                    &sound_events::BLOCK_SHELF_SINGLE_SWAP
                },
                pos,
                1.0,
                1.0,
                None,
            );
        }

        shelf_entity.mark_changed(world, Some(&vanilla_game_events::ITEM_INTERACT_FINISH));
        InteractionResult::SuccessServer
    }

    fn has_analog_output_signal(&self, _state: BlockStateId) -> bool {
        true
    }

    fn get_analog_output_signal(
        &self,
        state: BlockStateId,
        world: &dyn LevelReader,
        pos: BlockPos,
        direction: Direction,
    ) -> i32 {
        if direction != state.get_value(FACING).opposite() {
            return 0;
        }
        let Some(block_entity) = world.get_block_entity(pos) else {
            return 0;
        };
        let Some(shelf_entity) = block_entity.downcast_ref::<ShelfBlockEntity>() else {
            return 0;
        };

        let mut signal = 0;
        for slot in 0..SHELF_COLUMNS as usize {
            if !shelf_entity.item(slot).is_empty() {
                signal |= 1 << slot;
            }
        }
        signal
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
            &vanilla_block_entity_types::SHELF,
            level,
            pos,
            state,
        ))
    }

    fn get_block_entity_ticker(
        &self,
        _world: &Arc<World>,
        _state: BlockStateId,
        _block_entity_type: BlockEntityTypeRef,
    ) -> Option<BlockEntityTicker> {
        // Vanilla `ShelfBlock` registers no ticker.
        None
    }
}
