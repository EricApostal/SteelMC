//! `DecoratedPotBlock` behavior (`net.minecraft.world.level.block.DecoratedPotBlock`).

use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{
    BlockStateProperties, BoolProperty, Direction, EnumProperty,
};
use steel_registry::stat::vanilla_stat_types;
use steel_registry::{sound_events, vanilla_block_entity_types, vanilla_game_events};
use steel_utils::types::{InteractionHand, UpdateFlags};
use steel_utils::{BlockPos, BlockStateId, Downcast as _};

use crate::behavior::block::schedule_water_tick_if_waterlogged;
use crate::behavior::{
    BlockBehavior, BlockEntityCreation, BlockHitResult, BlockPlaceContext, InteractionResult,
    InventoryAccess,
};
use crate::block_entity::entities::DecoratedPotBlockEntity;
use crate::block_entity::{BLOCK_ENTITIES, BlockEntityTicker};
use crate::entity::Entity;
use crate::entity::projectile::Projectile;
use crate::player::Player;
use crate::world::game_event::GameEventContext;
use crate::world::{ClipHitResult, LevelReader, ScheduledTickAccess, SignalQueryContext, World};

const HORIZONTAL_FACING: &EnumProperty<Direction> = &BlockStateProperties::HORIZONTAL_FACING;
const CRACKED: &BoolProperty = &BlockStateProperties::CRACKED;
const WATERLOGGED: &BoolProperty = &BlockStateProperties::WATERLOGGED;

/// `DecoratedPotBlock` behavior.
#[block_behavior]
pub struct DecoratedPotBlock {
    block: BlockRef,
}

impl DecoratedPotBlock {
    /// Creates a decorated pot block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }
}

impl BlockBehavior for DecoratedPotBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(
            self.block
                .default_state()
                .set_value(HORIZONTAL_FACING, context.horizontal_direction())
                .set_value(WATERLOGGED, context.is_full_water())
                .set_value(CRACKED, false),
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

    fn use_item_on(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hand: InteractionHand,
        _hit_result: &BlockHitResult,
        inv: &mut InventoryAccess,
    ) -> InteractionResult {
        let Some(block_entity) = world.get_block_entity(pos) else {
            return InteractionResult::Pass;
        };
        let Some(pot) = block_entity.downcast_ref::<DecoratedPotBlockEntity>() else {
            return InteractionResult::Pass;
        };

        if inv.with_item(|stack| stack.is_empty()) {
            return InteractionResult::TryEmptyHandInteraction;
        }

        let inserted_item = inv.with_item(|stack| stack.item());
        if pot.try_insert(world, pos, player, inv) {
            player.award_stat(&vanilla_stat_types::ITEM_USED, inserted_item);
            InteractionResult::Success
        } else {
            InteractionResult::TryEmptyHandInteraction
        }
    }

    fn use_without_item(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        let Some(block_entity) = world.get_block_entity(pos) else {
            return InteractionResult::Pass;
        };
        let Some(pot) = block_entity.downcast_ref::<DecoratedPotBlockEntity>() else {
            return InteractionResult::Pass;
        };

        world.play_block_sound(
            &sound_events::BLOCK_DECORATED_POT_INSERT_FAIL,
            pos,
            1.0,
            1.0,
            None,
        );
        pot.wobble_negative();
        world.game_event(
            &vanilla_game_events::BLOCK_CHANGE,
            pos,
            &GameEventContext::new(Some(player as &dyn Entity), None),
        );
        InteractionResult::Success
    }

    fn player_will_destroy(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _player: &Player,
    ) -> BlockStateId {
        // Vanilla only cracks the pot when broken with a shattering item; the
        // tag check lives on the destroyed item, which Steel does not hand to
        // block behavior at this hook (see `player_will_destroy` signature),
        // so Steel leaves the state unchanged here.
        //
        // TODO(deps): crack the pot when the destroyed-with item carries the
        // `minecraft:breaks_decorated_pots` item tag once the item is available
        // at this hook or a `player_destroy` tool-equivalent hook exists.
        let _ = (state, world, pos);
        state
    }

    fn on_projectile_hit(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        hit: &ClipHitResult,
        projectile: &dyn Projectile,
    ) {
        let pos = hit.block_pos;
        if projectile.may_interact(world, pos) && projectile.may_break(world) {
            world.set_block(
                pos,
                state.set_value(CRACKED, true),
                UpdateFlags::UPDATE_ALL_IMMEDIATE,
            );
            let _ = world.destroy_block(pos, true);
        }
    }

    fn has_analog_output_signal(&self, _state: BlockStateId) -> bool {
        true
    }

    fn get_analog_output_signal(
        &self,
        _state: BlockStateId,
        world: &dyn LevelReader,
        pos: BlockPos,
        _direction: Direction,
    ) -> i32 {
        let Some(block_entity) = world.get_block_entity(pos) else {
            return 0;
        };
        let Some(pot) = block_entity.downcast_ref::<DecoratedPotBlockEntity>() else {
            return 0;
        };
        pot.analog_output_signal()
    }

    fn is_signal_source(&self, _state: BlockStateId, _context: SignalQueryContext) -> bool {
        false
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
            &vanilla_block_entity_types::DECORATED_POT,
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
        // Vanilla `DecoratedPotBlock` registers no ticker.
        None
    }
}
