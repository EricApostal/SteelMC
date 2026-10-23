//! `minecraft:fill_layer` feature (vanilla `FillLayerFeature`).

use super::super::prelude::*;
use super::super::runner::FeatureDecorationRunner;

impl FeatureDecorationRunner {
    /// Fills a 16x16 chunk-layer band at `region.min_y() + config.height`.
    ///
    /// Mirrors vanilla: only writes into air blocks, always returns `true`.
    pub(in crate::worldgen::feature) fn place_fill_layer_feature(
        region: &mut WorldGenRegion<'_>,
        registry: &Registry,
        config: &FillLayerConfiguration,
        origin: BlockPos,
    ) -> bool {
        let state = BlockStateProviderEvaluator::block_state_from_data(registry, &config.state);
        let y = region.min_y() + config.height;
        for dx in 0..16 {
            for dz in 0..16 {
                let pos = BlockPos::new(origin.x() + dx, y, origin.z() + dz);
                if region.block_state(pos).is_air() {
                    let _ = region.set_block_state(pos, state, UpdateFlags::UPDATE_CLIENTS);
                }
            }
        }
        true
    }
}
