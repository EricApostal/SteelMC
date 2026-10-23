//! Vanilla anvil behavior.
//!
//! Anvils are falling blocks whose impact hurts entities with the anvil damage
//! source, plays the anvil land/broken level events, and that open the repair
//! menu on use.

use std::sync::Arc;

use steel_macros::block_behavior;
use steel_registry::{
    blocks::{
        BlockRef,
        block_state_ext::BlockStateExt,
        properties::{BlockStateProperties, EnumProperty},
    },
    items::item::BlockHitResult,
    vanilla_blocks, vanilla_custom_stats, vanilla_damage_types, vanilla_level_events,
};
use steel_utils::{BlockPos, BlockStateId, Direction, translations};
use text_components::TextComponent;

use crate::{
    behavior::blocks::falling::FallingBlock,
    behavior::{BlockBehavior, BlockPlaceContext, Fallable, InteractionResult, InventoryAccess},
    entity::ai::path::PathComputationType,
    entity::damage::DamageSource,
    entity::entities::FallingBlockEntity,
    inventory::menu::kinds::anvil,
    player::Player,
    world::{ScheduledTickAccess, World},
};

/// Behavior for Anvils
#[block_behavior]
pub struct AnvilBlock {
    block: BlockRef,
}

const FACING: &EnumProperty<Direction> = &BlockStateProperties::FACING;
const HORIZONTAL_FACING: &EnumProperty<Direction> = &BlockStateProperties::HORIZONTAL_FACING;

/// Vanilla `AnvilBlock.FALL_DAMAGE_PER_DISTANCE`.
const FALL_DAMAGE_PER_DISTANCE: f32 = 2.0;
/// Vanilla `AnvilBlock.FALL_DAMAGE_MAX`.
const FALL_DAMAGE_MAX: i32 = 40;

impl AnvilBlock {
    /// Creates a new Anvil Block Behavior
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Returns the next damage level block state for anvil `BlockStates`
    #[must_use]
    pub fn damage(state: BlockStateId) -> Option<BlockStateId> {
        let block = state.get_block();
        if block == &vanilla_blocks::ANVIL {
            Some(
                vanilla_blocks::CHIPPED_ANVIL
                    .default_state()
                    .copy_value(FACING, &state),
            )
        } else if block == &vanilla_blocks::CHIPPED_ANVIL {
            Some(
                vanilla_blocks::DAMAGED_ANVIL
                    .default_state()
                    .copy_value(FACING, &state),
            )
        } else {
            None
        }
    }
}

impl Fallable for AnvilBlock {
    /// Vanilla `AnvilBlock.onLand`: the anvil land level event.
    fn on_land(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        _state: BlockStateId,
        _replaced_state: BlockStateId,
        entity: &FallingBlockEntity,
    ) {
        if !entity.is_silent() {
            world.level_event(vanilla_level_events::SOUND_ANVIL_LAND, pos, 0, None);
        }
    }

    /// Vanilla `AnvilBlock.onBrokenAfterFall`: the anvil broken level event.
    fn on_broken_after_fall(&self, world: &Arc<World>, pos: BlockPos, entity: &FallingBlockEntity) {
        if !entity.is_silent() {
            world.level_event(vanilla_level_events::SOUND_ANVIL_BROKEN, pos, 0, None);
        }
    }

    /// Vanilla `AnvilBlock.getFallDamageSource`: `minecraft:falling_anvil`.
    fn get_fall_damage_source(&self, entity: &Arc<FallingBlockEntity>) -> DamageSource {
        DamageSource::direct(
            &vanilla_damage_types::FALLING_ANVIL,
            Arc::<FallingBlockEntity>::clone(entity),
        )
    }
}

impl BlockBehavior for AnvilBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(self.block.default_state().set_value(
            HORIZONTAL_FACING,
            context.horizontal_direction().rotate_y_clockwise(),
        ))
    }

    fn use_without_item(
        &self,
        _state: BlockStateId,
        _world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        let inventory = Arc::clone(&player.inventory);
        player.open_menu(
            TextComponent::translated(translations::CONTAINER_REPAIR.msg()),
            move |context| anvil(inventory, context.container_id, pos, context.world),
        );
        player.award_custom_stat(&vanilla_custom_stats::INTERACT_WITH_ANVIL);
        InteractionResult::Success
    }

    /// Vanilla `FallingBlock.onPlace`: schedule the two-tick fall check.
    fn on_place(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _old_state: BlockStateId,
        _moved_by_piston: bool,
    ) {
        FallingBlock::new(self.block).on_place(world, pos);
    }

    /// Vanilla `FallingBlock.updateShape`: re-check the fall on every shape update.
    fn update_shape(
        &self,
        state: BlockStateId,
        ticks: &dyn ScheduledTickAccess,
        pos: BlockPos,
        _direction: Direction,
        _neighbor_pos: BlockPos,
        _neighbor_state: BlockStateId,
    ) -> BlockStateId {
        FallingBlock::new(self.block).update_shape(state, ticks, pos)
    }

    /// Vanilla `FallingBlock.tick`: spawn the falling entity.
    ///
    /// `AnvilBlock.falling` additionally makes the impact hurt entities with
    /// `FALL_DAMAGE_PER_DISTANCE` per block, capped at `FALL_DAMAGE_MAX`.
    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        if let Some(entity) = FallingBlock::tick(state, world, pos) {
            entity.set_hurts_entities(FALL_DAMAGE_PER_DISTANCE, FALL_DAMAGE_MAX);
        }
    }

    fn as_fallable(&self) -> Option<&dyn Fallable> {
        Some(self)
    }

    /// Vanilla `AnvilBlock.isPathfindable`.
    fn is_pathfindable(
        &self,
        _state: BlockStateId,
        _computation_type: PathComputationType,
    ) -> bool {
        false
    }
}
