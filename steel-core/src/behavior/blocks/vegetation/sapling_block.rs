//! Vanilla `SaplingBlock`.
//!
//! Saplings survive on `#minecraft:supports_vegetation`, advance their `stage`
//! on random ticks and on bonemeal, and grow a tree once the stage is 1. The
//! tree choice mirrors vanilla's `TreeGrower` tables: an optional 2x2 "mega"
//! tree, then the regular list, preferring the bee-bearing variants when a
//! `#minecraft:flowers` block is nearby.
//!
//! Deviation: vanilla's `TreeGrower.growTree` places a `Feature` through
//! `ChunkGenerator`; Steel's only live-world tree entry point is
//! `worldgen::feature::FeatureDecorationRunner::place_tree_feature`, which is
//! what this behavior calls (same as `MangrovePropaguleBlock`).

use std::sync::{Arc, LazyLock};

use rand::{Rng, RngExt};
use steel_macros::block_behavior;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::blocks::properties::{BlockStateProperties, IntProperty};
use steel_registry::feature::{ConfiguredFeature, ConfiguredFeatureKind, TrunkPlacer};
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::{REGISTRY, vanilla_blocks, vanilla_configured_features};
use steel_utils::random::worldgen_random::WorldgenRandom;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, Direction};
use steel_worldgen::biomes::obfuscate_biome_seed;

use crate::behavior::block::BlockBehavior;
use crate::behavior::blocks::vegetation::Vegetation;
use crate::behavior::blocks::vegetation::bonemealable::Bonemealable;
use crate::behavior::blocks::vegetation::vegetation_block::survival_update_shape;
use crate::behavior::context::BlockPlaceContext;
use crate::world::{LevelReader, ScheduledTickAccess, World};
use crate::worldgen::feature::{FeatureDecorationRunner, no_nested_features};

use super::{BlockRef, default_surviving_state, survives_on_tag};

const STAGE: &IntProperty = &BlockStateProperties::STAGE;
/// Vanilla `SaplingBlock.BRIGHTNESS_FOR_SAPLING_GROWTH`.
const BRIGHTNESS_FOR_SAPLING_GROWTH: u8 = 9;
/// Vanilla `SaplingBlock.TICK_CHANCE_FOR_SAPLING_GROWTH`.
const TICK_CHANCE_FOR_SAPLING_GROWTH: i32 = 7;
/// Vanilla `SaplingBlock.isBonemealSuccess`.
const BONEMEAL_SUCCESS_CHANCE: f32 = 0.45;

/// One weighted configured tree in a `TreeGrower` list.
struct WeightedTree {
    feature: &'static LazyLock<ConfiguredFeature>,
    weight: i32,
}

const fn tree(feature: &'static LazyLock<ConfiguredFeature>, weight: i32) -> WeightedTree {
    WeightedTree { feature, weight }
}

/// Vanilla `TreeGrower` entry.
struct TreeGrower {
    trees: &'static [WeightedTree],
    mega_trees: &'static [WeightedTree],
    flower_trees: &'static [WeightedTree],
    shortest_tree: Option<&'static LazyLock<ConfiguredFeature>>,
}

/// Vanilla `TreeGrowers.OAK`.
const OAK_TREES: &[WeightedTree] = &[
    tree(&vanilla_configured_features::OAK, 9),
    tree(&vanilla_configured_features::FANCY_OAK, 1),
];
const OAK_FLOWERS: &[WeightedTree] = &[
    tree(&vanilla_configured_features::OAK_BEES_005, 9),
    tree(&vanilla_configured_features::FANCY_OAK_BEES_005, 1),
];
/// Vanilla `TreeGrowers.SPRUCE`.
const SPRUCE_TREES: &[WeightedTree] = &[tree(&vanilla_configured_features::SPRUCE, 1)];
const SPRUCE_MEGA: &[WeightedTree] = &[
    tree(&vanilla_configured_features::MEGA_SPRUCE, 1),
    tree(&vanilla_configured_features::MEGA_PINE, 1),
];
/// Vanilla `TreeGrowers.BIRCH`.
const BIRCH_TREES: &[WeightedTree] = &[tree(&vanilla_configured_features::BIRCH, 1)];
const BIRCH_FLOWERS: &[WeightedTree] = &[tree(&vanilla_configured_features::BIRCH_BEES_005, 1)];
/// Vanilla `TreeGrowers.JUNGLE`.
const JUNGLE_TREES: &[WeightedTree] = &[tree(&vanilla_configured_features::JUNGLE_TREE_NO_VINE, 1)];
const JUNGLE_MEGA: &[WeightedTree] = &[tree(&vanilla_configured_features::MEGA_JUNGLE_TREE, 1)];
/// Vanilla `TreeGrowers.ACACIA`.
const ACACIA_TREES: &[WeightedTree] = &[tree(&vanilla_configured_features::ACACIA, 1)];
/// Vanilla `TreeGrowers.CHERRY`.
const CHERRY_TREES: &[WeightedTree] = &[tree(&vanilla_configured_features::CHERRY, 1)];
const CHERRY_FLOWERS: &[WeightedTree] = &[tree(&vanilla_configured_features::CHERRY_BEES_005, 1)];
/// Vanilla `TreeGrowers.DARK_OAK`.
const DARK_OAK_MEGA: &[WeightedTree] = &[tree(&vanilla_configured_features::DARK_OAK, 1)];
/// Vanilla `TreeGrowers.PALE_OAK`.
const PALE_OAK_MEGA: &[WeightedTree] = &[tree(&vanilla_configured_features::PALE_OAK_BONEMEAL, 1)];
/// Vanilla `TreeGrowers.POPLAR`.
const POPLAR_TREES: &[WeightedTree] = &[
    tree(&vanilla_configured_features::RED_POPLAR, 1),
    tree(&vanilla_configured_features::ORANGE_POPLAR, 1),
    tree(&vanilla_configured_features::YELLOW_POPLAR, 1),
];

/// Vanilla `TreeGrower.removeSapling` flags 818: clients, known shape, no drops,
/// no block-entity side effects, and no on-place hook.
const REMOVE_SAPLING_FLAGS: UpdateFlags = UpdateFlags::UPDATE_CLIENTS
    .union(UpdateFlags::UPDATE_KNOWN_SHAPE)
    .union(UpdateFlags::UPDATE_SUPPRESS_DROPS)
    .union(UpdateFlags::UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS)
    .union(UpdateFlags::UPDATE_SKIP_ON_PLACE);

/// Vanilla `SaplingBlock` survival, random growth and bonemeal growth.
#[block_behavior]
pub struct SaplingBlock {
    block: BlockRef,
}

impl SaplingBlock {
    /// Creates a new sapling block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Returns vanilla's `TreeGrower` for this sapling, or `None` for blocks that
    /// are not saplings in vanilla (Steel also assigns this behavior to
    /// `bamboo_sapling`, which vanilla grows through `BambooSaplingBlock`).
    fn grower(&self) -> Option<TreeGrower> {
        let block = self.block;
        let grower = if block == &vanilla_blocks::OAK_SAPLING {
            TreeGrower {
                trees: OAK_TREES,
                mega_trees: &[],
                flower_trees: OAK_FLOWERS,
                shortest_tree: Some(&vanilla_configured_features::OAK),
            }
        } else if block == &vanilla_blocks::SPRUCE_SAPLING {
            TreeGrower {
                trees: SPRUCE_TREES,
                mega_trees: SPRUCE_MEGA,
                flower_trees: &[],
                shortest_tree: Some(&vanilla_configured_features::SPRUCE),
            }
        } else if block == &vanilla_blocks::BIRCH_SAPLING {
            TreeGrower {
                trees: BIRCH_TREES,
                mega_trees: &[],
                flower_trees: BIRCH_FLOWERS,
                shortest_tree: Some(&vanilla_configured_features::BIRCH),
            }
        } else if block == &vanilla_blocks::JUNGLE_SAPLING {
            TreeGrower {
                trees: JUNGLE_TREES,
                mega_trees: JUNGLE_MEGA,
                flower_trees: &[],
                shortest_tree: Some(&vanilla_configured_features::JUNGLE_TREE_NO_VINE),
            }
        } else if block == &vanilla_blocks::ACACIA_SAPLING {
            TreeGrower {
                trees: ACACIA_TREES,
                mega_trees: &[],
                flower_trees: &[],
                shortest_tree: Some(&vanilla_configured_features::ACACIA),
            }
        } else if block == &vanilla_blocks::CHERRY_SAPLING {
            TreeGrower {
                trees: CHERRY_TREES,
                mega_trees: &[],
                flower_trees: CHERRY_FLOWERS,
                shortest_tree: Some(&vanilla_configured_features::CHERRY),
            }
        } else if block == &vanilla_blocks::DARK_OAK_SAPLING {
            TreeGrower {
                trees: &[],
                mega_trees: DARK_OAK_MEGA,
                flower_trees: &[],
                shortest_tree: None,
            }
        } else if block == &vanilla_blocks::PALE_OAK_SAPLING {
            TreeGrower {
                trees: &[],
                mega_trees: PALE_OAK_MEGA,
                flower_trees: &[],
                shortest_tree: None,
            }
        } else if block == &vanilla_blocks::POPLAR_SAPLING {
            TreeGrower {
                trees: POPLAR_TREES,
                mega_trees: &[],
                flower_trees: &[],
                shortest_tree: Some(&vanilla_configured_features::RED_POPLAR),
            }
        } else {
            return None;
        };
        Some(grower)
    }

    /// Vanilla `WeightedList.getRandom`: weighted pick that consumes one roll.
    fn pick(
        entries: &'static [WeightedTree],
        rng: &mut dyn Rng,
    ) -> Option<&'static ConfiguredFeature> {
        let total_weight: i32 = entries.iter().map(|entry| entry.weight).sum();
        if total_weight <= 0 {
            return None;
        }

        let mut target = rng.random_range(0..total_weight);
        for entry in entries {
            if target < entry.weight {
                return Some(entry.feature);
            }
            target -= entry.weight;
        }
        None
    }

    /// Vanilla `TreeGrower.hasFlowers`: any `#minecraft:flowers` block in the
    /// 5x3x5 volume centered on the sapling.
    fn has_flowers(world: &dyn LevelReader, pos: BlockPos) -> bool {
        BlockPos::between_closed(pos.offset(-2, -1, -2), pos.offset(2, 1, 2)).any(|check_pos| {
            world
                .get_block_state(check_pos)
                .get_block()
                .has_tag(&BlockTag::FLOWERS)
        })
    }

    /// Vanilla `TreeGrower.findTwoByTwoSaplingPos`: the lower-left offset of a
    /// 2x2 block of identical saplings, if any. Vanilla iterates
    /// `for (int i = 0; i >= -1; --i)`, i.e. 0 then -1, which in Rust is
    /// `(-1..=0).rev()`; the tempting `(0..=-1).rev()` is an empty range and
    /// would make every 2x2 tree (dark oak, mega spruce, mega jungle) un-growable.
    fn find_two_by_two(
        world: &dyn LevelReader,
        pos: BlockPos,
        state: BlockStateId,
    ) -> Option<(i32, i32)> {
        let block = state.get_block();
        for dx in (-1..=0).rev() {
            for dz in (-1..=0).rev() {
                let matches = [
                    pos.offset(dx, 0, dz),
                    pos.offset(dx + 1, 0, dz),
                    pos.offset(dx, 0, dz + 1),
                    pos.offset(dx + 1, 0, dz + 1),
                ]
                .iter()
                .all(|sapling_pos| world.get_block_state(*sapling_pos).get_block() == block);
                if matches {
                    return Some((dx, dz));
                }
            }
        }
        None
    }

    /// Vanilla `TreeGrower.removeSapling`: replaces the sapling with the legacy
    /// block of its fluid state.
    fn remove_sapling(world: &Arc<World>, pos: BlockPos) {
        let fluid_state = world.get_block_state(pos).get_fluid_state();
        world.set_block(pos, fluid_state.create_legacy_block(), REMOVE_SAPLING_FLAGS);
    }

    /// Vanilla `TreeGrower.resetSaplings`.
    fn restore_saplings(world: &Arc<World>, saplings: &[(BlockStateId, BlockPos)]) {
        for (state, sapling_pos) in saplings {
            world.set_block(*sapling_pos, *state, UpdateFlags::UPDATE_NONE);
        }
    }

    /// Vanilla `Feature.place` for a configured tree.
    fn place_tree(
        world: &Arc<World>,
        feature: &'static ConfiguredFeature,
        pos: BlockPos,
        rng: &mut dyn Rng,
    ) -> bool {
        let ConfiguredFeatureKind::Tree(config) = &feature.kind else {
            return false;
        };

        let mut worldgen_random = WorldgenRandom::from_seed(rng.random());
        let mut level = Arc::clone(world);
        let placed = FeatureDecorationRunner::place_tree_feature(
            &mut level,
            &REGISTRY,
            &mut worldgen_random,
            config,
            pos,
            obfuscate_biome_seed(world.seed()),
            no_nested_features,
        );
        if placed && world.get_block_state(pos).is_air() {
            world.send_block_updated(pos);
        }
        placed
    }

    /// Vanilla `TreeGrower.growTree`.
    fn grow_tree(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        state: BlockStateId,
        rng: &mut dyn Rng,
    ) -> bool {
        let Some(grower) = self.grower() else {
            return false;
        };

        if let Some(mega_feature) = Self::pick(grower.mega_trees, rng)
            && let Some((dx, dz)) = Self::find_two_by_two(world.as_ref(), pos, state)
        {
            let saplings = [
                (state, pos.offset(dx, 0, dz)),
                (state, pos.offset(dx + 1, 0, dz)),
                (state, pos.offset(dx, 0, dz + 1)),
                (state, pos.offset(dx + 1, 0, dz + 1)),
            ];
            Self::remove_saplings(world, &saplings);
            if Self::place_tree(world, mega_feature, pos.offset(dx, 0, dz), rng) {
                return true;
            }
            Self::restore_saplings(world, &saplings);
            return false;
        }

        let has_flowers = Self::has_flowers(world.as_ref(), pos);
        let feature = if has_flowers && !grower.flower_trees.is_empty() {
            Self::pick(grower.flower_trees, rng)
        } else {
            Self::pick(grower.trees, rng)
        };
        let Some(feature) = feature else {
            return false;
        };

        Self::remove_sapling(world, pos);
        if Self::place_tree(world, feature, pos, rng) {
            true
        } else {
            Self::restore_saplings(world, &[(state, pos)]);
            false
        }
    }

    /// Vanilla `TreeGrower.removeSaplings`.
    fn remove_saplings(world: &Arc<World>, saplings: &[(BlockStateId, BlockPos)]) {
        for (_, sapling_pos) in saplings {
            Self::remove_sapling(world, *sapling_pos);
        }
    }

    /// Vanilla `SaplingBlock.advanceTree`.
    fn advance_tree(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        state: BlockStateId,
        rng: &mut dyn Rng,
    ) {
        if state.get_value(STAGE) == 0 {
            world.set_block(pos, state.set_value(STAGE, 1), UpdateFlags::UPDATE_NONE);
        } else {
            self.grow_tree(world, pos, state, rng);
        }
    }

    /// Vanilla `TreeGrower.getMinimumHeight`: the shortest tree's trunk base height.
    fn minimum_height(&self) -> i32 {
        let Some(shortest) = self.grower().and_then(|grower| grower.shortest_tree) else {
            return 0;
        };
        let ConfiguredFeatureKind::Tree(config) = &shortest.kind else {
            return 0;
        };
        trunk_base_height(&config.trunk_placer)
    }
}

/// Vanilla `TrunkPlacer.getBaseHeight` for every placer kind.
const fn trunk_base_height(placer: &TrunkPlacer) -> i32 {
    match placer {
        TrunkPlacer::Straight(base)
        | TrunkPlacer::Giant(base)
        | TrunkPlacer::Fancy(base)
        | TrunkPlacer::Forking(base)
        | TrunkPlacer::DarkOak(base)
        | TrunkPlacer::MegaJungle(base) => base.base_height,
        TrunkPlacer::Bending(placer) => placer.base_height,
        TrunkPlacer::UpwardsBranching(placer) => placer.base_height,
        TrunkPlacer::Cherry(placer) => placer.base_height,
        TrunkPlacer::Poplar(placer) => placer.base_height,
    }
}

impl BlockBehavior for SaplingBlock {
    fn can_survive(&self, _state: BlockStateId, world: &dyn LevelReader, pos: BlockPos) -> bool {
        survives_on_tag(world, pos, &BlockTag::SUPPORTS_VEGETATION)
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
        survival_update_shape(self, state, world, pos)
    }

    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        default_surviving_state(self.block, self, context)
    }

    fn random_tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        if world.max_local_raw_brightness(pos.above(), world.sky_darkening())
            >= BRIGHTNESS_FOR_SAPLING_GROWTH
            && rand::random_range(0..TICK_CHANCE_FOR_SAPLING_GROWTH) == 0
        {
            let mut rng = rand::rng();
            self.advance_tree(world, pos, state, &mut rng);
        }
    }

    fn as_bonemealable(&self) -> Option<&dyn Bonemealable> {
        Some(self)
    }
}

impl Vegetation for SaplingBlock {}

impl Bonemealable for SaplingBlock {
    fn is_valid_bonemeal_target(
        &self,
        state: BlockStateId,
        world: &dyn LevelReader,
        pos: BlockPos,
    ) -> bool {
        let Some(grower) = self.grower() else {
            return false;
        };

        // Vanilla `TreeGrower.canGrow`: a grower whose only source is a mega tree
        // needs a 2x2 sapling square; every other grower can always grow.
        if grower.trees.is_empty()
            && !grower.mega_trees.is_empty()
            && Self::find_two_by_two(world, pos, state).is_none()
        {
            return false;
        }

        !world.is_outside_build_height(pos.y() + self.minimum_height())
    }

    fn is_bonemeal_success(
        &self,
        _state: BlockStateId,
        _world: &Arc<World>,
        rng: &mut dyn Rng,
        _pos: BlockPos,
    ) -> bool {
        rng.random::<f32>() < BONEMEAL_SUCCESS_CHANCE
    }

    fn perform_bonemeal(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        rng: &mut dyn Rng,
        pos: BlockPos,
    ) {
        self.advance_tree(world, pos, state, rng);
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, vanilla_blocks};
    use steel_utils::BlockPos;

    use super::*;
    use crate::behavior::init_behaviors;
    use crate::test_support::TestLevel;

    #[test]
    fn sapling_requires_vegetation_support() {
        init_vanilla_registry();
        init_behaviors();
        let behavior = SaplingBlock::new(&vanilla_blocks::OAK_SAPLING);
        let state = vanilla_blocks::OAK_SAPLING.default_state();
        let pos = BlockPos::new(0, 64, 0);

        let dirt =
            TestLevel::default().with_block(pos.below(), vanilla_blocks::DIRT.default_state());
        assert!(behavior.can_survive(state, &dirt, pos));

        let air = TestLevel::default().with_block(pos.below(), vanilla_blocks::AIR.default_state());
        assert!(!behavior.can_survive(state, &air, pos));
    }

    #[test]
    fn bonemeal_target_requires_build_height_for_the_shortest_tree() {
        init_vanilla_registry();
        init_behaviors();
        let behavior = SaplingBlock::new(&vanilla_blocks::OAK_SAPLING);
        let state = vanilla_blocks::OAK_SAPLING.default_state();
        let level = TestLevel::default();
        let pos = BlockPos::new(0, 64, 0);

        assert!(behavior.is_valid_bonemeal_target(state, &level, pos));
        assert!(behavior.minimum_height() > 0);
        let top = BlockPos::new(0, level.max_y_exclusive() - 1, 0);
        assert!(!behavior.is_valid_bonemeal_target(state, &level, top));
    }

    #[test]
    fn bamboo_sapling_has_no_tree_grower() {
        init_vanilla_registry();
        init_behaviors();
        assert!(
            SaplingBlock::new(&vanilla_blocks::BAMBOO_SAPLING)
                .grower()
                .is_none()
        );
    }

    /// Vanilla `TreeGrower.findTwoByTwoSaplingPos` walks `0` then `-1` on both axes
    /// (`for (int i = 0; i >= -1; --i)`), so a 2x2 patch is found for the patch corner
    /// at the sapling and for the sapling at the patch's south-east corner. The
    /// empty-range form `(0..=-1).rev()` would find nothing and silently make dark
    /// oak, mega spruce and mega jungle un-growable.
    #[test]
    fn two_by_two_detection_covers_both_offsets() {
        init_vanilla_registry();
        init_behaviors();
        let block = &vanilla_blocks::DARK_OAK_SAPLING;
        let state = block.default_state();

        // Patch corner at (0, 0): the sapling sits at the north-west cell.
        let corner = TestLevel::default()
            .with_block(BlockPos::new(0, 64, 0), state)
            .with_block(BlockPos::new(1, 64, 0), state)
            .with_block(BlockPos::new(0, 64, 1), state)
            .with_block(BlockPos::new(1, 64, 1), state);
        assert_eq!(
            SaplingBlock::find_two_by_two(&corner, BlockPos::new(0, 64, 0), state),
            Some((0, 0))
        );

        // The sapling at the patch's south-east corner must also see the patch,
        // which needs the `-1` offsets.
        assert_eq!(
            SaplingBlock::find_two_by_two(&corner, BlockPos::new(1, 64, 1), state),
            Some((-1, -1))
        );
    }
}
