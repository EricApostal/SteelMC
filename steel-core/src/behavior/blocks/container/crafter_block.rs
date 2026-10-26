//! Crafter block behavior.
//!
//! The crafter opens its 3x3 menu on use and runs a craft on the rising edge of
//! a redstone neighbor signal, mirroring Vanilla `CrafterBlock`.
//!
//! Deviation: Vanilla `CrafterBlock.dispenseFrom` calls
//! `ItemStack.onCraftedBySystem`, i.e. `Item.onCraftedPostProcess` (the filled
//! map's `map_post_processing` lock/scale); Steel has no crafted-post-process item
//! hook (owner: `behavior/item.rs`).

use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_registry::advancement::triggers::TriggerEvent;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{
    BlockStateProperties, BoolProperty, Direction, EnumProperty, FrontAndTop,
};
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_block_entity_types;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, Downcast as _, Identifier, WorldAabb, translations};
use text_components::TextComponent;

use crate::behavior::block::{BlockBehavior, BlockEntityCreation};
use crate::behavior::context::{BlockHitResult, BlockPlaceContext, InteractionResult};
use crate::behavior::{InventoryAccess, PlacementSource};
use crate::block_entity::entities::{CrafterBlockEntity, hopper_add_item, hopper_container_at};
use crate::block_entity::{BLOCK_ENTITIES, BlockEntityTicker};
use crate::entity::Entity as _;
use crate::entity::living_loot_equipment;
use crate::entity::projectile::triangle_random;
use crate::inventory::lock::{ContainerLockGuard, ContainerRef};
use crate::inventory::menu::kinds::crafter;
use crate::inventory::recipe_manager;
use crate::player::Player;
use crate::player::advancement::triggers::{WorldLevelView, event_base, player_view, raise};
use crate::world::{LevelReader, ScheduledTickAccess as _, SignalGetter as _, World};

const TRIGGERED: &BoolProperty = &BlockStateProperties::TRIGGERED;
const CRAFTING: &BoolProperty = &BlockStateProperties::CRAFTING;
const ORIENTATION: &EnumProperty<FrontAndTop> = &BlockStateProperties::ORIENTATION;

const MAX_CRAFTING_TICKS: i32 = 6;
const CRAFTING_TICK_DELAY: i32 = 4;

/// Vanilla `LevelEvent.SOUND_CRAFTER_CRAFT`.
const LEVEL_EVENT_SOUND_CRAFTER_CRAFT: i32 = 1049;
/// Vanilla `LevelEvent.SOUND_CRAFTER_FAIL`.
const LEVEL_EVENT_SOUND_CRAFTER_FAIL: i32 = 1050;
/// Vanilla `LevelEvent.PARTICLES_CRAFTER_CRAFT`.
const LEVEL_EVENT_PARTICLES_CRAFTER_CRAFT: i32 = 2010;

/// Behavior for the crafter block.
#[block_behavior]
pub struct CrafterBlock {
    block: BlockRef,
}

/// Vanilla `FrontAndTop.fromFrontAndTop`.
const fn from_front_and_top(front: Direction, top: Direction) -> FrontAndTop {
    match front {
        Direction::Down => match top {
            Direction::East => FrontAndTop::DownEast,
            Direction::South => FrontAndTop::DownSouth,
            Direction::West => FrontAndTop::DownWest,
            _ => FrontAndTop::DownNorth,
        },
        Direction::Up => match top {
            Direction::East => FrontAndTop::UpEast,
            Direction::South => FrontAndTop::UpSouth,
            Direction::West => FrontAndTop::UpWest,
            _ => FrontAndTop::UpNorth,
        },
        Direction::West => FrontAndTop::WestUp,
        Direction::East => FrontAndTop::EastUp,
        Direction::South => FrontAndTop::SouthUp,
        Direction::North => FrontAndTop::NorthUp,
    }
}

/// Vanilla `FrontAndTop.front`.
const fn orientation_front(orientation: FrontAndTop) -> Direction {
    match orientation {
        FrontAndTop::DownEast
        | FrontAndTop::DownNorth
        | FrontAndTop::DownSouth
        | FrontAndTop::DownWest => Direction::Down,
        FrontAndTop::UpEast | FrontAndTop::UpNorth | FrontAndTop::UpSouth | FrontAndTop::UpWest => {
            Direction::Up
        }
        FrontAndTop::WestUp => Direction::West,
        FrontAndTop::EastUp => Direction::East,
        FrontAndTop::NorthUp => Direction::North,
        FrontAndTop::SouthUp => Direction::South,
    }
}

impl CrafterBlock {
    /// Creates a new crafter block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Vanilla `CrafterBlock.setBlockEntityTriggered`.
    fn set_block_entity_triggered(world: &Arc<World>, pos: BlockPos, triggered: bool) {
        if let Some(block_entity) = world.get_block_entity(pos)
            && let Some(crafter) = block_entity.downcast_ref::<CrafterBlockEntity>()
        {
            crafter.set_triggered(triggered);
        }
    }

    /// Vanilla `CrafterBlock.dispenseFrom`.
    fn dispense_from(state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        let Some(block_entity) = world.get_block_entity(pos) else {
            return;
        };
        let Some(crafter) = block_entity.downcast_ref::<CrafterBlockEntity>() else {
            return;
        };

        let input = crafter.crafting_input();
        let Some(recipe) = recipe_manager::find_recipe(&input) else {
            world.level_event(LEVEL_EVENT_SOUND_CRAFTER_FAIL, pos, 0, None);
            return;
        };
        let positioned = input.as_positioned_input();
        let results = recipe_manager::assemble_recipe(recipe, &positioned.input);
        if results.is_empty() {
            world.level_event(LEVEL_EVENT_SOUND_CRAFTER_FAIL, pos, 0, None);
            return;
        }

        crafter.set_crafting_ticks_remaining(MAX_CRAFTING_TICKS);
        world.set_block(
            pos,
            state.set_value(CRAFTING, true),
            UpdateFlags::UPDATE_CLIENTS,
        );

        // Vanilla `ItemStack.onCraftedBySystem` runs `Item.onCraftedPostProcess`
        // here; Steel has no crafted-post-process item hook (see module docs).

        let direction = orientation_front(state.get_value(ORIENTATION));
        Self::dispense_item(world, pos, results, direction, recipe.key(), input.items());

        if let Some((remainders, _)) = recipe_manager::get_remaining_items(&input) {
            for remainder in remainders {
                if !remainder.is_empty() {
                    Self::dispense_item(
                        world,
                        pos,
                        remainder,
                        direction,
                        recipe.key(),
                        input.items(),
                    );
                }
            }
        }

        crafter.consume_crafting_ingredients();
        block_entity.set_changed();
    }

    /// Vanilla `CrafterBlock.dispenseItem`.
    fn dispense_item(
        world: &Arc<World>,
        pos: BlockPos,
        results: ItemStack,
        direction: Direction,
        recipe: &Identifier,
        ingredients: &[ItemStack],
    ) {
        let target_pos = direction.relative(pos);
        let target = hopper_container_at(world, target_pos);
        let mut remaining = results;

        if let Some(target_ref) = target {
            let target_is_crafter = world
                .get_block_entity(target_pos)
                .is_some_and(|entity| entity.downcast_ref::<CrafterBlockEntity>().is_some());
            let max_stack = {
                let guard = ContainerLockGuard::lock_all(&[&target_ref]);
                guard
                    .get(target_ref.container_id())
                    .map_or(64, |container| {
                        container.get_max_stack_size_for_item(&remaining)
                    })
            };
            let one_at_a_time = target_is_crafter || remaining.count() > max_stack;

            let mut guard = ContainerLockGuard::lock_all(&[&target_ref]);
            let target_id = target_ref.container_id();
            if one_at_a_time {
                while !remaining.is_empty() {
                    let Some(target_container) = guard.get_mut(target_id) else {
                        break;
                    };
                    let copy = remaining.copy_with_count(1);
                    let leftover =
                        hopper_add_item(None, target_container, copy, Some(direction.opposite()));
                    if !leftover.is_empty() {
                        break;
                    }
                    remaining.shrink(1);
                }
            } else {
                while !remaining.is_empty() {
                    let Some(target_container) = guard.get_mut(target_id) else {
                        break;
                    };
                    let before = remaining.count();
                    remaining = hopper_add_item(
                        None,
                        target_container,
                        remaining,
                        Some(direction.opposite()),
                    );
                    if remaining.count() == before {
                        break;
                    }
                }
            }
            guard.set_changed(target_id);
        }

        if remaining.is_empty() {
            return;
        }

        // Vanilla ejects the leftover from `centerPos.relative(direction, 0.7)`
        // with a directional triangle velocity.
        let (dx, dy, dz) = direction.offset();
        let spawn = glam::DVec3::new(
            f64::from(pos.x()) + 0.5 + f64::from(dx) * 0.7,
            f64::from(pos.y()) + 0.5 + f64::from(dy) * 0.7 - 0.125,
            f64::from(pos.z()) + 0.5 + f64::from(dz) * 0.7,
        );
        let pow = rand::random::<f64>() * 0.1 + 0.2;
        let deviation = 0.017_227_5 * 6.0;
        let velocity = glam::DVec3::new(
            triangle_random(f64::from(dx) * pow, deviation),
            triangle_random(0.2, deviation),
            triangle_random(f64::from(dz) * pow, deviation),
        );
        let _ = world.spawn_item_with_velocity(spawn, remaining, velocity);

        Self::trigger_crafter_recipe_crafted(world, pos, recipe, ingredients);

        world.level_event(LEVEL_EVENT_SOUND_CRAFTER_CRAFT, pos, 0, None);
        world.level_event(
            LEVEL_EVENT_PARTICLES_CRAFTER_CRAFT,
            pos,
            direction.get_3d_data_value(),
            None,
        );
    }

    /// Vanilla `CriteriaTriggers.CRAFTER_RECIPE_CRAFTED.trigger` for every
    /// `ServerPlayer` inside `AABB.ofSize(centerPos, 17, 17, 17)`.
    fn trigger_crafter_recipe_crafted(
        world: &Arc<World>,
        pos: BlockPos,
        recipe: &Identifier,
        ingredients: &[ItemStack],
    ) {
        let (center_x, center_y, center_z) = pos.get_bottom_center();
        let center = glam::DVec3::new(center_x, center_y + 0.5, center_z);
        let search_box = WorldAabb::of_size(center, 17.0, 17.0, 17.0);

        for entity in
            world.get_entities_in_aabb_matching(&search_box, |entity| entity.as_player().is_some())
        {
            let Some(player) = entity.as_player() else {
                continue;
            };
            let origin = player.block_position();
            let level = WorldLevelView::new(world, origin);
            let equipment = living_loot_equipment(player);
            let known = player.recipe_book().unlocked_identifiers().to_vec();
            let view = player_view(player, &level, Some(&equipment), &known);
            let event = TriggerEvent::CrafterRecipeCrafted {
                base: event_base(&view, &level, origin),
                recipe,
                ingredients,
            };
            raise(
                player,
                &Identifier::vanilla_static("crafter_recipe_crafted"),
                &event,
            );
        }
    }
}

impl BlockBehavior for CrafterBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        let nearest = context.get_nearest_looking_direction().opposite();
        let vertical = match nearest {
            Direction::Down => context.horizontal_direction().opposite(),
            Direction::Up => context.horizontal_direction(),
            Direction::North | Direction::South | Direction::West | Direction::East => {
                Direction::Up
            }
        };
        Some(
            self.block
                .default_state()
                .set_value(ORIENTATION, from_front_and_top(nearest, vertical))
                .set_value(
                    TRIGGERED,
                    context.world.has_neighbor_signal(context.hit_pos()),
                ),
        )
    }

    fn set_placed_by(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _source: &PlacementSource<'_>,
    ) {
        if state.get_value(TRIGGERED) {
            world.schedule_block_tick_default(pos, self.block, CRAFTING_TICK_DELAY);
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
        let Some(container_ref) = ContainerRef::from_block_entity(block_entity) else {
            return InteractionResult::Pass;
        };
        let inventory = Arc::clone(&player.inventory);
        player.open_menu(
            TextComponent::translated(translations::CONTAINER_CRAFTER.msg()),
            move |context| crafter(inventory, context.container_id, container_ref),
        );
        InteractionResult::Success
    }

    fn handle_neighbor_changed(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _source_block: BlockRef,
        _moved_by_piston: bool,
    ) {
        let should_trigger = world.has_neighbor_signal(pos);
        let is_triggered = state.get_value(TRIGGERED);
        if should_trigger && !is_triggered {
            world.schedule_block_tick_default(pos, self.block, CRAFTING_TICK_DELAY);
            world.set_block(
                pos,
                state.set_value(TRIGGERED, true),
                UpdateFlags::UPDATE_CLIENTS,
            );
            Self::set_block_entity_triggered(world, pos, true);
        } else if !should_trigger && is_triggered {
            world.set_block(
                pos,
                state.set_value(TRIGGERED, false).set_value(CRAFTING, false),
                UpdateFlags::UPDATE_CLIENTS,
            );
            Self::set_block_entity_triggered(world, pos, false);
        }
    }

    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        Self::dispense_from(state, world, pos);
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        let Some(entity) =
            BLOCK_ENTITIES.create(&vanilla_block_entity_types::CRAFTER, level, pos, state)
        else {
            return BlockEntityCreation::Unimplemented;
        };
        if let Some(crafter) = entity.downcast_ref::<CrafterBlockEntity>() {
            crafter.set_triggered(state.try_get_value(TRIGGERED).unwrap_or(false));
        }
        BlockEntityCreation::Created(entity)
    }

    fn get_block_entity_ticker(
        &self,
        _world: &Arc<World>,
        _state: BlockStateId,
        block_entity_type: BlockEntityTypeRef,
    ) -> Option<BlockEntityTicker> {
        BlockEntityTicker::for_matching_entity_tick(
            block_entity_type,
            &vanilla_block_entity_types::CRAFTER,
        )
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
        let Some(entity) = world.get_block_entity(pos) else {
            return 0;
        };
        entity
            .downcast_ref::<CrafterBlockEntity>()
            .map_or(0, CrafterBlockEntity::get_redstone_signal)
    }

    fn affect_neighbors_after_removal(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _moved_by_piston: bool,
    ) {
        world.update_neighbor_for_output_signal(pos, self.block);
    }
}
