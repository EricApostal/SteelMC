//! `DecoratedPotBlock` behavior (`net.minecraft.world.level.block.DecoratedPotBlock`).
//!
//! Deviation: vanilla `DecoratedPotBlock.getSoundType` returns `SoundType.DECORATED_POT_CRACKED`
//! for a cracked pot; Steel lacks a per-state sound-type hook and resolves one registry sound
//! type per block (owner: `steel-registry` block config / `BlockBehavior`).
//! Deviation: vanilla's loot table copies `minecraft:pot_decorations` from the block entity with
//! `minecraft:copy_components`; Steel lacks block-entity plumbing in
//! `World::default_block_drops` (owner: `steel-core/src/world/level_effects.rs`).
//! Deviation: vanilla's loot table emits the dynamic `minecraft:sherds` entry; Steel lacks a
//! `LootEntry::Dynamic` implementation (owner: `steel-registry/src/loot_table/entries.rs`), so
//! [`DecoratedPotBlock::get_drops`] supplies the sherds itself.
//!
//! Vanilla `DecoratedPotBlock.rotate`/`mirror` have no `BlockBehavior` counterpart; Steel rotates
//! `HORIZONTAL_FACING` generically in `StructureTemplate::transform_state`.

use std::sync::{Arc, Weak};

use glam::DVec3;
use steel_macros::block_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{
    BlockStateProperties, BoolProperty, Direction, EnumProperty,
};
use steel_registry::data_components::vanilla_components::{POT_DECORATIONS, PotDecorations};
use steel_registry::item_stack::ItemStack;
use steel_registry::particle_type::ParticleData;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::vanilla_enchantment_tags::EnchantmentTag;
use steel_registry::{
    ItemStackTemplate, REGISTRY, RegistryExt as _, TaggedRegistryExt as _, sound_events,
    vanilla_block_entity_types, vanilla_game_events, vanilla_item_tags, vanilla_particle_types,
};
use steel_utils::types::{InteractionHand, UpdateFlags};
use steel_utils::{BlockPos, BlockStateId, Downcast as _};

use crate::behavior::block::schedule_water_tick_if_waterlogged;
use crate::behavior::{
    BlockBehavior, BlockEntityCreation, BlockHitResult, BlockLootContext, BlockPlaceContext,
    InteractionResult, InventoryAccess,
};
use crate::block_entity::entities::DecoratedPotBlockEntity;
use crate::block_entity::{BLOCK_ENTITIES, BlockEntityTicker, SharedBlockEntity};
use crate::entity::Entity;
use crate::entity::ai::path::PathComputationType;
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

/// Vanilla `EnchantmentHelper.hasTag(stack, EnchantmentTags.PREVENTS_DECORATED_POT_SHATTERING)`:
/// silk touch keeps the pot intact when it is broken by a shattering item.
fn prevents_shattering(stack: &ItemStack) -> bool {
    let Some(enchantments) = stack.get_enchantments() else {
        return false;
    };
    enchantments.iter().any(|(key, _)| {
        REGISTRY.enchantments.by_key(key).is_some_and(|ench| {
            REGISTRY
                .enchantments
                .is_in_tag(ench, &EnchantmentTag::PREVENTS_DECORATED_POT_SHATTERING)
        })
    })
}

/// Vanilla `DecoratedPotBlock.getDrops`' dynamic `minecraft:sherds` drop, in vanilla order:
/// left, back, front, right.
fn sherd_drops(pot: &DecoratedPotBlockEntity) -> Vec<ItemStack> {
    // `SHERD_SLOT_NAMES` is back, left, right, front.
    [1, 0, 3, 2]
        .into_iter()
        .filter_map(|index| {
            let sherd = pot.sherd(index);
            (!sherd.is_empty()).then_some(sherd)
        })
        .collect()
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

    /// Vanilla `DecoratedPotBlock.useItemOn`: stores a matching item in the pot, plays the insert
    /// sound with a pitch derived from the resulting stack size, and puffs `DUST_PLUME`.
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

        let pot_item = pot.item();
        let inserted_item = inv.with_item(|stack| stack.item());
        let can_insert = inv.with_item(|stack| {
            !stack.is_empty()
                && (pot_item.is_empty()
                    || (ItemStack::is_same_item_same_components(&pot_item, stack)
                        && pot_item.count() < pot_item.max_stack_size()))
        });
        if !can_insert {
            return InteractionResult::TryEmptyHandInteraction;
        }

        pot.wobble_positive();
        player.award_stat(&vanilla_stat_types::ITEM_USED, inserted_item);

        let has_infinite_materials = player.has_infinite_materials();
        let awarded_item =
            inv.with_item(|stack| stack.consume_and_return(1, has_infinite_materials));

        let (stored_item, pitch_bend) = if pot_item.is_empty() {
            let pitch = stack_count_ratio(&awarded_item);
            (awarded_item, pitch)
        } else {
            let mut grown = pot_item;
            grown.grow(1);
            let pitch = stack_count_ratio(&grown);
            (grown, pitch)
        };
        pot.set_item(stored_item);

        world.play_sound(
            &sound_events::BLOCK_DECORATED_POT_INSERT,
            SoundSource::Blocks,
            pos,
            1.0,
            0.7 + 0.5 * pitch_bend,
            None,
        );
        let (center_x, _, center_z) = pos.get_center();
        world.send_particles(
            ParticleData::simple(&vanilla_particle_types::DUST_PLUME),
            DVec3::new(center_x, f64::from(pos.y()) + 1.2, center_z),
            7,
            DVec3::ZERO,
            0.0,
        );
        world.game_event(
            &vanilla_game_events::BLOCK_CHANGE,
            pos,
            &GameEventContext::new(Some(player as &dyn Entity), None),
        );
        InteractionResult::Success
    }

    /// Vanilla `DecoratedPotBlock.useWithoutItem`: shakes the pot and reports the failed insert.
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

    /// Vanilla `DecoratedPotBlock.playerWillDestroy`: a shattering tool cracks the pot so its loot
    /// table shatters it into sherds, unless the tool carries
    /// `minecraft:prevents_decorated_pot_shattering`.
    fn player_will_destroy(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
    ) -> BlockStateId {
        let inventory = player.inventory.lock();
        let destroyed_with = inventory.get_item_in_hand(InteractionHand::MainHand);
        if !destroyed_with
            .item()
            .has_tag(&vanilla_item_tags::ItemTag::BREAKS_DECORATED_POTS)
            || prevents_shattering(&destroyed_with)
        {
            return state;
        }

        let next_state = state.set_value(CRACKED, true);
        // Vanilla `Level.setBlock(pos, nextState, 260)` = UPDATE_INVISIBLE |
        // UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS.
        world.set_block(pos, next_state, UpdateFlags::UPDATE_NONE);
        next_state
    }

    /// Vanilla `DecoratedPotBlock.getDrops`' dynamic `minecraft:sherds` drop for cracked pots;
    /// intact pots fall through to the block's loot table.
    fn get_drops(
        &self,
        state: BlockStateId,
        context: &BlockLootContext<'_>,
    ) -> Option<Vec<ItemStack>> {
        if !state.get_value(CRACKED) {
            return None;
        }
        if let Some(pot) = context
            .block_entity()
            .and_then(|block_entity| block_entity.downcast_ref::<DecoratedPotBlockEntity>())
        {
            return Some(sherd_drops(pot));
        }
        // Destruction paths that drop loot before removing the block (for example
        // `World::destroy_block`) do not hand the block entity to the loot context.
        // Vanilla's `alternatives` entry picks the dynamic `sherds` drop for a cracked pot
        // regardless, so a missing block entity yields no items rather than the pot item.
        let block_entity = context.world().get_block_entity(context.pos());
        Some(
            block_entity
                .as_ref()
                .and_then(|block_entity| block_entity.downcast_ref::<DecoratedPotBlockEntity>())
                .map_or_else(Vec::new, sherd_drops),
        )
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
                UpdateFlags::UPDATE_NONE,
            );
            world.destroy_block_by_entity(pos, true, projectile.as_entity_event_source());
        }
    }

    /// Vanilla `DecoratedPotBlock.getCloneItemStack`: the picked pot carries its decorations.
    fn get_clone_item_stack(
        &self,
        block: BlockRef,
        _state: BlockStateId,
        block_entity: Option<SharedBlockEntity>,
        _include_data: bool,
    ) -> Option<ItemStack> {
        let mut item = ItemStack::new(REGISTRY.items.by_block(block));
        let Some(pot) = block_entity
            .as_ref()
            .and_then(|block_entity| block_entity.downcast_ref::<DecoratedPotBlockEntity>())
        else {
            return Some(item);
        };

        let decoration = |index: usize| ItemStackTemplate::from_stack(&pot.sherd(index)).ok();
        item.set(
            POT_DECORATIONS,
            PotDecorations::new(decoration(0), decoration(1), decoration(2), decoration(3)),
        );
        Some(item)
    }

    /// Vanilla `DecoratedPotBlock.affectNeighborsAfterRemoval`:
    /// `Containers.updateNeighboursAfterDestroy` refreshes comparators reading the pot.
    fn affect_neighbors_after_removal(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _moved_by_piston: bool,
    ) {
        world.update_neighbor_for_output_signal(pos, self.block);
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

    /// Vanilla `DecoratedPotBlock.isPathfindable`: never pathfindable.
    fn is_pathfindable(
        &self,
        _state: BlockStateId,
        _computation_type: PathComputationType,
    ) -> bool {
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

/// Vanilla's insert-sound pitch input: `stack.getCount() / stack.getMaxStackSize()`.
fn stack_count_ratio(stack: &ItemStack) -> f32 {
    stack.count() as f32 / stack.max_stack_size() as f32
}
