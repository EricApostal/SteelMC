//! Vanilla `CopperGolemStatueBlock` and `WeatheringCopperGolemStatueBlock`.

use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{
    BlockStateProperties, BoolProperty, EnumProperty, Pose, PropertyEnum as _,
};
use steel_registry::data_components::components::BlockItemStateProperties;
use steel_registry::data_components::vanilla_components::BLOCK_STATE;
use steel_registry::fluid::FluidStateExt as _;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::item::BlockHitResult;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{REGISTRY, sound_events, vanilla_game_events};
use steel_utils::types::{InteractionHand, UpdateFlags};
use steel_utils::{BlockPos, BlockStateId, Direction, Downcast as _};

use crate::behavior::InventoryAccess;
use crate::behavior::block::{
    BlockBehavior, BlockEntityCreation, schedule_water_tick_if_waterlogged,
};
use crate::behavior::blocks::building::{WeatherState, WeatheringCopper};
use crate::behavior::context::{BlockPlaceContext, InteractionResult};
use crate::behavior::weathering::get_weather_state;
use crate::block_entity::BLOCK_ENTITIES;
use crate::block_entity::SharedBlockEntity;
use crate::block_entity::entities::CopperGolemStatueBlockEntity;
use crate::entity::ai::path::PathComputationType;
use crate::player::Player;
use crate::world::game_event::GameEventContext;
use crate::world::{LevelReader, ScheduledTickAccess, World};

const FACING: &EnumProperty<Direction> = &BlockStateProperties::HORIZONTAL_FACING;
const POSE: &EnumProperty<Pose> = &BlockStateProperties::COPPER_GOLEM_POSE;
const WATERLOGGED: &BoolProperty = &BlockStateProperties::WATERLOGGED;

/// Copper golem statue behavior, used by the waxed variants.
#[block_behavior]
pub struct CopperGolemStatueBlock {
    block: BlockRef,
}

/// Weathering copper golem statue behavior.
#[block_behavior]
pub struct WeatheringCopperGolemStatueBlock {
    block: BlockRef,
    weathering: WeatheringCopper,
}

impl CopperGolemStatueBlock {
    /// Creates a copper golem statue behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }
}

impl WeatheringCopperGolemStatueBlock {
    /// Creates a weathering copper golem statue behavior.
    #[must_use]
    pub fn new(block: BlockRef) -> Self {
        Self {
            block,
            weathering: WeatheringCopper::new(
                get_weather_state(block).unwrap_or(WeatherState::Unaffected),
            ),
        }
    }
}

/// Vanilla `CopperGolemStatueBlock.Pose.getNextPose`.
const fn next_pose(pose: Pose) -> Pose {
    match pose {
        Pose::Standing => Pose::Sitting,
        Pose::Sitting => Pose::Running,
        Pose::Running => Pose::Star,
        Pose::Star => Pose::Standing,
    }
}

/// Vanilla `CopperGolemStatueBlock.Pose.ordinal() + 1`, the comparator output.
const fn pose_signal(pose: Pose) -> i32 {
    match pose {
        Pose::Standing => 1,
        Pose::Sitting => 2,
        Pose::Running => 3,
        Pose::Star => 4,
    }
}

/// Vanilla `CopperGolemStatueBlock.updatePose`.
fn update_pose(world: &Arc<World>, state: BlockStateId, pos: BlockPos, player: &Player) {
    world.play_block_sound(
        &sound_events::ENTITY_COPPER_GOLEM_BECOME_STATUE,
        pos,
        1.0,
        1.0,
        None,
    );
    let new_state = state.set_value(POSE, next_pose(state.get_value(POSE)));
    world.set_block(pos, new_state, UpdateFlags::UPDATE_ALL);
    world.game_event(
        &vanilla_game_events::BLOCK_CHANGE,
        pos,
        &GameEventContext::new(Some(player), Some(new_state)),
    );
}

fn place_statue_entity(
    level: Weak<World>,
    pos: BlockPos,
    state: BlockStateId,
) -> BlockEntityCreation {
    BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
        &steel_registry::vanilla_block_entity_types::COPPER_GOLEM_STATUE,
        level,
        pos,
        state,
    ))
}

/// Vanilla `CopperGolemStatueBlock.getCloneItemStack`: the picked statue keeps
/// its stored components and its current pose.
fn clone_item(
    block: BlockRef,
    state: BlockStateId,
    block_entity: Option<&SharedBlockEntity>,
) -> ItemStack {
    let mut item = ItemStack::new(REGISTRY.items.by_block(block));
    if let Some(block_entity) = block_entity {
        let components = block_entity.stored_components();
        if !components.is_empty() {
            let mut patch = item.patch().clone();
            for (key, value) in components.iter() {
                patch.set_raw(key.clone(), value.clone());
            }
            item = ItemStack::with_count_and_patch(item.item(), item.count(), patch);
        }
    }

    let mut properties = std::collections::BTreeMap::new();
    properties.insert(
        POSE.name.to_string(),
        state.get_value(POSE).as_str().to_string(),
    );
    item.set(BLOCK_STATE, BlockItemStateProperties::new(properties));
    item
}

impl BlockBehavior for CopperGolemStatueBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(
            self.block
                .default_state()
                .set_value(FACING, context.horizontal_direction().opposite())
                .set_value(WATERLOGGED, context.is_water_source()),
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

    /// Vanilla `CopperGolemStatueBlock.useItemOn`: any item that is not an axe
    /// cycles the pose; axes fall through to the item.
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
        if inv.with_item(|stack| stack.item().has_tag(&ItemTag::AXES)) {
            return InteractionResult::Pass;
        }

        update_pose(world, state, pos, player);
        InteractionResult::Success
    }

    /// Vanilla `CopperGolemStatueBlock.isPathfindable`: water pathfinding only
    /// while waterlogged.
    fn is_pathfindable(&self, state: BlockStateId, computation_type: PathComputationType) -> bool {
        computation_type == PathComputationType::Water
            && state.get_value(WATERLOGGED)
            && state.get_fluid_state().is_water()
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        place_statue_entity(level, pos, state)
    }

    /// Vanilla `CopperGolemStatueBlock.shouldChangedStateKeepBlockEntity`: the
    /// block entity survives oxidation because every statue is in the tag.
    fn should_keep_block_entity(&self, old_state: BlockStateId, _new_state: BlockStateId) -> bool {
        old_state
            .get_block()
            .has_tag(&BlockTag::COPPER_GOLEM_STATUES)
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
        pose_signal(state.get_value(POSE))
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

    fn get_clone_item_stack(
        &self,
        block: BlockRef,
        state: BlockStateId,
        block_entity: Option<SharedBlockEntity>,
        _include_data: bool,
    ) -> Option<ItemStack> {
        Some(clone_item(block, state, block_entity.as_ref()))
    }
}

impl BlockBehavior for WeatheringCopperGolemStatueBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(
            self.block
                .default_state()
                .set_value(FACING, context.horizontal_direction().opposite())
                .set_value(WATERLOGGED, context.is_water_source()),
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

    /// Vanilla `WeatheringCopperGolemStatueBlock.useItemOn`: an axe converts an
    /// unoxidized statue back into a live copper golem; anything else (except
    /// honeycomb, which waxes the statue) cycles the pose.
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
        let (is_axe, is_honeycomb) = inv.with_item(|stack| {
            (
                stack.item().has_tag(&ItemTag::AXES),
                stack.is(&steel_registry::vanilla_items::HONEYCOMB),
            )
        });

        if !is_axe {
            if is_honeycomb {
                return InteractionResult::Pass;
            }
            update_pose(world, state, pos, player);
            return InteractionResult::Success;
        }

        if get_weather_state(state.get_block()) != Some(WeatherState::Unaffected) {
            return InteractionResult::Pass;
        }

        let Some(block_entity) = world.get_block_entity(pos) else {
            return InteractionResult::Pass;
        };
        let Some(statue) = block_entity.downcast_ref::<CopperGolemStatueBlockEntity>() else {
            return InteractionResult::Pass;
        };

        // Vanilla damages the axe before checking the converted golem, so a
        // failed conversion still costs durability.
        let golem = statue.remove_statue();
        inv.with_item(|stack| {
            stack.hurt_and_break(1, player.has_infinite_materials());
        });
        let Some(golem) = golem else {
            return InteractionResult::Pass;
        };
        let _ = world.try_add_entity(golem);
        world.remove_block(pos, false);
        InteractionResult::Success
    }

    /// Vanilla `WeatheringCopperGolemStatueBlock.randomTick`.
    fn random_tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        self.weathering.change_over_time(state, world, pos);
    }

    fn is_pathfindable(&self, state: BlockStateId, computation_type: PathComputationType) -> bool {
        computation_type == PathComputationType::Water
            && state.get_value(WATERLOGGED)
            && state.get_fluid_state().is_water()
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        place_statue_entity(level, pos, state)
    }

    fn should_keep_block_entity(&self, old_state: BlockStateId, _new_state: BlockStateId) -> bool {
        old_state
            .get_block()
            .has_tag(&BlockTag::COPPER_GOLEM_STATUES)
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
        pose_signal(state.get_value(POSE))
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

    fn get_clone_item_stack(
        &self,
        block: BlockRef,
        state: BlockStateId,
        block_entity: Option<SharedBlockEntity>,
        _include_data: bool,
    ) -> Option<ItemStack> {
        Some(clone_item(block, state, block_entity.as_ref()))
    }
}
