//! Beehive block behavior implementation.
//!
//! Mirrors vanilla `BeehiveBlock`: facing placement, shears/glass-bottle
//! harvesting at full honey level, bee release on destruction, the
//! `bee_nest_destroyed` criterion, fire-driven shape updates, and the analog
//! output signal. Occupant storage, ticking and release live in
//! [`BeehiveBlockEntity`].
//!
//! Deviations:
//! - Vanilla `BlockItem.updateBlockStateFromTag` applies the `minecraft:block_state`
//!   item component to the placement state for every block; Steel's
//!   `BlockItem` does not (owner: `behavior/items/block_item.rs`), so
//!   [`BeehiveBlock::get_state_for_placement`] applies the honey level itself.
//! - Vanilla's `minecraft:blocks/beehive` loot table copies `minecraft:bees` and
//!   `honey_level` for silk touch through `minecraft:copy_components` /
//!   `minecraft:copy_state`; Steel's `ItemStack::copy_components` and
//!   `ItemStack::copy_block_state` are no-ops (owner: `steel-registry/src/item_stack.rs`),
//!   so `get_drops` supplies the silk-touch item directly.
//! - Vanilla `BeehiveBlock.onExplosionHit` calls `angerNearbyBees`; Steel routes
//!   explosion drops through `BlockBehavior::get_drops`, which performs the
//!   vanilla `getDrops` explosion release, so the angering side effect is not
//!   repeated there (owner: `world/explosion.rs`).

use std::collections::BTreeMap;
use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::advancement::triggers::TriggerEvent;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::blocks::properties::{
    BlockStateProperties, Direction, EnumProperty, IntProperty, Property,
};
use steel_registry::data_components::components::BlockItemStateProperties;
use steel_registry::data_components::vanilla_components::{BEES, BLOCK_STATE};
use steel_registry::item_stack::ItemStack;
use steel_registry::items::item::BlockHitResult;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::{
    REGISTRY, RegistryExt, TaggedRegistryExt, sound_events, vanilla_block_entity_types,
    vanilla_game_events, vanilla_items, vanilla_loot_tables,
};
use steel_utils::types::{InteractionHand, UpdateFlags};
use steel_utils::{BlockPos, BlockStateId, Downcast, Identifier, WorldAabb};

use crate::behavior::block::drop_from_block_interact_loot_table;
use crate::behavior::{
    BlockBehavior, BlockEntityCreation, BlockLootContext, BlockPlaceContext, InteractionResult,
    InventoryAccess,
};
use crate::block_entity::entities::BeehiveBlockEntity;
use crate::block_entity::{BLOCK_ENTITIES, BlockEntityTicker};
use crate::entity::entities::BeeEntity;
use crate::entity::{Entity, Mob, living_loot_equipment};
use crate::player::Player;
use crate::player::advancement::triggers::{WorldLevelView, event_base, player_view, raise};
use crate::player::player_inventory::Prediction;
use crate::world::game_event::GameEventContext;
use crate::world::{LevelReader, ScheduledTickAccess, World};

/// Behavior for beehive and bee nest blocks.
#[block_behavior]
pub struct BeehiveBlock {
    block: BlockRef,
}

const HORIZONTAL_FACING: &EnumProperty<Direction> = &BlockStateProperties::HORIZONTAL_FACING;
const LEVEL_HONEY: &IntProperty = &BlockStateProperties::LEVEL_HONEY;
/// Vanilla `BeehiveBlock.MAX_HONEY_LEVELS`.
const MAX_HONEY_LEVELS: u8 = 5;

impl BeehiveBlock {
    /// Creates a new beehive block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Vanilla `BeehiveBlock.angerNearbyBees`.
    fn anger_nearby_bees(world: &Arc<World>, pos: BlockPos) {
        let area = WorldAabb::new(
            f64::from(pos.x()),
            f64::from(pos.y()),
            f64::from(pos.z()),
            f64::from(pos.x()) + 1.0,
            f64::from(pos.y()) + 1.0,
            f64::from(pos.z()) + 1.0,
        )
        .inflate_xyz(8.0, 6.0, 8.0);

        let bees = world.get_entities_in_aabb_matching(&area, |entity| {
            entity.downcast_ref::<BeeEntity>().is_some()
        });
        if bees.is_empty() {
            return;
        }

        let players =
            world.get_entities_in_aabb_matching(&area, |entity| entity.as_player().is_some());
        if players.is_empty() {
            return;
        }

        for bee in bees {
            let Some(mob) = bee.as_mob() else {
                continue;
            };
            if mob.target().is_some() {
                continue;
            }
            let target = &players[rand::random_range(0..players.len())];
            let _ = mob.set_target(Some(target));
        }
    }

    /// Vanilla `BeehiveBlock.hiveContainsBees`.
    fn hive_contains_bees(world: &Arc<World>, pos: BlockPos) -> bool {
        world
            .get_block_entity(pos)
            .and_then(|entity| {
                entity
                    .downcast_ref::<BeehiveBlockEntity>()
                    .map(|hive| !hive.is_empty())
            })
            .unwrap_or(false)
    }

    /// Vanilla `BeehiveBlock.releaseBeesAndResetHoneyLevel`.
    ///
    /// Vanilla only calls this with `BeeReleaseStatus.EMERGENCY`.
    fn release_bees_and_reset_honey_level(
        &self,
        world: &Arc<World>,
        state: BlockStateId,
        pos: BlockPos,
        player: Option<&Player>,
    ) {
        self.reset_honey_level(world, state, pos);
        if let Some(hive) = world
            .get_block_entity(pos)
            .and_then(|entity| entity.downcast_ref::<BeehiveBlockEntity>())
        {
            hive.empty_all_living_from_hive(player, state, BeehiveBlockEntity::EMERGENCY);
        }
    }

    /// Vanilla `BeehiveBlock.resetHoneyLevel`.
    fn reset_honey_level(&self, world: &Arc<World>, state: BlockStateId, pos: BlockPos) {
        world.set_block(
            pos,
            state.set_value(LEVEL_HONEY, 0),
            UpdateFlags::UPDATE_ALL,
        );
    }

    /// Vanilla `BeehiveBlock.dropHoneycomb` (the `harvest/beehive` interact table).
    fn drop_honeycomb(
        world: &Arc<World>,
        state: BlockStateId,
        pos: BlockPos,
        player: &Player,
        tool: &ItemStack,
    ) {
        let mut rng = rand::rng();
        let drops = drop_from_block_interact_loot_table(
            &vanilla_loot_tables::HARVEST_BEEHIVE,
            state,
            world.get_block_entity(pos),
            Some(tool),
            Some(player),
            &mut rng,
        );
        for drop in drops {
            world.pop_resource(pos, drop);
        }
    }

    /// The `minecraft:block_state` component vanilla attaches to picked beehives.
    fn honey_level_component(honey_level: u8) -> BlockItemStateProperties {
        BlockItemStateProperties::new(BTreeMap::from([(
            LEVEL_HONEY.get_name().to_owned(),
            honey_level.to_string(),
        )]))
    }
}

/// Vanilla `EnchantmentHelper.hasTag(tool, PREVENTS_BEE_SPAWNS_WHEN_MINING)`.
fn prevents_bee_spawns_when_mining(tool: &ItemStack) -> bool {
    let Some(enchantments) = tool.get_enchantments() else {
        return false;
    };
    enchantments.iter().any(|(key, _)| {
        REGISTRY.enchantments.by_key(key).is_some_and(|ench| {
            REGISTRY.enchantments.is_in_tag(
                ench,
                &steel_registry::vanilla_enchantment_tags::EnchantmentTag::PREVENTS_BEE_SPAWNS_WHEN_MINING,
            )
        })
    })
}

impl BlockBehavior for BeehiveBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        let mut state = self
            .block
            .default_state()
            .set_value(HORIZONTAL_FACING, context.horizontal_direction().opposite());

        // Vanilla `BlockItem.updateBlockStateFromTag` applies the item's
        // `minecraft:block_state` component; see the module docs.
        let honey_level = context.source().with_item(|item| {
            item.get(BLOCK_STATE)
                .and_then(|properties| properties.get(LEVEL_HONEY.get_name()))
                .and_then(|value| LEVEL_HONEY.get_value(value))
        });
        if let Some(honey_level) = honey_level {
            state = state.set_value(LEVEL_HONEY, honey_level);
        }

        Some(state)
    }

    fn use_item_on(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        hand: InteractionHand,
        _hit_result: &BlockHitResult,
        inv: &mut InventoryAccess,
    ) -> InteractionResult {
        let honey_level = state.get_value(LEVEL_HONEY);
        let mut hive_emptied = false;
        let mut used_item = None;

        if honey_level >= MAX_HONEY_LEVELS {
            let shears = inv.with_item(|stack| stack.is(&vanilla_items::SHEARS));
            let glass_bottle = inv.with_item(|stack| stack.is(&vanilla_items::GLASS_BOTTLE));

            if shears {
                let tool = inv.with_item(|stack| stack.clone());
                Self::drop_honeycomb(world, state, pos, player, &tool);
                world.play_sound_at(
                    &sound_events::BLOCK_BEEHIVE_SHEAR,
                    SoundSource::Blocks,
                    player.position(),
                    1.0,
                    1.0,
                    None,
                );
                let has_infinite_materials = player.has_infinite_materials();
                inv.with_item(|stack| stack.hurt_and_break(1, has_infinite_materials));
                hive_emptied = true;
                world.game_event(
                    &vanilla_game_events::SHEAR,
                    pos,
                    &GameEventContext::new(Some(player), None),
                );
                used_item = Some(&*vanilla_items::SHEARS);
            } else if glass_bottle {
                let honey_bottle = ItemStack::new(&vanilla_items::HONEY_BOTTLE);
                let overflow = inv.with_inventory(|inventory| {
                    inventory.shrink_item_in_hand(hand, 1);
                    if inventory.get_item_in_hand(hand).is_empty() {
                        inventory.set_item_in_hand(hand, honey_bottle);
                        ItemStack::empty()
                    } else {
                        inventory.add_or_return(honey_bottle)
                    }
                });
                if !overflow.is_empty() {
                    let _ = player.drop_item(overflow, false, Prediction::Predicted);
                }

                world.play_sound_at(
                    &sound_events::ITEM_BOTTLE_FILL,
                    SoundSource::Blocks,
                    player.position(),
                    1.0,
                    1.0,
                    Some(player.id()),
                );
                hive_emptied = true;
                world.game_event(
                    &vanilla_game_events::FLUID_PICKUP,
                    pos,
                    &GameEventContext::new(Some(player), None),
                );
                used_item = Some(&*vanilla_items::GLASS_BOTTLE);
            }

            if hive_emptied && let Some(item) = used_item {
                player.award_stat(&vanilla_stat_types::ITEM_USED, item);
            }
        }

        if !hive_emptied {
            return InteractionResult::TryEmptyHandInteraction;
        }

        if !BeehiveBlockEntity::is_smokey_pos(world, pos) {
            if Self::hive_contains_bees(world, pos) {
                Self::anger_nearby_bees(world, pos);
            }
            self.release_bees_and_reset_honey_level(world, state, pos, Some(player));
        } else {
            self.reset_honey_level(world, state, pos);
        }

        InteractionResult::Success
    }

    fn player_destroy(
        &self,
        world: &Arc<World>,
        player: &Player,
        pos: BlockPos,
        state: BlockStateId,
        block_entity: Option<&crate::block_entity::SharedBlockEntity>,
        tool: &ItemStack,
    ) {
        let Some(hive) =
            block_entity.and_then(|entity| entity.downcast_ref::<BeehiveBlockEntity>())
        else {
            return;
        };

        if !prevents_bee_spawns_when_mining(tool) {
            hive.empty_all_living_from_hive(Some(player), state, BeehiveBlockEntity::EMERGENCY);
            world.update_neighbor_shapes_at(state, pos, UpdateFlags::UPDATE_ALL, 512);
            Self::anger_nearby_bees(world, pos);
        }

        // Vanilla `CriteriaTriggers.BEE_NEST_DESTROYED.trigger`.
        let occupant_count = i32::try_from(hive.occupant_count()).unwrap_or(i32::MAX);
        let origin = player.block_position();
        let level = WorldLevelView::new(world, origin);
        let equipment = living_loot_equipment(player);
        let known = player.recipe_book().unlocked_identifiers().to_vec();
        let view = player_view(player, &level, Some(&equipment), &known);
        raise(
            player,
            &Identifier::vanilla_static("bee_nest_destroyed"),
            &TriggerEvent::BeeNestDestroyed {
                base: event_base(&view, &level, origin),
                state,
                tool,
                bees_inside: occupant_count,
            },
        );
    }

    fn player_will_destroy(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
    ) -> BlockStateId {
        // Vanilla `BeehiveBlock.playerWillDestroy`: creative breaking drops the
        // hive with its occupants and honey level.
        if player.prevents_block_drops()
            && world.get_game_rule(&steel_registry::vanilla_game_rules::BLOCK_DROPS)
            && let Some(hive) = world
                .get_block_entity(pos)
                .and_then(|entity| entity.downcast_ref::<BeehiveBlockEntity>())
        {
            let honey_level = state.get_value(LEVEL_HONEY);
            if !hive.is_empty() || honey_level > 0 {
                let mut item = ItemStack::new(REGISTRY.items.by_block(self.block));
                item.set(BEES, hive.collect_bees_component());
                item.set(BLOCK_STATE, Self::honey_level_component(honey_level));
                world.pop_resource(pos, item);
            }
        }

        state
    }

    fn get_drops(
        &self,
        state: BlockStateId,
        context: &BlockLootContext<'_>,
    ) -> Option<Vec<ItemStack>> {
        let world = context.world();
        let pos = context.pos();

        // Vanilla `BeehiveBlock.getDrops`: explosives release the occupants.
        let entity = context.entity();
        if entity.is_some_and(is_destroying_explosive)
            && let Some(hive) = world
                .get_block_entity(pos)
                .and_then(|block_entity| block_entity.downcast_ref::<BeehiveBlockEntity>())
        {
            hive.empty_all_living_from_hive(None, state, BeehiveBlockEntity::EMERGENCY);
        }

        // Vanilla's loot table is `beehive` with a silk-touch branch that copies
        // the bees and honey level; Steel's loot copy functions are no-ops (see
        // the module docs), so the silk-touch branch is built here.
        let silk_touch = context.tool().is_some_and(prevents_bee_spawns_when_mining);
        if !silk_touch {
            return None;
        }

        let mut item = ItemStack::new(REGISTRY.items.by_block(self.block));
        if let Some(hive) = world
            .get_block_entity(pos)
            .and_then(|block_entity| block_entity.downcast_ref::<BeehiveBlockEntity>())
        {
            item.set(BEES, hive.collect_bees_component());
        }
        item.set(
            BLOCK_STATE,
            Self::honey_level_component(state.get_value(LEVEL_HONEY)),
        );
        Some(vec![item])
    }

    fn get_clone_item_stack(
        &self,
        block: BlockRef,
        state: BlockStateId,
        _block_entity: Option<crate::block_entity::SharedBlockEntity>,
        include_data: bool,
    ) -> Option<ItemStack> {
        let mut item = ItemStack::new(REGISTRY.items.by_block(block));
        if include_data {
            item.set(
                BLOCK_STATE,
                Self::honey_level_component(state.get_value(LEVEL_HONEY)),
            );
        }
        Some(item)
    }

    fn update_shape(
        &self,
        state: BlockStateId,
        world: &dyn ScheduledTickAccess,
        pos: BlockPos,
        _direction: Direction,
        neighbor_pos: BlockPos,
        _neighbor_state: BlockStateId,
    ) -> BlockStateId {
        let neighbor = world.get_block_state(neighbor_pos).get_block();
        if (std::ptr::eq(neighbor, &steel_registry::vanilla_blocks::FIRE)
            || std::ptr::eq(neighbor, &steel_registry::vanilla_blocks::SOUL_FIRE))
            && let Some(hive) = world
                .get_block_entity(pos)
                .and_then(|block_entity| block_entity.downcast_ref::<BeehiveBlockEntity>())
        {
            hive.empty_all_living_from_hive(None, state, BeehiveBlockEntity::EMERGENCY);
        }

        state
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
            &vanilla_block_entity_types::BEEHIVE,
            level,
            pos,
            state,
        ))
    }

    fn get_block_entity_ticker(
        &self,
        _world: &Arc<World>,
        _state: BlockStateId,
        block_entity_type: BlockEntityTypeRef,
    ) -> Option<BlockEntityTicker> {
        BlockEntityTicker::for_matching_entity_tick(
            block_entity_type,
            &vanilla_block_entity_types::BEEHIVE,
        )
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
        state.get_value(LEVEL_HONEY).into()
    }
}

/// Vanilla `BeehiveBlock.getDrops`'s explosive destroyer check.
fn is_destroying_explosive(entity: &dyn Entity) -> bool {
    use steel_registry::vanilla_entities;
    let entity_type = entity.entity_type();
    std::ptr::eq(entity_type, &vanilla_entities::TNT)
        || std::ptr::eq(entity_type, &vanilla_entities::CREEPER)
        || std::ptr::eq(entity_type, &vanilla_entities::WITHER_SKULL)
        || std::ptr::eq(entity_type, &vanilla_entities::WITHER)
        || std::ptr::eq(entity_type, &vanilla_entities::TNT_MINECART)
}
