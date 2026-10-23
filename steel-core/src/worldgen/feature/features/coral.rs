use steel_registry::vanilla_block_tags::BlockTag;

use super::super::prelude::*;
use super::super::runner::FeatureDecorationRunner;

impl FeatureDecorationRunner {
    /// Vanilla `CoralClawFeature.place`: places the configured placed feature
    /// at the origin, then walks claw branches placing that same feature.
    pub(in crate::worldgen::feature) fn place_coral_claw_feature(
        region: &mut WorldGenRegion<'_>,
        registry: &Registry,
        random: &mut WorldgenRandom,
        origin: BlockPos,
        feature: &PlacedFeatureRef,
        biome_zoom_seed: i64,
    ) -> bool {
        if !Self::place_placed_feature_ref(
            region,
            registry,
            random,
            origin,
            feature,
            biome_zoom_seed,
        ) {
            return false;
        }

        let claw_direction = Self::random_horizontal_direction(random);
        let n_branches = random.next_i32_bounded(2) + 2;
        let possible_directions = Self::shuffled_directions(
            random,
            [
                claw_direction,
                claw_direction.rotate_y_clockwise(),
                claw_direction.rotate_y_counter_clockwise(),
            ],
        );

        for branch_direction in possible_directions.into_iter().take(n_branches as usize) {
            let mut pos = origin.relative(branch_direction);
            let sideway_length = random.next_i32_bounded(2) + 1;
            let segment_direction;
            let inway_length;
            if branch_direction == claw_direction {
                segment_direction = claw_direction;
                inway_length = random.next_i32_bounded(3) + 2;
            } else {
                pos = pos.above();
                segment_direction = if random.next_i32_bounded(2) == 0 {
                    branch_direction
                } else {
                    Direction::Up
                };
                inway_length = random.next_i32_bounded(3) + 3;
            }

            // Vanilla: `for (i = 0; i < sidewayLength && place(mutPos); i++)`.
            let mut i = 0;
            while i < sideway_length
                && Self::place_placed_feature_ref(
                    region,
                    registry,
                    random,
                    pos,
                    feature,
                    biome_zoom_seed,
                )
            {
                i += 1;
                pos = pos.relative(segment_direction);
            }

            pos = pos.relative(segment_direction.opposite()).above();

            for _ in 0..inway_length {
                pos = pos.relative(claw_direction);
                if !Self::place_placed_feature_ref(
                    region,
                    registry,
                    random,
                    pos,
                    feature,
                    biome_zoom_seed,
                ) {
                    break;
                }

                if random.next_f32() < 0.25 {
                    pos = pos.above();
                }
            }
        }

        true
    }

    pub(in crate::worldgen::feature) fn place_coral_mushroom_feature(
        region: &mut WorldGenRegion<'_>,
        registry: &Registry,
        random: &mut WorldgenRandom,
        origin: BlockPos,
    ) -> bool {
        let Some(coral) = Self::random_block_from_tag(registry, random, &BlockTag::CORAL_BLOCKS)
        else {
            return false;
        };

        let state = coral.default_state();
        // Vanilla `CoralMushroomFeature` samples the extents in x, y, z order and
        // then the sink offset; each extent is 3..=5 and the sink is 1..=3.
        let x_extent = random.next_i32_bounded(3) + 3;
        let y_extent = random.next_i32_bounded(3) + 3;
        let z_extent = random.next_i32_bounded(3) + 3;
        let sink_value = random.next_i32_bounded(3) + 1;

        for x in 0..=x_extent {
            for y in 0..=y_extent {
                for z in 0..=z_extent {
                    let pos = origin.offset(x, y - sink_value, z);
                    if (y != 0 && y != y_extent
                        || x != 0 && x != x_extent && z != 0 && z != z_extent)
                        && ((x != 0 && x != x_extent) || (z != 0 && z != z_extent))
                        && (x == 0
                            || x == x_extent
                            || y == 0
                            || y == y_extent
                            || z == 0
                            || z == z_extent)
                        && random.next_f32() >= 0.1
                    {
                        let _ = Self::place_coral_block(region, registry, random, pos, state);
                    }
                }
            }
        }

        true
    }

    /// Vanilla `CoralTreeFeature.place`: places the configured placed feature
    /// along the trunk and branches instead of re-implementing the block writes.
    pub(in crate::worldgen::feature) fn place_coral_tree_feature(
        region: &mut WorldGenRegion<'_>,
        registry: &Registry,
        random: &mut WorldgenRandom,
        origin: BlockPos,
        feature: &PlacedFeatureRef,
        biome_zoom_seed: i64,
    ) -> bool {
        let mut pos = origin;
        let trunk_height = random.next_i32_bounded(3) + 1;
        for _ in 0..trunk_height {
            if !Self::place_placed_feature_ref(
                region,
                registry,
                random,
                pos,
                feature,
                biome_zoom_seed,
            ) {
                return true;
            }
            pos = pos.above();
        }

        let trunk_top = pos;
        let n_branches = random.next_i32_bounded(3) + 2;
        let directions = Self::shuffled_directions(random, Self::VANILLA_HORIZONTAL_DIRECTIONS);

        for branch_direction in directions.into_iter().take(n_branches as usize) {
            pos = trunk_top.relative(branch_direction);
            let branch_height = random.next_i32_bounded(5) + 2;
            let mut segment_length = 0;

            for j in 0..branch_height {
                if !Self::place_placed_feature_ref(
                    region,
                    registry,
                    random,
                    pos,
                    feature,
                    biome_zoom_seed,
                ) {
                    break;
                }
                segment_length += 1;
                pos = pos.above();
                if j == 0 || (segment_length >= 2 && random.next_f32() < 0.25) {
                    pos = pos.relative(branch_direction);
                    segment_length = 0;
                }
            }
        }

        true
    }

    fn place_coral_block(
        region: &mut WorldGenRegion<'_>,
        registry: &Registry,
        random: &mut WorldgenRandom,
        pos: BlockPos,
        state: BlockStateId,
    ) -> bool {
        let above = pos.above();
        let target_state = region.block_state(pos);
        if target_state.get_block() != &vanilla_blocks::WATER
            && !registry
                .blocks
                .is_in_tag(target_state.get_block(), &BlockTag::CORALS)
        {
            return false;
        }

        if region.block_state(above).get_block() != &vanilla_blocks::WATER {
            return false;
        }

        let _ = region.set_block_state(pos, state, UpdateFlags::UPDATE_ALL);
        if random.next_f32() < 0.25 {
            if let Some(coral) = Self::random_block_from_tag(registry, random, &BlockTag::CORALS) {
                let _ = region.set_block_state(
                    above,
                    coral.default_state(),
                    UpdateFlags::UPDATE_CLIENTS,
                );
            }
        } else if random.next_f32() < 0.05 {
            let pickles = random.next_i32_bounded(4) as u8 + 1;
            let sea_pickle = vanilla_blocks::SEA_PICKLE
                .default_state()
                .set_value(&BlockStateProperties::PICKLES, pickles);
            let _ = region.set_block_state(above, sea_pickle, UpdateFlags::UPDATE_CLIENTS);
        }

        for direction in Self::VANILLA_HORIZONTAL_DIRECTIONS {
            if random.next_f32() >= 0.2 {
                continue;
            }

            let relative_pos = pos.relative(direction);
            if region.block_state(relative_pos).get_block() != &vanilla_blocks::WATER {
                continue;
            }

            if let Some(coral) =
                Self::random_block_from_tag(registry, random, &BlockTag::WALL_CORALS)
            {
                let mut coral_fan_state = coral.default_state();
                if coral_fan_state
                    .try_get_value(&BlockStateProperties::HORIZONTAL_FACING)
                    .is_some()
                {
                    coral_fan_state = coral_fan_state
                        .set_value(&BlockStateProperties::HORIZONTAL_FACING, direction);
                }
                let _ = region.set_block_state(
                    relative_pos,
                    coral_fan_state,
                    UpdateFlags::UPDATE_CLIENTS,
                );
            }
        }

        true
    }

    pub(super) fn random_block_from_tag(
        registry: &Registry,
        random: &mut WorldgenRandom,
        tag: &Identifier,
    ) -> Option<BlockRef> {
        let blocks = registry.blocks.get_tag(tag)?;
        if blocks.is_empty() {
            return None;
        }

        let Ok(block_count) = i32::try_from(blocks.len()) else {
            panic!("block tag {tag} has too many entries to choose randomly");
        };
        Some(blocks[random.next_i32_bounded(block_count) as usize])
    }
}
