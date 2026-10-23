use super::super::prelude::*;
use super::super::runner::FeatureDecorationRunner;

impl FeatureDecorationRunner {
    pub(in crate::worldgen::feature) fn place_underwater_magma_feature(
        region: &mut WorldGenRegion<'_>,
        random: &mut WorldgenRandom,
        config: &UnderwaterMagmaConfiguration,
        origin: BlockPos,
    ) -> bool {
        let Some(floor_y) = Self::underwater_magma_floor_y(region, origin, config) else {
            return false;
        };

        let floor_pos = origin.at_y(floor_y);
        let radius = config.placement_radius_around_floor;
        let mut placed = false;

        Self::for_each_vanilla_between_closed(
            floor_pos.offset(-radius, -radius, -radius),
            floor_pos.offset(radius, radius, radius),
            |pos| {
                if random.next_f32() < config.placement_probability_per_valid_position
                    && Self::underwater_magma_valid_placement(region, pos)
                {
                    // Vanilla maps each surviving position to `1` after writing the
                    // magma block, ignoring `setBlock`'s result, so a valid position
                    // always counts towards the returned success value.
                    let _ = region.set_block_state(
                        pos,
                        vanilla_blocks::MAGMA_BLOCK.default_state(),
                        UpdateFlags::UPDATE_CLIENTS,
                    );
                    placed = true;
                }
            },
        );

        placed
    }

    fn underwater_magma_floor_y(
        region: &WorldGenRegion<'_>,
        origin: BlockPos,
        config: &UnderwaterMagmaConfiguration,
    ) -> Option<i32> {
        if region.block_state(origin).get_block() != &vanilla_blocks::WATER {
            return None;
        }

        let mut pos = origin;
        for _ in 1..config.floor_search_range {
            if region.block_state(pos).get_block() != &vanilla_blocks::WATER {
                break;
            }
            pos = pos.below();
        }

        if region.block_state(pos).get_block() == &vanilla_blocks::WATER {
            return None;
        }

        Some(pos.y())
    }

    fn underwater_magma_valid_placement(region: &WorldGenRegion<'_>, pos: BlockPos) -> bool {
        let state = region.block_state(pos);
        if Self::underwater_magma_is_water_or_air(state)
            || Self::underwater_magma_visible_from_outside(region, pos.below())
        {
            return false;
        }

        for direction in Self::VANILLA_HORIZONTAL_DIRECTIONS {
            if Self::underwater_magma_visible_from_outside(region, pos.relative(direction)) {
                return false;
            }
        }

        true
    }

    fn underwater_magma_is_water_or_air(state: BlockStateId) -> bool {
        state.get_block() == &vanilla_blocks::WATER || state.is_air()
    }

    /// Vanilla `UnderwaterMagmaFeature.isVisibleFromOutside`.
    ///
    /// Vanilla reads the neighbour's face occlusion shape for the covered
    /// direction and then tests it with `Block.isShapeFullBlock`, so the whole
    /// occlusion shape has to be a full block — a partial shape is always
    /// considered visible.
    fn underwater_magma_visible_from_outside(region: &WorldGenRegion<'_>, pos: BlockPos) -> bool {
        let face_occlusion_shape = region.block_state(pos).get_occlusion_shape();
        !shapes::is_shape_full_block(face_occlusion_shape)
    }
}
