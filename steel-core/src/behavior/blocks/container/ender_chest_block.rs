//! Ender chest block behavior implementation.
//!
//! Vanilla `EnderChestBlock` shows the *player's* ender chest inventory; the
//! block entity only supplies the lid animation and the open/close sounds.
//!
//! Deviations:
//! - Vanilla `EnderChestBlock.useWithoutItem` calls
//!   `PiglinAi.angerNearbyPiglins(level, player, true)`; Steel has no such helper
//!   (owner: `entity/ai/behavior/piglin/ai.rs`).
//! - Vanilla `EnderChestBlock.tick` calls `EnderChestBlockEntity.recheckOpen`
//!   and its `triggerEvent` drives a `ChestLidController`; Steel's
//!   `EnderChestBlockEntity` has neither an opener counter nor a lid controller
//!   (owner: `block_entity/entities/ender_chest.rs`).
//! - Vanilla `animateTick` emits portal particles on the client; Steel has no
//!   client-side block ticking.

use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::{vanilla_block_entity_types, vanilla_custom_stats};
use steel_utils::{BlockPos, BlockStateId, Direction, translations};
use text_components::TextComponent;

use crate::behavior::InventoryAccess;
use crate::behavior::block::{
    BlockBehavior, BlockEntityCreation, schedule_water_tick_if_waterlogged,
};
use crate::behavior::context::{BlockHitResult, BlockPlaceContext, InteractionResult};
use crate::block_entity::BLOCK_ENTITIES;
use crate::entity::ai::path::PathComputationType;
use crate::inventory::menu::kinds::ender_chest;
use crate::player::Player;
use crate::world::{ScheduledTickAccess, World, is_redstone_conductor};

/// The ender chest block behavior.
#[block_behavior]
pub struct EnderChestBlock {
    block: BlockRef,
}

impl EnderChestBlock {
    /// Creates a new ender chest block.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }
}

impl BlockBehavior for EnderChestBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        let facing = context.horizontal_direction().opposite();

        Some(
            self.block
                .default_state()
                .set_value(&BlockStateProperties::FACING, facing)
                .set_value(
                    &BlockStateProperties::WATERLOGGED,
                    context.is_water_source(),
                ),
        )
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
            return InteractionResult::Success;
        };
        if block_entity.get_type() != &vanilla_block_entity_types::ENDER_CHEST {
            return InteractionResult::Success;
        }

        let above_pos = pos.above();
        if is_redstone_conductor(world.as_ref(), world.get_block_state(above_pos), above_pos) {
            return InteractionResult::Success;
        }

        let inventory = Arc::clone(&player.inventory);
        let container = Arc::clone(&player.ender_chest_inventory);
        let chest = Arc::downgrade(&block_entity);
        player.open_menu(
            TextComponent::translated(translations::CONTAINER_ENDERCHEST.msg()),
            move |context| {
                // open only after active chest has closed, since
                // closing an ender chest menu clears the active chest.
                container.lock().set_active_chest(chest);
                ender_chest(inventory, context.container_id, container)
            },
        );

        player.award_custom_stat(&vanilla_custom_stats::OPEN_ENDERCHEST);

        InteractionResult::Success
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
            &vanilla_block_entity_types::ENDER_CHEST,
            level,
            pos,
            state,
        ))
    }

    /// Vanilla `EnderChestBlock.updateShape` reschedules the water tick while
    /// waterlogged, then defers to the base behaviour.
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

    /// Vanilla `BaseEntityBlock.triggerEvent` forwards block events to the block
    /// entity, where `EnderChestBlockEntity.triggerEvent` drives the lid.
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

    /// Vanilla `EnderChestBlock.isPathfindable` always returns `false`.
    fn is_pathfindable(
        &self,
        _state: BlockStateId,
        _computation_type: PathComputationType,
    ) -> bool {
        false
    }
}
