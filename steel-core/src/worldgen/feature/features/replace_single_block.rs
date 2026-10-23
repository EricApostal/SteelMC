//! `minecraft:replace_single_block` feature (vanilla `ReplaceBlockFeature`).

use super::super::prelude::*;
use super::super::runner::FeatureDecorationRunner;
use crate::worldgen::template::StructureTemplate;

impl FeatureDecorationRunner {
    /// Tests each replacement target against the block at `origin`, in order,
    /// writing the first matching state. Mirrors vanilla
    /// `ReplaceBlockFeature.place`, always returning `true`.
    pub(in crate::worldgen::feature) fn place_replace_single_block_feature(
        region: &mut WorldGenRegion<'_>,
        registry: &Registry,
        random: &mut WorldgenRandom,
        replacements: &[BlockReplacement],
        origin: BlockPos,
    ) -> bool {
        let current = region.block_state(origin);
        for replacement in replacements {
            // Vanilla `BlockReplacement.target` is the shared `RuleTest` tree,
            // evaluated by the same code as structure-processor predicates.
            if StructureTemplate::rule_test_matches(
                registry,
                &replacement.target,
                current,
                origin,
                random,
            ) {
                let state = BlockStateProviderEvaluator::block_state_from_data(
                    registry,
                    &replacement.state,
                );
                let _ = region.set_block_state(origin, state, UpdateFlags::UPDATE_CLIENTS);
                break;
            }
        }
        true
    }
}
