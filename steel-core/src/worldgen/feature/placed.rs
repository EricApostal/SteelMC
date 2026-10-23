#![expect(
    clippy::too_many_lines,
    reason = "placed feature modifier chaining mirrors vanilla placement flow"
)]

use super::prelude::*;
use super::runner::FeatureDecorationRunner;
use smallvec::SmallVec;

#[derive(Clone, Copy)]
enum BiomeFilterMode<'a> {
    Check(Option<&'a Identifier>),
    Ignore,
}

impl FeatureDecorationRunner {
    pub(super) fn place_placed_feature_entry(
        region: &mut WorldGenRegion<'_>,
        registry: &Registry,
        random: &mut WorldgenRandom,
        origin: BlockPos,
        feature: PlacedFeatureEntryRef,
        biome_zoom_seed: i64,
    ) -> bool {
        assert!(
            feature.try_id().is_some(),
            "top-level placed feature {} is not registered",
            feature.key
        );
        Self::place_placed_feature_data(
            region,
            registry,
            random,
            origin,
            &feature.data,
            Some(&feature.key),
            biome_zoom_seed,
        )
    }

    pub(super) fn place_placed_feature_data(
        region: &mut WorldGenRegion<'_>,
        registry: &Registry,
        random: &mut WorldgenRandom,
        origin: BlockPos,
        feature: &PlacedFeatureData,
        biome_filter: Option<&Identifier>,
        biome_zoom_seed: i64,
    ) -> bool {
        Self::place_placed_feature_from_modifier(
            region,
            registry,
            random,
            origin,
            feature,
            BiomeFilterMode::Check(biome_filter),
            biome_zoom_seed,
            0,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "threading vanilla placed-feature stream state explicitly"
    )]
    fn place_placed_feature_from_modifier(
        region: &mut WorldGenRegion<'_>,
        registry: &Registry,
        random: &mut WorldgenRandom,
        origin: BlockPos,
        feature: &PlacedFeatureData,
        biome_filter: BiomeFilterMode<'_>,
        biome_zoom_seed: i64,
        modifier_index: usize,
    ) -> bool {
        let Some(modifier) = feature.placement.get(modifier_index) else {
            return Self::place_configured_feature(
                region,
                registry,
                random,
                &feature.feature,
                origin,
                biome_zoom_seed,
            );
        };

        let positions = Self::apply_placement_modifier(
            region,
            registry,
            random,
            origin,
            biome_filter,
            biome_zoom_seed,
            modifier,
        );
        let mut placed = false;
        for position in positions {
            if Self::place_placed_feature_from_modifier(
                region,
                registry,
                random,
                position,
                feature,
                biome_filter,
                biome_zoom_seed,
                modifier_index + 1,
            ) {
                placed = true;
            }
        }
        placed
    }

    /// Applies one vanilla `PlacementModifier` to `origin`, returning the
    /// positions the next modifier in the chain runs on.
    ///
    /// Mirrors the `Consumer<BlockPos>` output of `PlacementModifier.modify`.
    /// `RandomlySelected` recurses here so it accepts any nested modifier
    /// (`RandomlySelectedPlacement.java:33-35`), not only `Offset`.
    fn apply_placement_modifier(
        region: &mut WorldGenRegion<'_>,
        registry: &Registry,
        random: &mut WorldgenRandom,
        origin: BlockPos,
        biome_filter: BiomeFilterMode<'_>,
        biome_zoom_seed: i64,
        modifier: &PlacementModifier,
    ) -> SmallVec<[BlockPos; 4]> {
        match modifier {
            PlacementModifier::Biome => {
                let biome_allows = match biome_filter {
                    BiomeFilterMode::Check(feature_key) => Self::biome_allows_feature(
                        region,
                        registry,
                        biome_zoom_seed,
                        origin,
                        feature_key,
                    ),
                    BiomeFilterMode::Ignore => true,
                };

                if biome_allows {
                    SmallVec::from_elem(origin, 1)
                } else {
                    SmallVec::new()
                }
            }
            PlacementModifier::BlockPredicateFilter { predicate } => {
                if BlockStateProviderEvaluator::test_block_predicate(
                    region, registry, predicate, origin,
                ) {
                    SmallVec::from_elem(origin, 1)
                } else {
                    SmallVec::new()
                }
            }
            PlacementModifier::Count { count } => {
                let count = usize::try_from(count.sample(random)).unwrap_or(0);
                SmallVec::from_elem(origin, count)
            }
            PlacementModifier::CountOnEveryLayer { count } => {
                Self::count_on_every_layer_positions(region, random, origin, count)
                    .into_iter()
                    .collect()
            }
            PlacementModifier::EnvironmentScan {
                direction_of_search,
                target_condition,
                allowed_search_condition,
                max_steps,
            } => Self::environment_scan_position(
                region,
                registry,
                origin,
                *direction_of_search,
                target_condition,
                allowed_search_condition.as_ref(),
                *max_steps,
            )
            .map_or_else(SmallVec::new, |position| SmallVec::from_elem(position, 1)),
            PlacementModifier::FixedPlacement { positions } => {
                let chunk_x = SectionPos::block_to_section_coord(origin.x());
                let chunk_z = SectionPos::block_to_section_coord(origin.z());
                let mut result = SmallVec::new();
                for position in positions {
                    let position = BlockPos::new(position[0], position[1], position[2]);
                    if chunk_x != SectionPos::block_to_section_coord(position.x())
                        || chunk_z != SectionPos::block_to_section_coord(position.z())
                    {
                        continue;
                    }
                    result.push(position);
                }
                result
            }
            PlacementModifier::HeightRange { height } => SmallVec::from_elem(
                BlockPos::new(
                    origin.x(),
                    height.sample(
                        random,
                        region.generation_min_y(),
                        region.generation_height(),
                        region.sea_level(),
                    ),
                    origin.z(),
                ),
                1,
            ),
            PlacementModifier::Heightmap { heightmap } => {
                let height = region.height_at(
                    Self::feature_heightmap_type(*heightmap),
                    origin.x(),
                    origin.z(),
                );
                if height > region.min_y() {
                    SmallVec::from_elem(BlockPos::new(origin.x(), height, origin.z()), 1)
                } else {
                    SmallVec::new()
                }
            }
            PlacementModifier::InSquare => SmallVec::from_elem(
                BlockPos::new(
                    origin.x() + random.next_i32_bounded(16),
                    origin.y(),
                    origin.z() + random.next_i32_bounded(16),
                ),
                1,
            ),
            PlacementModifier::NoiseBasedCount {
                noise_to_count_ratio,
                noise_factor,
                noise_offset,
            } => {
                let noise = Self::biome_info_noise_value(
                    f64::from(origin.x()) / *noise_factor,
                    f64::from(origin.z()) / *noise_factor,
                );
                let count =
                    ((noise + *noise_offset) * f64::from(*noise_to_count_ratio)).ceil() as i32;
                let count = usize::try_from(count).unwrap_or(0);
                SmallVec::from_elem(origin, count)
            }
            PlacementModifier::NoiseThresholdCount {
                noise_level,
                below_noise,
                above_noise,
            } => {
                let noise = Self::biome_info_noise_value(
                    f64::from(origin.x()) / 200.0,
                    f64::from(origin.z()) / 200.0,
                );
                let count = if noise < *noise_level {
                    *below_noise
                } else {
                    *above_noise
                };
                let count = usize::try_from(count).unwrap_or(0);
                SmallVec::from_elem(origin, count)
            }
            PlacementModifier::RandomOffset {
                xz_spread,
                y_spread,
            } => SmallVec::from_elem(
                BlockPos::new(
                    origin.x() + xz_spread.sample(random),
                    origin.y() + y_spread.sample(random),
                    origin.z() + xz_spread.sample(random),
                ),
                1,
            ),
            PlacementModifier::RarityFilter { chance } => {
                assert!(
                    *chance > 0,
                    "rarity filter chance must be positive, got {chance}"
                );
                if random.next_f32() < 1.0 / (*chance as f32) {
                    SmallVec::from_elem(origin, 1)
                } else {
                    SmallVec::new()
                }
            }
            PlacementModifier::SurfaceRelativeThresholdFilter {
                heightmap,
                min_inclusive,
                max_inclusive,
            } => {
                let surface_y = i64::from(region.height_at(
                    Self::feature_heightmap_type(*heightmap),
                    origin.x(),
                    origin.z(),
                ));
                let min_y = surface_y + i64::from(min_inclusive.unwrap_or(i32::MIN));
                let max_y = surface_y + i64::from(max_inclusive.unwrap_or(i32::MAX));
                let origin_y = i64::from(origin.y());
                if min_y <= origin_y && origin_y <= max_y {
                    SmallVec::from_elem(origin, 1)
                } else {
                    SmallVec::new()
                }
            }
            PlacementModifier::SurfaceWaterDepthFilter { max_water_depth } => {
                let ocean_floor =
                    region.height_at(HeightmapType::OceanFloor, origin.x(), origin.z());
                let surface = region.height_at(HeightmapType::WorldSurface, origin.x(), origin.z());
                if surface - ocean_floor <= *max_water_depth {
                    SmallVec::from_elem(origin, 1)
                } else {
                    SmallVec::new()
                }
            }
            PlacementModifier::Offset { x, y, z } => SmallVec::from_elem(
                BlockPos::new(
                    origin.x() + x.sample(random),
                    origin.y() + y.sample(random),
                    origin.z() + z.sample(random),
                ),
                1,
            ),
            PlacementModifier::RandomChance { chance } => {
                if random.next_f32() < *chance {
                    SmallVec::from_elem(origin, 1)
                } else {
                    SmallVec::new()
                }
            }
            PlacementModifier::RandomlySelected { placements } => {
                assert!(
                    !placements.is_empty(),
                    "vanilla randomly selected placement requires a non-empty list"
                );
                let Ok(count) = i32::try_from(placements.len()) else {
                    panic!("vanilla randomly selected placement count exceeds i32")
                };

                let index = random.next_i32_bounded(count) as usize;
                Self::apply_placement_modifier(
                    region,
                    registry,
                    random,
                    origin,
                    biome_filter,
                    biome_zoom_seed,
                    &placements[index],
                )
            }
            PlacementModifier::Cuboid {
                xz_size,
                y_size,
                include_edges,
                include_interior,
            } => {
                let height = y_size.sample(random);
                let width = xz_size.sample(random);
                let length = xz_size.sample(random);
                let mut result = SmallVec::new();
                for x in 0..=width {
                    for y in 0..=height {
                        for z in 0..=length {
                            let included =
                                (*include_edges || x != 0 && x != width || y != 0 && y != height)
                                    && (*include_edges
                                        || z != 0 && z != length
                                        || y != 0 && y != height)
                                    && (*include_edges
                                        || x != 0 && x != width
                                        || z != 0 && z != length)
                                    && (*include_interior
                                        || x == 0
                                        || x == width
                                        || y == 0
                                        || y == height
                                        || z == 0
                                        || z == length);
                            if included {
                                result.push(BlockPos::new(
                                    origin.x() + x,
                                    origin.y() + y,
                                    origin.z() + z,
                                ));
                            }
                        }
                    }
                }
                result
            }
        }
    }

    pub(super) fn place_placed_feature_ref(
        region: &mut WorldGenRegion<'_>,
        registry: &Registry,
        random: &mut WorldgenRandom,
        origin: BlockPos,
        feature: &PlacedFeatureRef,
        biome_zoom_seed: i64,
    ) -> bool {
        let feature_data = match feature {
            PlacedFeatureRef::Reference(feature) => &feature.data,
            PlacedFeatureRef::Inline(feature) => feature,
        };

        Self::place_placed_feature_data(
            region,
            registry,
            random,
            origin,
            feature_data,
            None,
            biome_zoom_seed,
        )
    }

    /// Places a placed feature for a flat generator, bypassing the key-based biome check.
    ///
    /// Vanilla's flat generator compares the biome by identity against
    /// `FlatLevelGeneratorSettings.adjustGenerationSettings`'s already-substituted
    /// stage list, so the key-based check must be bypassed here: the stage list is
    /// fully controlled by the generator and every entry in it is allowed.
    pub(super) fn place_flat_feature(
        region: &mut WorldGenRegion<'_>,
        registry: &Registry,
        random: &mut WorldgenRandom,
        origin: BlockPos,
        feature: &PlacedFeatureRef,
        biome_zoom_seed: i64,
    ) -> bool {
        let feature_data = match feature {
            PlacedFeatureRef::Reference(feature) => &feature.data,
            PlacedFeatureRef::Inline(feature) => feature,
        };

        Self::place_placed_feature_from_modifier(
            region,
            registry,
            random,
            origin,
            feature_data,
            BiomeFilterMode::Ignore,
            biome_zoom_seed,
            0,
        )
    }

    pub(crate) fn place_structure_pool_feature(
        region: &mut WorldGenRegion<'_>,
        registry: &Registry,
        random: &mut WorldgenRandom,
        origin: BlockPos,
        feature_key: &Identifier,
        biome_zoom_seed: i64,
    ) -> bool {
        let Some(feature) = registry.placed_features.by_key(feature_key) else {
            panic!("template pool references unknown placed feature {feature_key}");
        };

        Self::place_placed_feature_from_modifier(
            region,
            registry,
            random,
            origin,
            &feature.data,
            BiomeFilterMode::Ignore,
            biome_zoom_seed,
            0,
        )
    }
}
