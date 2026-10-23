//! `ShelfBlock` behavior (`net.minecraft.world.level.block.ShelfBlock`).
//!
//! Vanilla `ShelfBlock` is also a `SideChainPartBlock`. Steel has no extracted
//! side-chain-part behavior yet, so the `SIDE_CHAIN_PART` property is kept at
//! `unconnected` and powered multi-shelf hotbar swapping is not performed.
//!
//! TODO(deps): powered multi-slot hotbar swap and side-chain chaining
//! (`getAllBlocksConnectedTo`, `updateSelfAndNeighborsOnPoweringUp`) once Steel
//! has the vanilla side-chain helper.

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
use steel_registry::{sound_events, vanilla_block_entity_types, vanilla_game_events};
use steel_utils::types::{InteractionHand, UpdateFlags};
use steel_utils::{BlockPos, BlockStateId, Downcast as _};

use crate::behavior::block::schedule_water_tick_if_waterlogged;
use crate::behavior::{
    BlockBehavior, BlockEntityCreation, BlockHitResult, BlockPlaceContext, InteractionResult,
    InventoryAccess,
};
use crate::block_entity::entities::ShelfBlockEntity;
use crate::block_entity::{BLOCK_ENTITIES, BlockEntityTicker};
use crate::player::Player;
use crate::world::game_event::GameEventContext;
use crate::world::{LevelReader, ScheduledTickAccess, SignalGetter as _, World};

const FACING: &EnumProperty<Direction> = &BlockStateProperties::HORIZONTAL_FACING;
const POWERED: &BoolProperty = &BlockStateProperties::POWERED;
const SIDE_CHAIN_PART: &EnumProperty<SideChainPart> = &BlockStateProperties::SIDE_CHAIN_PART;
const WATERLOGGED: &BoolProperty = &BlockStateProperties::WATERLOGGED;

/// Columns in a shelf (vanilla `ShelfBlock.getColumns`).
const SHELF_COLUMNS: i32 = 3;
const PIXELS_PER_BLOCK_EDGE: f32 = 16.0;

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

    fn play_sound(
        world: &Arc<World>,
        pos: BlockPos,
        sound: &'static steel_registry::sound_event::SoundEvent,
    ) {
        world.play_block_sound(sound, pos, 1.0, 1.0, None);
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
        let Some(shelf) = block_entity.downcast_ref::<ShelfBlockEntity>() else {
            return InteractionResult::Pass;
        };
        let Some(slot) = Self::hit_slot(state, hit_result) else {
            return InteractionResult::Pass;
        };

        if state.get_value(POWERED) {
            // TODO(deps): powered multi-slot hotbar swap needs the side-chain
            // helper to gather connected shelves.
            return InteractionResult::Consume;
        }

        let removed = inv.with_item(|held| {
            let placed = std::mem::replace(held, ItemStack::empty());
            let removed = shelf.swap_item(slot, placed.copy_with_count(placed.count()));
            *held = if player.has_infinite_materials() && removed.is_empty() {
                placed
            } else {
                removed.copy_with_count(removed.count())
            };
            removed
        });

        let held_is_empty = inv.with_item(|held| held.is_empty());
        if !removed.is_empty() {
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
        } else {
            if held_is_empty {
                return InteractionResult::Pass;
            }
            world.play_block_sound(&sound_events::BLOCK_SHELF_PLACE_ITEM, pos, 1.0, 1.0, None);
        }

        shelf.mark_changed(world, Some(&vanilla_game_events::ITEM_INTERACT_FINISH));
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
        let Some(shelf) = block_entity.downcast_ref::<ShelfBlockEntity>() else {
            return 0;
        };

        let mut signal = 0;
        for slot in 0..SHELF_COLUMNS as usize {
            if !shelf.item(slot).is_empty() {
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
