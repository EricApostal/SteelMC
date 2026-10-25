//! Bucket item behavior implementation.
//!
//! Handles water buckets, lava buckets, and empty buckets.
//!
//! Mirrors vanilla's `BucketItem(Fluid fluid)`: `fluid_block = None` = empty bucket,
//! `Some(block)` = filled bucket. Logic is dispatched in `use_item`.
//!
use crate::behavior::advancement_hooks::{raise_filled_bucket, raise_placed_block};
use crate::behavior::context::InteractionResult;
use crate::behavior::item_utils::create_filled_result;
use crate::behavior::{
    BLOCK_BEHAVIORS, BlockStateBehaviorExt, FLUID_BEHAVIORS, ItemBehavior, UseItemContext,
    pickup_waterlogged_block,
};
use crate::entity::Entity;
use crate::fluid::FluidStateExt;
use crate::world::RaytraceAction;
use glam::DVec3;
use steel_macros::item_behavior;
use steel_protocol::packets::game::RandomizationType;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::blocks::properties::Direction;
use steel_registry::fluid::FluidState;
use steel_registry::item_stack::ItemStack;
use steel_registry::particle_type::{ParticleData, SimpleParticleOptions};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::sound_events;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::vanilla_blocks;
use steel_registry::vanilla_fluids;
use steel_registry::vanilla_game_events;
use steel_registry::vanilla_items;
use steel_registry::vanilla_particle_types;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId};

use crate::world::game_event::GameEventContext;

/// Handles all bucket variants (empty, water, lava).
#[item_behavior]
pub struct BucketItem {
    #[json_arg(vanilla_blocks, json = "content", optional = "empty")]
    fluid_block: Option<BlockRef>,
}

impl BucketItem {
    /// Creates a new bucket behavior. `None` = empty bucket, `Some(block)` = filled.
    #[must_use]
    pub const fn new(fluid_block: Option<BlockRef>) -> Self {
        Self { fluid_block }
    }
}

impl ItemBehavior for BucketItem {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        match self.fluid_block {
            None => use_empty_bucket(context),
            Some(fluid_block) => use_filled_bucket(fluid_block, None, context).0,
        }
    }
}

/// Vanilla `BucketItem.getEmptySuccessItem`: the stack left after emptying a
/// bucket (`Items.BUCKET`), or the original stack for infinite-materials users.
pub(crate) fn empty_success_stack(context: &UseItemContext) -> ItemStack {
    if context.player.has_infinite_materials() {
        context
            .inv
            .with_item(|item| item.copy_with_count(item.count()))
    } else {
        ItemStack::new(&vanilla_items::BUCKET)
    }
}

fn use_empty_bucket(context: &mut UseItemContext) -> InteractionResult {
    let (start, end) = context.player.get_ray_endpoints();

    // Raytrace: stop on source fluids
    let (hit_block, hit_direction) = context.world.raytrace(start, end, |pos, world| {
        let state = world.get_block_state(pos);
        let block = state.get_block();

        if block == &vanilla_blocks::AIR {
            return RaytraceAction::Pass;
        }

        let fluid_state = state.get_fluid_state();
        if fluid_state.is_source() {
            return RaytraceAction::ImmediateHit;
        }
        // Vanilla parity: ClipContext.Fluid.SOURCE_ONLY — flowing fluid is transparent.
        if !fluid_state.is_empty() {
            return RaytraceAction::Pass;
        }

        RaytraceAction::CheckShape
    });

    // Vanilla returns PASS when raytrace misses (allows other handlers to try)
    let (Some(hit_pos), Some(direction)) = (hit_block, hit_direction) else {
        return InteractionResult::Pass;
    };

    let item = context.inv.with_item(|item| item.clone());
    if !context
        .player
        .may_use_item_at(direction.relative(hit_pos), direction, &item)
    {
        return InteractionResult::Fail;
    }

    let hit_state = context.world.get_block_state(hit_pos);
    let block_behavior = BLOCK_BEHAVIORS.get_behavior(hit_state.get_block());

    if let Some(result) =
        block_behavior.pickup_block(context.world, hit_pos, hit_state, Some(context.player))
    {
        context.inv.with_item(|item| {
            context
                .player
                .award_stat(&vanilla_stat_types::ITEM_USED, item.item());
        });
        // Apply sound
        if let Some(sound) = result.sound {
            // Vanilla `BucketItem.use`: `bucketPickupBlock.getPickupSound()...player.playSound(...)`.
            context.player.play_sound(sound, 1.0, 1.0);
        }

        // Give filled bucket
        let filled = result.filled_bucket.clone();
        create_filled_result(context, result.filled_bucket, true);
        // Vanilla `BucketItem.use` raises `filled_bucket` with the new stack after
        // `ItemUtils.createFilledResult` has put it in the player's hand.
        raise_filled_bucket(context.player, context.world, &filled);
        context.world.game_event(
            &vanilla_game_events::FLUID_PICKUP,
            hit_pos,
            &GameEventContext::new(Some(context.player.as_ref()), None),
        );

        return InteractionResult::Success;
    }

    // TODO: Remove fallback once all waterloggable blocks implement pickup_block.
    if let Some(result) = pickup_waterlogged_block(
        block_behavior,
        context.world,
        hit_pos,
        hit_state,
        Some(context.player),
    ) {
        context.inv.with_item(|item| {
            context
                .player
                .award_stat(&vanilla_stat_types::ITEM_USED, item.item());
        });
        if let Some(sound) = result.sound {
            // Vanilla `BucketItem.use`: `bucketPickupBlock.getPickupSound()...player.playSound(...)`.
            context.player.play_sound(sound, 1.0, 1.0);
        }

        let filled = result.filled_bucket.clone();
        create_filled_result(context, result.filled_bucket, true);
        // Vanilla `BucketItem.use` raises `filled_bucket` with the new stack after
        // `ItemUtils.createFilledResult` has put it in the player's hand.
        raise_filled_bucket(context.player, context.world, &filled);
        context.world.game_event(
            &vanilla_game_events::FLUID_PICKUP,
            hit_pos,
            &GameEventContext::new(Some(context.player.as_ref()), None),
        );

        return InteractionResult::Success;
    }

    // Nothing was picked up — no fluid source block and no waterlogged block found.
    // Vanilla returns FAIL here so the client knows no item change occurred.
    InteractionResult::Fail
}

/// Mirrors vanilla `BucketItem.emptyContents` together with the placement
/// position chosen by `BucketItem.use`, returning the position where the
/// contents were actually placed.
///
/// `MobBucketItem` needs that position for its `checkExtraContent` mob spawn,
/// and passes its own empty sound so `playEmptySound` matches vanilla's
/// per-entity override.
#[expect(
    clippy::too_many_lines,
    reason = "mirrors vanilla's emptyContents flow; splitting would obscure the sequential placement logic"
)]
pub(crate) fn use_filled_bucket(
    fluid_block: BlockRef,
    empty_sound: Option<SoundEventRef>,
    context: &mut UseItemContext,
) -> (InteractionResult, Option<BlockPos>) {
    // Raytrace to find target block
    let (start, end) = context.player.get_ray_endpoints();
    let (ray_block, ray_dir) = context.world.raytrace(start, end, |pos, world| {
        let state = world.get_block_state(pos);
        let block = state.get_block();
        // Filled buckets use ClipContext.Fluid.NONE: ignore fluid shapes, but
        // still test the block shape of waterlogged/container blocks.
        if block == &vanilla_blocks::AIR {
            return RaytraceAction::Pass;
        }
        RaytraceAction::CheckShape
    });

    // Vanilla returns PASS when raytrace misses (allows other handlers to try)
    let (Some(clicked_pos), Some(direction)) = (ray_block, ray_dir) else {
        return (InteractionResult::Pass, None);
    };

    // If the block is out of bounds, return fail
    if !context.world.is_in_valid_bounds(clicked_pos) {
        return (InteractionResult::Fail, None);
    }

    let item = context.inv.with_item(|item| item.clone());
    if !context
        .player
        .may_use_item_at(direction.relative(clicked_pos), direction, &item)
    {
        return (InteractionResult::Fail, None);
    }

    let clicked_state = context.world.get_block_state(clicked_pos);
    let is_sneaking = context.player.is_crouching();

    // Define fluid placement logic as a closure to reuse for primary/secondary targets.
    // `check_sneak`: true for primary attempt, false for secondary (vanilla parity:
    // recursive emptyContents passes hitResult=null for fallback, bypassing sneak check).
    let try_place_fluid = |pos: BlockPos, check_sneak: bool| -> bool {
        if !context.world.is_in_valid_bounds(pos) {
            return false;
        }

        let state = context.world.get_block_state(pos);
        let fluid_state = state.get_fluid_state();

        // Vanilla parity (bl4): when sneaking, only air allows placement at this position.
        // Non-air blocks redirect to the neighbor — handled by the secondary call.
        // The secondary call bypasses this check (hitResult == null in vanilla).
        if check_sneak && is_sneaking && !state.get_block().config.is_air {
            return false;
        }

        let is_water_bucket = fluid_block == &vanilla_blocks::WATER;
        let behavior = BLOCK_BEHAVIORS.get_behavior(state.get_block());
        let is_liquid_container = state.is_liquid_container();
        let can_place_liquid = is_water_bucket
            && is_liquid_container
            && behavior.can_place_liquid_with_player(
                state,
                FluidState::source(&vanilla_fluids::WATER).fluid_id,
                Some(context.player),
            );
        let can_replace = state.can_be_replaced_by_fluid(fluid_block);

        // Vanilla parity: block must be replaceable or liquid-container-admissible for placement.
        if !can_replace && !can_place_liquid {
            return false;
        }

        // Vanilla `BucketItem.emptyContents`: in worlds where water evaporates
        // (e.g. the Nether) a water bucket fizzes out without placing a fluid —
        // the extinguish sound plus a large-smoke burst at the block corner.
        //
        // Vanilla reads `EnvironmentAttributes.WATER_EVAPORATES` per position;
        // Steel's environment attributes cover only the sky/weather set, so this
        // uses the dimension-type flag, which is equal for every vanilla
        // dimension.
        if is_water_bucket && context.world.dimension_type.water_evaporates {
            context.world.play_block_sound(
                &sound_events::BLOCK_FIRE_EXTINGUISH,
                pos,
                0.5,
                // Java: `2.6F + (random.nextFloat() - random.nextFloat()) * 0.8F`.
                2.6 + (rand::random::<f32>() - rand::random::<f32>()) * 0.8,
                // Vanilla `level.playSound(user, pos, ...)` excludes the placing player.
                Some(context.player.id()),
            );
            context.world.send_particles_with_randomization(
                ParticleData::new(&vanilla_particle_types::LARGE_SMOKE, SimpleParticleOptions),
                DVec3::new(f64::from(pos.x()), f64::from(pos.y()), f64::from(pos.z())),
                8,
                DVec3::new(1.0, 1.0, 1.0),
                0.0,
                RandomizationType::Alternative,
            );
            return true;
        }

        // 1. Try LiquidBlockContainer handling (only if Water bucket).
        if is_water_bucket && is_liquid_container {
            let source_water = FluidState::source(&vanilla_fluids::WATER);
            behavior.place_liquid(context.world, pos, state, source_water);
            play_empty_sound_and_event(context, pos, true, empty_sound);
            return true;
        }

        // 2. Try Standard Placement (Replaceable block)
        if can_replace {
            // If same fluid already exists and is source, just consume bucket (parity)
            let is_same_fluid = if is_water_bucket {
                fluid_state.is_water()
            } else {
                fluid_state.is_lava()
            };

            if is_same_fluid && fluid_state.is_source() {
                play_empty_sound_and_event(context, pos, is_water_bucket, empty_sound);
                return true;
            }

            // Vanilla parity: destroy non-liquid replaceable blocks first so they
            // drop their items (e.g. tall grass, flowers, snow layers).
            if !state.get_block().config.liquid && !state.get_block().config.is_air {
                context.player.get_world().destroy_block(pos, true);
            }

            // Place fluid block
            let fluid_state_to_place = fluid_block.default_state();
            if context
                .world
                .set_block(pos, fluid_state_to_place, UpdateFlags::UPDATE_ALL_IMMEDIATE)
            {
                let fluid_ref = if is_water_bucket {
                    &vanilla_fluids::WATER
                } else {
                    &vanilla_fluids::LAVA
                };
                let tick_delay = FLUID_BEHAVIORS
                    .get_behavior(fluid_ref)
                    .tick_delay(context.world);
                context
                    .world
                    .schedule_fluid_tick_default(pos, fluid_ref, tick_delay);

                play_empty_sound_and_event(context, pos, is_water_bucket, empty_sound);

                return true;
            }
        }
        false
    };

    // Vanilla parity (BucketItem.java): position selection mirrors
    // `instanceof LiquidBlockContainer && content == Fluids.WATER ? pos : directionOffsetPos`.
    // If primary fails, secondary retries at the offset pos without sneak check,
    // matching vanilla's recursive `emptyContents(hitResult=null)` fallback.
    let is_water_bucket = fluid_block == &vanilla_blocks::WATER;
    let primary_pos =
        filled_bucket_primary_pos(clicked_state, clicked_pos, direction, is_water_bucket);

    // Attempt Primary (with sneak check)
    if try_place_fluid(primary_pos, true) {
        raise_bucket_placed_block(context, primary_pos);
        let result_stack = empty_success_stack(context);
        create_filled_result(context, result_stack, true);
        return (InteractionResult::Success, Some(primary_pos));
    }

    // Attempt Secondary (Fallback — no sneak check, matching vanilla hitResult=null).
    // Vanilla's emptyContents always recurses with hitResult=null at the offset position
    // when the primary attempt fails, regardless of bucket type.
    let secondary_pos = direction.relative(clicked_pos);
    if try_place_fluid(secondary_pos, false) {
        raise_bucket_placed_block(context, secondary_pos);
        let result_stack = empty_success_stack(context);
        create_filled_result(context, result_stack, true);
        return (InteractionResult::Success, Some(secondary_pos));
    }

    (InteractionResult::Fail, None)
}

/// Vanilla `BucketItem.use`'s `PLACED_BLOCK` raise: a filled bucket that emptied
/// its fluid at `pos`. The held stack is snapshotted before the raise so the
/// player's inventory is not locked underneath it.
fn raise_bucket_placed_block(context: &UseItemContext, pos: BlockPos) {
    let item = context.inv.with_item(|item| item.clone());
    raise_placed_block(context.player, context.world, pos, &item);
}

fn play_empty_sound_and_event(
    context: &UseItemContext,
    pos: BlockPos,
    is_water_bucket: bool,
    empty_sound: Option<SoundEventRef>,
) {
    // Vanilla `MobBucketItem` overrides `BucketItem.playEmptySound` with its
    // own per-entity sound; every other bucket keeps the fluid default.
    let sound_event = empty_sound.unwrap_or(if is_water_bucket {
        &sound_events::ITEM_BUCKET_EMPTY
    } else {
        &sound_events::ITEM_BUCKET_EMPTY_LAVA
    });
    context.world.play_block_sound(
        sound_event,
        pos,
        1.0,
        1.0,
        // Vanilla `BucketItem.playEmptySound`: `level.playSound(user, pos, ...)` excludes the placing player.
        Some(context.player.id()),
    );
    context.world.game_event(
        &vanilla_game_events::FLUID_PLACE,
        pos,
        &GameEventContext::new(Some(context.player.as_ref()), None),
    );
}

fn filled_bucket_primary_pos(
    clicked_state: BlockStateId,
    clicked_pos: BlockPos,
    direction: Direction,
    is_water_bucket: bool,
) -> BlockPos {
    if is_water_bucket && clicked_state.is_liquid_container() {
        clicked_pos
    } else {
        direction.relative(clicked_pos)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        behavior::init_behaviors,
        player::game_mode::use_item,
        test_support::{TestPlayerBuilder, fresh_test_world, insert_ready_full_chunk},
    };
    use glam::DVec3;
    use std::sync::Arc;
    use steel_registry::{
        RegistryHolderSet,
        data_component_predicate::DataComponentMatchers,
        data_components::{
            AdventureModePredicate, BlockPredicate, vanilla_components::CAN_PLACE_ON,
        },
        init_vanilla_registry,
        item_stack::ItemStack,
        vanilla_blocks, vanilla_items,
    };
    use steel_utils::{
        BlockPos, ChunkPos,
        types::{GameType, InteractionHand, UpdateFlags},
    };

    use super::*;

    #[test]
    fn filled_water_bucket_targets_non_waterlogged_liquid_container_in_place() {
        init_vanilla_registry();
        init_behaviors();

        let kelp = vanilla_blocks::KELP.default_state();

        assert_eq!(
            filled_bucket_primary_pos(kelp, BlockPos::ZERO, Direction::North, true),
            BlockPos::ZERO
        );
        assert_eq!(
            filled_bucket_primary_pos(kelp, BlockPos::ZERO, Direction::North, false),
            Direction::North.relative(BlockPos::ZERO)
        );
    }

    #[test]
    fn adventure_buckets_require_can_place_on_and_fail_when_denied() {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world("adventure_bucket_restriction");
        let world = &world_fixture.world;
        let pos = BlockPos::new(0, 65, 2);
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));
        assert!(world.set_block(
            pos,
            vanilla_blocks::WATER.default_state(),
            UpdateFlags::UPDATE_ALL,
        ));

        let player = TestPlayerBuilder::new(Arc::clone(world), "TestPlayer", 1).build();
        player.base().set_position_local(DVec3::new(0.5, 64.0, 0.5));
        player.set_rotation((0.0, 0.0));
        player.restore_game_modes(GameType::Adventure, None);
        player
            .abilities
            .lock()
            .update_for_game_mode(GameType::Adventure);

        player
            .inventory
            .lock()
            .set_selected_item(ItemStack::new(&vanilla_items::BUCKET));
        assert_eq!(
            use_item(&player, world, InteractionHand::MainHand),
            InteractionResult::Fail
        );
        assert_eq!(
            world.get_block_state(pos),
            vanilla_blocks::WATER.default_state()
        );

        let predicate = BlockPredicate::new(
            Some(RegistryHolderSet::Direct(vec![&vanilla_blocks::WATER])),
            None,
            None,
            DataComponentMatchers::ANY,
        );
        let mut allowed_bucket = ItemStack::new(&vanilla_items::BUCKET);
        allowed_bucket.set(
            CAN_PLACE_ON,
            AdventureModePredicate::new(vec![predicate]).expect("one block predicate is valid"),
        );
        player.inventory.lock().set_selected_item(allowed_bucket);
        assert!(use_item(&player, world, InteractionHand::MainHand).consumes_action());
        assert_eq!(
            world.get_block_state(pos),
            vanilla_blocks::AIR.default_state()
        );

        assert!(world.set_block(
            pos,
            vanilla_blocks::STONE.default_state(),
            UpdateFlags::UPDATE_ALL,
        ));
        player
            .inventory
            .lock()
            .set_selected_item(ItemStack::new(&vanilla_items::WATER_BUCKET));
        assert_eq!(
            use_item(&player, world, InteractionHand::MainHand),
            InteractionResult::Fail
        );
        assert_eq!(
            world.get_block_state(pos),
            vanilla_blocks::STONE.default_state()
        );
    }
}
