//! Lectern block behavior implementation.
//!
//! A lectern holds one book: right-clicking with a writable or written book
//! places it; right-clicking a filled lectern opens the
//! [`lectern`](crate::inventory::menu::kinds::lectern) menu. Page turns pulse
//! the `POWERED` property for two ticks, mirroring vanilla's
//! `signalPageChange`, and comparators read the page fraction through the
//! block entity.

use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::blocks::properties::{
    BlockStateProperties, BoolProperty, Direction, EnumProperty,
};
use steel_registry::data_components::vanilla_components::BLOCK_ENTITY_DATA;
use steel_registry::item_stack::ItemStack;
use steel_registry::{
    REGISTRY, TaggedRegistryExt as _, sound_events, vanilla_block_entity_types,
    vanilla_custom_stats, vanilla_item_tags,
};
use steel_utils::types::{InteractionHand, UpdateFlags};
use steel_utils::{BlockPos, BlockStateId, Downcast as _, translations};
use text_components::TextComponent;

use crate::behavior::block::{BlockBehavior, BlockEntityCreation};
use crate::behavior::context::{BlockHitResult, BlockPlaceContext, InteractionResult};
use crate::behavior::{InventoryAccess, PlacementSource};
use crate::block_entity::BLOCK_ENTITIES;
use crate::block_entity::entities::LecternBlockEntity;
use crate::inventory::lock::ContainerRef;
use crate::inventory::menu::kinds::lectern;
use crate::player::Player;
use crate::world::SignalQueryContext;
use crate::world::{LevelReader, World};

/// Behavior for lectern blocks.
///
/// Mirrors vanilla `LecternBlock`: horizontal facing plus the `HAS_BOOK` and
/// `POWERED` properties, book placement through `use_item_on`, and the lectern
/// menu through `use_without_item`.
#[block_behavior]
pub struct LecternBlock {
    block: BlockRef,
}

const FACING: &EnumProperty<Direction> = &BlockStateProperties::HORIZONTAL_FACING;
const HAS_BOOK: &BoolProperty = &BlockStateProperties::HAS_BOOK;
const POWERED: &BoolProperty = &BlockStateProperties::POWERED;

/// Vanilla redstone output of a lectern whose page just changed.
const POWERED_SIGNAL: i32 = 15;

impl LecternBlock {
    /// Creates a new lectern block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Opens the lectern menu for a player at a book-bearing lectern.
    fn open_screen(world: &Arc<World>, pos: BlockPos, player: &Player) {
        let Some(block_entity) = world.get_block_entity(pos) else {
            return;
        };
        let Some(container_ref) = ContainerRef::from_block_entity(Arc::clone(&block_entity)) else {
            return;
        };
        let Some(lectern_entity) = block_entity.downcast_ref::<LecternBlockEntity>() else {
            return;
        };
        let state = lectern_entity.state_handle();
        player.open_menu(
            TextComponent::translated(translations::CONTAINER_LECTERN.msg()),
            move |context| lectern(context.container_id, container_ref, state),
        );
        player.award_custom_stat(&vanilla_custom_stats::INTERACT_WITH_LECTERN);
    }

    /// Mirrors vanilla `LecternBlock.placeBook`: store the book and play the
    /// placement sound. `HAS_BOOK` is published by the block entity once the
    /// container lock is released.
    fn place_book(world: &Arc<World>, pos: BlockPos, book: ItemStack) {
        let Some(block_entity) = world.get_block_entity(pos) else {
            return;
        };
        let Some(lectern) = block_entity.downcast_ref::<LecternBlockEntity>() else {
            return;
        };
        lectern.set_book(book);
        world.play_block_sound(&sound_events::ITEM_BOOK_PUT, pos, 1.0, 1.0, None);
    }
}

impl BlockBehavior for LecternBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        // Vanilla restores `HAS_BOOK` from a creative pick-block item carrying
        // `BLOCK_ENTITY_DATA` with a stored book.
        let has_book = context.with_item(|stack| {
            stack
                .get(BLOCK_ENTITY_DATA)
                .is_some_and(|data| data.data().as_compound().get("Book").is_some())
        });
        Some(
            self.block
                .default_state()
                .set_value(FACING, context.horizontal_direction().opposite())
                .set_value(POWERED, false)
                .set_value(HAS_BOOK, has_book),
        )
    }

    fn set_placed_by(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        source: &PlacementSource<'_>,
    ) {
        let payload = source.with_item(|stack| {
            stack
                .get(BLOCK_ENTITY_DATA)
                .map(|data| data.data().copy_tag())
        });
        let Some(payload) = payload else {
            return;
        };
        let Some(block_entity) = world.get_block_entity(pos) else {
            return;
        };
        let Some(lectern) = block_entity.downcast_ref::<LecternBlockEntity>() else {
            return;
        };
        lectern.apply_item_block_entity_data(&payload);
    }

    fn use_item_on(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hand: InteractionHand,
        _hit_result: &BlockHitResult,
        inv: &mut InventoryAccess,
    ) -> InteractionResult {
        if state.get_value(HAS_BOOK) {
            return InteractionResult::TryEmptyHandInteraction;
        }
        let is_lectern_book = inv.with_item(|stack| {
            !stack.is_empty()
                && REGISTRY
                    .items
                    .is_in_tag(stack.item(), &vanilla_item_tags::ItemTag::LECTERN_BOOKS)
        });
        if !is_lectern_book {
            return InteractionResult::TryEmptyHandInteraction;
        }

        let infinite = player.has_infinite_materials();
        let book = inv.with_item(|stack| stack.consume_and_return(1, infinite));
        Self::place_book(world, pos, book);
        InteractionResult::Success
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
        if state.get_value(HAS_BOOK) {
            Self::open_screen(world, pos, player);
            InteractionResult::Success
        } else {
            InteractionResult::Consume
        }
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
            &vanilla_block_entity_types::LECTERN,
            level,
            pos,
            state,
        ))
    }

    fn is_signal_source(&self, _state: BlockStateId, _context: SignalQueryContext) -> bool {
        true
    }

    fn get_own_signal(
        &self,
        state: BlockStateId,
        _world: &dyn LevelReader,
        _pos: BlockPos,
        _context: SignalQueryContext,
    ) -> i32 {
        if state.get_value(POWERED) {
            POWERED_SIGNAL
        } else {
            0
        }
    }

    fn get_direct_signal(
        &self,
        state: BlockStateId,
        _world: &dyn LevelReader,
        _pos: BlockPos,
        direction: Direction,
        _context: SignalQueryContext,
    ) -> i32 {
        if direction == Direction::Up && state.get_value(POWERED) {
            POWERED_SIGNAL
        } else {
            0
        }
    }

    fn affect_neighbors_after_removal(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _moved_by_piston: bool,
    ) {
        if state.get_value(POWERED) {
            world.update_neighbors_at(pos.below(), state.get_block());
        }
    }

    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        world.set_block(
            pos,
            state.set_value(POWERED, false),
            UpdateFlags::UPDATE_ALL,
        );
        world.update_neighbors_at(pos.below(), state.get_block());
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
        // Vanilla `LecternBlock.getAnalogOutputSignal` reads the block entity
        // directly and yields 0 unless `HAS_BOOK` is set.
        if !state.get_value(HAS_BOOK) {
            return 0;
        }
        world
            .get_block_entity(pos)
            .and_then(|block_entity| {
                block_entity
                    .downcast_ref::<LecternBlockEntity>()
                    .map(LecternBlockEntity::redstone_signal)
            })
            .unwrap_or(0)
    }
}
