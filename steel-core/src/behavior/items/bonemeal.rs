use std::sync::Arc;

use rand::RngExt;
use steel_macros::item_behavior;
use steel_registry::{
    REGISTRY, TaggedRegistryExt as _,
    blocks::{
        BlockRef, block_state_ext::BlockStateExt, properties::BlockStateProperties,
        shapes::is_offset_shape_full_block,
    },
    data_components::vanilla_components::USE_EFFECTS,
    item_stack::ItemStack,
    level_events,
    vanilla_biome_tags::BiomeTag,
    vanilla_block_tags::BlockTag,
    vanilla_blocks, vanilla_game_events,
};
use steel_utils::{BlockPos, Direction, types::UpdateFlags};

use crate::{
    behavior::{BLOCK_BEHAVIORS, InteractionResult, ItemBehavior, UseOnContext},
    entity::Entity,
    world::{LevelReader as _, World},
};

/// Behavior for the Bonemeal item.
#[item_behavior]
pub struct BoneMealItem;

impl BoneMealItem {
    /// Vanilla `BuiltInRegistries.BLOCK.getRandomElementOf(tag, random)`.
    ///
    /// Returns `None` for an empty or unknown tag, which vanilla treats as
    /// "keep the current state".
    fn random_block_from_tag(
        tag: &steel_utils::Identifier,
        rng: &mut (impl RngExt + ?Sized),
    ) -> Option<BlockRef> {
        let blocks = REGISTRY.blocks.get_tag(tag)?;
        if blocks.is_empty() {
            return None;
        }
        let index = rng.random_range(0..blocks.len());
        Some(blocks[index])
    }

    fn cause_finish_use_vibration(context: &UseOnContext<'_>) {
        let interact_vibrations = context.inv.with_item(|item| {
            item.get(USE_EFFECTS)
                .is_some_and(|effects| effects.interact_vibrations)
        });
        if interact_vibrations {
            context
                .player
                .game_event(&vanilla_game_events::ITEM_INTERACT_FINISH);
        }
    }

    fn grow(world: &Arc<World>, pos: BlockPos) -> bool {
        let state = world.get_block_state(pos);
        let Some(behavior) = BLOCK_BEHAVIORS.get_behavior_for_state(state) else {
            return false;
        };
        if let Some(bonemealable) = behavior.as_bonemealable() {
            if !bonemealable.is_valid_bonemeal_target(state, world.as_ref(), pos) {
                return false;
            }

            let mut rng = rand::rng();
            if bonemealable.is_bonemeal_success(state, world, &mut rng, pos) {
                bonemealable.perform_bonemeal(state, world, &mut rng, pos);
            }

            return true;
        }
        false
    }

    fn grow_water_plant(world: &Arc<World>, pos: BlockPos, clicked_face: Direction) -> bool {
        let state = world.get_block_state(pos);
        if state.get_block() != &vanilla_blocks::WATER || !state.get_fluid_state().is_full() {
            return false;
        }

        let Some(bonemealable) = BLOCK_BEHAVIORS
            .get_behavior(&vanilla_blocks::SEAGRASS)
            .as_bonemealable()
        else {
            return false;
        };

        let mut rng = rand::rng();

        'outer: for i in 0..128 {
            let mut new_pos = pos;
            let mut new_state = vanilla_blocks::SEAGRASS.default_state();

            for _ in 0..(i / 16) {
                new_pos = new_pos.offset(
                    rng.random_range(0i32..3) - 1,
                    (rng.random_range(0i32..3) - 1) * rng.random_range(0i32..3) / 2,
                    rng.random_range(0i32..3) - 1,
                );

                if is_offset_shape_full_block(
                    world
                        .get_block_state(new_pos)
                        .get_collision_shape_at(new_pos),
                ) {
                    continue 'outer;
                }
            }

            // Vanilla `growWaterPlant`: coral biomes grow wall fans on the
            // clicked horizontal face for the first position and underwater
            // bonemeal blocks on one in four positions.
            if world
                .biome_at(new_pos)
                .is_some_and(|biome| biome.has_tag(&BiomeTag::PRODUCES_CORALS_FROM_BONEMEAL))
            {
                if i == 0 && clicked_face.is_horizontal() {
                    if let Some(coral) =
                        Self::random_block_from_tag(&BlockTag::WALL_CORALS, &mut rng)
                    {
                        new_state = coral.default_state();
                        if new_state
                            .try_get_value(&BlockStateProperties::HORIZONTAL_FACING)
                            .is_some()
                        {
                            new_state = new_state
                                .set_value(&BlockStateProperties::HORIZONTAL_FACING, clicked_face);
                        }
                    }
                } else if rng.random_range(0..4) == 0
                    && let Some(coral) =
                        Self::random_block_from_tag(&BlockTag::UNDERWATER_BONEMEALS, &mut rng)
                {
                    new_state = coral.default_state();
                }
            }

            // Vanilla retries a wall coral's facing until it can survive.
            if new_state
                .try_get_value(&BlockStateProperties::HORIZONTAL_FACING)
                .is_some()
                && new_state.get_block().has_tag(&BlockTag::WALL_CORALS)
            {
                for _ in 0..4 {
                    let behavior = BLOCK_BEHAVIORS.get_behavior(new_state.get_block());
                    if behavior.can_survive(new_state, world, new_pos) {
                        break;
                    }
                    let direction = Direction::HORIZONTAL[rng.random_range(0..4usize)];
                    new_state =
                        new_state.set_value(&BlockStateProperties::HORIZONTAL_FACING, direction);
                }
            }

            let Some(behavior) = BLOCK_BEHAVIORS.get_behavior_for_state(new_state) else {
                return false;
            };

            if behavior.can_survive(new_state, world, new_pos) {
                let current_state = world.get_block_state(new_pos);
                if current_state.get_block() == &vanilla_blocks::WATER
                    && current_state.get_fluid_state().is_full()
                {
                    world.set_block(new_pos, new_state, UpdateFlags::UPDATE_ALL);
                } else if current_state.get_block() == &vanilla_blocks::SEAGRASS
                    && bonemealable.is_valid_bonemeal_target(current_state, world.as_ref(), new_pos)
                    && rng.random_range(0..10) == 0
                {
                    bonemealable.perform_bonemeal(current_state, world, &mut rng, new_pos);
                }
            }
        }

        true
    }
}

impl ItemBehavior for BoneMealItem {
    fn use_on(&self, context: &mut UseOnContext) -> InteractionResult {
        if Self::grow(context.world, context.hit_result.block_pos) {
            context.inv.with_item(ItemStack::shrink_one);
            Self::cause_finish_use_vibration(context);
            context.world.level_event(
                level_events::PARTICLES_AND_SOUND_PLANT_GROWTH,
                context.hit_result.block_pos,
                15,
                None,
            );
            return InteractionResult::Success;
        }
        let state = context.world.get_block_state(context.hit_result.block_pos);
        let is_clicked_face_sturdy = context.world.is_face_sturdy(
            state,
            context.hit_result.block_pos,
            context.hit_result.direction,
        );
        if is_clicked_face_sturdy
            && Self::grow_water_plant(
                context.world,
                context
                    .hit_result
                    .block_pos
                    .relative(context.hit_result.direction),
                context.hit_result.direction,
            )
        {
            context.inv.with_item(ItemStack::shrink_one);
            Self::cause_finish_use_vibration(context);
            context.world.level_event(
                level_events::PARTICLES_AND_SOUND_PLANT_GROWTH,
                context
                    .hit_result
                    .block_pos
                    .relative(context.hit_result.direction),
                15,
                None,
            );
            return InteractionResult::Success;
        }
        InteractionResult::Pass
    }
}
