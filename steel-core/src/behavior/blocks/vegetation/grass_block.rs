//! Vanilla `GrassBlock`.
//!
//! Besides spreading to dirt on random ticks, a grass block grows a patch of
//! short grass and biome flowers when bonemealed.
//!
//! Deviation: vanilla places the `grass_bonemeal` *placed* feature and runs
//! biome `CAN_SPAWN_FROM_BONE_MEAL` features through `Feature.place`; Steel's
//! worldgen entry points (`worldgen::feature::FeatureDecorationRunner`) require a
//! `WorldGenRegion`, so the configured `grass` feature and the biome's flower
//! `SimpleBlock` features are placed directly on the live world. Every feature in
//! `#minecraft:can_spawn_from_bone_meal` is currently a `SimpleBlock` feature, so
//! no placement modifier is skipped (owner: worldgen/feature).

use std::sync::Arc;

use rand::{Rng, RngExt};
use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::feature::{ConfiguredFeature, ConfiguredFeatureKind, ConfiguredFeatureRef};
use steel_registry::vanilla_feature_tags::FeatureTag;
use steel_registry::{
    REGISTRY, RegistryExt as _, TaggedRegistryExt as _, vanilla_blocks, vanilla_configured_features,
};
use steel_utils::random::worldgen_random::WorldgenRandom;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, Direction};

use super::bonemealable::{BonemealAction, Bonemealable};
use super::snowy_block::{snowy_placement_state, update_snowy_shape};
use super::spreading_grass_block::SpreadingGrassBlock;
use crate::behavior::BLOCK_BEHAVIORS;
use crate::behavior::block::BlockBehavior;
use crate::behavior::context::BlockPlaceContext;
use crate::block_state_provider::BlockStateProviderEvaluator;
use crate::world::spawn::placement::is_collision_shape_full_block;
use crate::world::{LevelReader, ScheduledTickAccess, World};

/// Vanilla `GrassBlock.ATTEMPT_COUNT`.
const ATTEMPT_COUNT: i32 = 128;
/// Vanilla `GrassBlock.GROW_TALL_GRASS_CHANCE`: chance to grow an existing short grass.
const GROW_TALL_GRASS_CHANCE: f32 = 0.1;
/// Vanilla `GrassBlock.PLACE_FLOWER_CHANCE`: chance to place a biome flower.
const PLACE_FLOWER_CHANCE: f32 = 0.125;

/// Behavior for grass blocks.
#[block_behavior]
pub struct GrassBlock {
    block: BlockRef,
}

impl GrassBlock {
    /// Creates a new grass block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Vanilla `GrassBlock.stopBonemealSpread`: stops the patch when the block
    /// below is no longer grass or the target position is already occupied.
    fn stop_bonemeal_spread(world: &World, pos: BlockPos) -> bool {
        world.get_block_state(pos.below()).get_block() != &vanilla_blocks::GRASS_BLOCK
            || is_collision_shape_full_block(world.get_block_state(pos), pos)
    }

    /// Vanilla `GrassBlock.placeBonemealEffect`.
    fn place_bonemeal_effect(world: &Arc<World>, rng: &mut dyn Rng, pos: BlockPos) {
        let test_state = world.get_block_state(pos);
        if test_state.get_block() == &vanilla_blocks::SHORT_GRASS
            && rng.random::<f32>() < GROW_TALL_GRASS_CHANCE
        {
            if let Some(bonemealable) = BLOCK_BEHAVIORS
                .get_behavior_for_state(test_state)
                .and_then(|behavior| behavior.as_bonemealable())
                && bonemealable.is_valid_bonemeal_target(test_state, world.as_ref(), pos)
            {
                bonemealable.perform_bonemeal(test_state, world, rng, pos);
            }
        }

        if !test_state.is_air() || world.is_outside_build_height(pos.y()) {
            return;
        }

        if rng.random::<f32>() < PLACE_FLOWER_CHANCE {
            let features = Self::biome_bonemeal_features(world, pos);
            if features.is_empty() {
                return;
            }
            let index = rng.random_range(0..features.len());
            Self::place_simple_block_feature(world, rng, features[index], pos);
        } else {
            Self::place_simple_block_feature(world, rng, &*vanilla_configured_features::GRASS, pos);
        }
    }

    /// Vanilla `BiomeGenerationSettings.getBoneMealFeatures`: every configured
    /// feature reachable from the biome's placed features that is tagged
    /// `#minecraft:can_spawn_from_bone_meal`.
    fn biome_bonemeal_features(world: &World, pos: BlockPos) -> Vec<&'static ConfiguredFeature> {
        let Some(biome) = world.biome_at(pos) else {
            return Vec::new();
        };

        let mut features = Vec::new();
        for placed_key in biome.features.iter().flatten() {
            let Some(placed) = REGISTRY.placed_features.by_key(placed_key) else {
                continue;
            };
            let configured = match &placed.data.feature {
                ConfiguredFeatureRef::Reference(feature) => *feature,
                ConfiguredFeatureRef::Inline(_) => continue,
            };
            if REGISTRY
                .configured_features
                .is_in_tag(configured, &FeatureTag::CAN_SPAWN_FROM_BONE_MEAL)
                && !features.contains(&configured)
            {
                features.push(configured);
            }
        }
        features
    }

    /// Vanilla `SimpleBlockFeature.place` for the bone-meal feature subset:
    /// sample the block state, require survival, then set it.
    fn place_simple_block_feature(
        world: &Arc<World>,
        rng: &mut dyn Rng,
        feature: &'static ConfiguredFeature,
        pos: BlockPos,
    ) -> bool {
        let ConfiguredFeatureKind::SimpleBlock(config) = &feature.kind else {
            return false;
        };

        // Steel's provider sampler needs a `steel_utils::random::Random`; derive
        // one from the bonemeal RNG like the other live-world feature callers.
        let mut random = WorldgenRandom::from_seed(rng.random());
        let Some(state) = BlockStateProviderEvaluator::sample_block_state_provider_optional(
            world.as_ref(),
            &REGISTRY,
            &mut random,
            &config.to_place,
            pos,
        ) else {
            return false;
        };

        let behavior = BLOCK_BEHAVIORS.get_behavior(state.get_block());
        if !behavior.can_survive(state, world.as_ref(), pos) {
            return false;
        }

        world.set_block(pos, state, UpdateFlags::UPDATE_ALL);
        if config.schedule_tick {
            world.schedule_block_tick_default(pos, state.get_block(), 1);
        }
        true
    }
}

impl BlockBehavior for GrassBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(snowy_placement_state(self.block, context))
    }

    fn update_shape(
        &self,
        state: BlockStateId,
        _world: &dyn ScheduledTickAccess,
        _pos: BlockPos,
        direction: Direction,
        _neighbor_pos: BlockPos,
        neighbor_state: BlockStateId,
    ) -> BlockStateId {
        update_snowy_shape(state, direction, neighbor_state)
    }

    fn random_tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        SpreadingGrassBlock::random_tick(
            &vanilla_blocks::GRASS_BLOCK,
            &vanilla_blocks::DIRT,
            state,
            world,
            pos,
        );
    }

    fn as_bonemealable(&self) -> Option<&dyn Bonemealable> {
        Some(self)
    }
}

impl Bonemealable for GrassBlock {
    fn is_valid_bonemeal_target(
        &self,
        _state: BlockStateId,
        world: &dyn LevelReader,
        pos: BlockPos,
    ) -> bool {
        let above = pos.above();
        world.get_block_state(above).is_air() && !world.is_outside_build_height(above.y())
    }

    fn perform_bonemeal(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        rng: &mut dyn Rng,
        pos: BlockPos,
    ) {
        let above = pos.above();

        for attempt in 0..ATTEMPT_COUNT {
            let mut test_pos = above;
            let randomize_count = attempt / 16;

            let mut blocked = false;
            for _ in 0..randomize_count {
                let dx = rng.random_range(-1..=1);
                let dy = rng.random_range(-1..=1) * rng.random_range(0..3) / 2;
                let dz = rng.random_range(-1..=1);
                test_pos = test_pos.offset(dx, dy, dz);
                if Self::stop_bonemeal_spread(world, test_pos) {
                    blocked = true;
                    break;
                }
            }
            if blocked {
                continue;
            }

            Self::place_bonemeal_effect(world, rng, test_pos);
        }
    }

    fn bonemeal_action_type(&self) -> BonemealAction {
        BonemealAction::NeighborSpreader
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::blocks::BlockRef;
    use steel_registry::blocks::block_state_ext::BlockStateExt;
    use steel_registry::blocks::properties::BlockStateProperties;
    use steel_registry::{init_vanilla_registry, vanilla_blocks};
    use steel_utils::{BlockPos, Direction};

    use super::*;
    use crate::behavior::init_behaviors;
    use crate::test_support::TestLevel;

    #[test]
    fn grass_block_updates_snowy_state() {
        init_vanilla_registry();
        init_behaviors();

        let level = TestLevel::default();
        let pos = BlockPos::new(0, 64, 0);
        let behavior = GrassBlock::new(&vanilla_blocks::GRASS_BLOCK);

        let non_snowy = vanilla_blocks::GRASS_BLOCK.default_state();
        let snowy = behavior.update_shape(
            non_snowy,
            &level,
            pos,
            Direction::Up,
            pos.above(),
            vanilla_blocks::SNOW.default_state(),
        );
        assert!(snowy.get_value(&BlockStateProperties::SNOWY));

        let cleared = behavior.update_shape(
            snowy,
            &level,
            pos,
            Direction::Up,
            pos.above(),
            vanilla_blocks::AIR.default_state(),
        );
        assert!(!cleared.get_value(&BlockStateProperties::SNOWY));
    }
}
