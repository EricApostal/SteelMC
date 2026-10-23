//! Vanilla composter behavior.
//!
//! Vanilla `ComposterBlock` has two independent halves: the comparator-visible
//! fill level in the block state, and the `useItemOn` / `useWithoutItem`
//! interactions that raise the level from compostable items and empty a ready
//! composter into bone meal.

use std::sync::Arc;

use rand::Rng as _;
use steel_macros::block_behavior;
use steel_registry::blocks::{
    BlockRef,
    block_state_ext::BlockStateExt as _,
    properties::{BlockStateProperties, Direction, IntProperty},
};
use steel_registry::data_components::vanilla_components;
use steel_registry::items::item::BlockHitResult;
use steel_registry::loot_table::LootContext;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::{
    REGISTRY, sound_events, vanilla_game_events, vanilla_items, vanilla_level_events,
};
use steel_utils::types::{InteractionHand, UpdateFlags};
use steel_utils::{BlockPos, BlockStateId, Identifier};

use crate::behavior::{BlockBehavior, BlockPlaceContext, InteractionResult, InventoryAccess};
use crate::entity::ai::path::PathComputationType;
use crate::entity::{Entity, entity_loot_ref, loot_equipment};
use crate::player::Player;
use crate::world::game_event::GameEventContext;
use crate::world::{LevelReader, World};

/// Vanilla `ComposterBlock.READY`: the level that holds a finished bone meal.
pub const COMPOSTER_READY: i32 = 8;
/// Vanilla `ComposterBlock.MAX_LEVEL`: the highest "filling" level.
pub const COMPOSTER_MAX_LEVEL: i32 = 7;

/// Vanilla composter behavior.
#[block_behavior]
pub struct ComposterBlock {
    block: BlockRef,
}

const LEVEL_COMPOSTER: &IntProperty = &BlockStateProperties::LEVEL_COMPOSTER;

impl ComposterBlock {
    /// Creates composter behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Vanilla `ComposterBlock.addLayer`: resolves the item's `compostable`
    /// component chance and raises the level, scheduling the ready tick when the
    /// composter reaches [`COMPOSTER_MAX_LEVEL`].
    fn add_layer(
        source_entity: Option<&dyn Entity>,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        layers_key: &Identifier,
    ) -> BlockStateId {
        let fill_level = state.get_value(LEVEL_COMPOSTER);
        let Some(layers_to_add) = Self::sample_layers(state, source_entity, layers_key) else {
            return state;
        };
        if layers_to_add <= 0 {
            return state;
        }

        let new_level = (fill_level + layers_to_add).clamp(0, COMPOSTER_MAX_LEVEL);
        let new_state = state.set_value(LEVEL_COMPOSTER, new_level);
        world.set_block(pos, new_state, UpdateFlags::UPDATE_ALL);
        world.game_event(
            &vanilla_game_events::BLOCK_CHANGE,
            pos,
            &GameEventContext::new(source_entity, Some(new_state)),
        );
        if new_level == COMPOSTER_MAX_LEVEL {
            world.schedule_block_tick_default(pos, state.get_block(), 20);
        }
        new_state
    }

    /// Samples the `compostable` component's context int provider.
    ///
    /// The providers vanilla ships dispatch on the block state only, so the loot
    /// context carries the state and the interacting entity.
    fn sample_layers(
        state: BlockStateId,
        source_entity: Option<&dyn Entity>,
        layers_key: &Identifier,
    ) -> Option<i32> {
        let entry = REGISTRY.context_int_providers.by_key(layers_key)?;
        let mut rng = rand::rng();
        let mut ctx = LootContext::new(&mut rng).with_block_state(state);
        if let Some(source_entity) = source_entity {
            let equipment = loot_equipment(source_entity);
            ctx = ctx.with_interacting_entity(entity_loot_ref(source_entity, equipment.as_ref()));
        }
        Some(entry.provider.get_int(&mut ctx))
    }

    /// Vanilla `ComposterBlock.empty`: resets the level and publishes the change.
    fn empty(
        source_entity: Option<&dyn Entity>,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
    ) -> BlockStateId {
        let new_state = state.set_value(LEVEL_COMPOSTER, 0);
        world.set_block(pos, new_state, UpdateFlags::UPDATE_ALL);
        world.game_event(
            &vanilla_game_events::BLOCK_CHANGE,
            pos,
            &GameEventContext::new(source_entity, Some(new_state)),
        );
        new_state
    }

    /// Vanilla `ComposterBlock.extractProduce`: drops one bone meal and empties
    /// the composter.
    fn extract_produce(
        source_entity: Option<&dyn Entity>,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
    ) -> BlockStateId {
        let mut rng = rand::rng();
        let (x_offset, z_offset) = (
            rng.random_range(-0.7_f32..=0.7_f32),
            rng.random_range(-0.7_f32..=0.7_f32),
        );
        world.spawn_item(
            glam::DVec3::new(
                f64::from(pos.x()) + 0.5 + f64::from(x_offset),
                f64::from(pos.y()) + 1.01,
                f64::from(pos.z()) + 0.5 + f64::from(z_offset),
            ),
            steel_registry::item_stack::ItemStack::new(&vanilla_items::BONE_MEAL),
        );

        let new_state = Self::empty(source_entity, state, world, pos);
        world.play_block_sound(&sound_events::COMPOSTER_EMPTY, pos, 1.0, 1.0, None);
        new_state
    }
}

impl BlockBehavior for ComposterBlock {
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(self.block.default_state())
    }

    /// Vanilla `ComposterBlock.onPlace`: a composter placed at level 7 finishes
    /// its cycle after 20 ticks.
    fn on_place(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _old_state: BlockStateId,
        _moved_by_piston: bool,
    ) {
        if state.get_value(LEVEL_COMPOSTER) == COMPOSTER_MAX_LEVEL {
            world.schedule_block_tick_default(pos, self.block, 20);
        }
    }

    /// Vanilla `ComposterBlock.useItemOn`.
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
        let fill_level = state.get_value(LEVEL_COMPOSTER);
        let Some(layers_key) = inv.with_item(|stack| {
            stack
                .get(vanilla_components::COMPOSTABLE)
                .map(|compostable| compostable.layers.clone())
        }) else {
            return InteractionResult::TryEmptyHandInteraction;
        };

        if fill_level >= COMPOSTER_READY {
            return InteractionResult::TryEmptyHandInteraction;
        }

        if fill_level < COMPOSTER_MAX_LEVEL {
            let new_state = Self::add_layer(Some(player), state, world, pos, &layers_key);
            world.level_event(
                vanilla_level_events::COMPOSTER_FILL,
                pos,
                i32::from(new_state != state),
                Some(player.id()),
            );
            let item_ref = inv.with_item(|stack| stack.item);
            player.award_stat(&vanilla_stat_types::ITEM_USED, item_ref);
            inv.with_item(|stack| stack.consume_one(player.has_infinite_materials()));
        }

        InteractionResult::Success
    }

    /// Vanilla `ComposterBlock.useWithoutItem`: a ready composter yields bone meal.
    fn use_without_item(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        if state.get_value(LEVEL_COMPOSTER) != COMPOSTER_READY {
            return InteractionResult::Pass;
        }

        Self::extract_produce(Some(player), state, world, pos);
        InteractionResult::Success
    }

    /// Vanilla `ComposterBlock.tick`: level 7 becomes level 8 (ready).
    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        if state.get_value(LEVEL_COMPOSTER) != COMPOSTER_MAX_LEVEL {
            return;
        }
        world.set_block(
            pos,
            state.set_value(LEVEL_COMPOSTER, COMPOSTER_READY),
            UpdateFlags::UPDATE_ALL,
        );
        world.play_block_sound(&sound_events::COMPOSTER_READY, pos, 1.0, 1.0, None);
    }

    fn has_analog_output_signal(&self, _state: BlockStateId) -> bool {
        true
    }

    fn get_analog_output_signal(
        &self,
        state: BlockStateId,
        _world: &dyn LevelReader,
        _pos: BlockPos,
        _direction: Direction,
    ) -> i32 {
        i32::from(state.get_value(LEVEL_COMPOSTER))
    }

    fn is_pathfindable(
        &self,
        _state: BlockStateId,
        _computation_type: PathComputationType,
    ) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, vanilla_blocks};

    use super::*;
    use crate::{
        behavior::{BLOCK_BEHAVIORS, init_behaviors},
        test_support::TestLevel,
    };

    #[test]
    fn registered_composter_outputs_its_full_state_level() {
        init_vanilla_registry();
        init_behaviors();
        let state = vanilla_blocks::COMPOSTER
            .default_state()
            .set_value(LEVEL_COMPOSTER, LEVEL_COMPOSTER.max);
        let behavior = BLOCK_BEHAVIORS.get_behavior(state.get_block());

        assert!(behavior.has_analog_output_signal(state));
        assert_eq!(
            behavior.get_analog_output_signal(
                state,
                &TestLevel::default(),
                BlockPos::ZERO,
                Direction::North,
            ),
            i32::from(LEVEL_COMPOSTER.max),
        );
    }
}
