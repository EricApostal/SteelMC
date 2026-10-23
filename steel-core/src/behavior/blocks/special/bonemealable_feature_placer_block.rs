//! `BonemealableFeaturePlacerBlock` behavior
//! (`net.minecraft.world.level.block.BonemealableFeaturePlacerBlock`).
//!
//! Vanilla grows `moss_block` / `pale_moss_block` into a configured feature patch
//! (`minecraft:moss_patch_bonemeal` for moss, `minecraft:pale_moss_patch_bonemeal` for pale
//! moss) when bonemealed. This block exposes the full bonemeal-surface contract (valid target,
//! success, feature name) and delegates placement to the worldgen feature plumbing.
//!
//! Deviation: vanilla `BonemealableFeaturePlacerBlock.performBonemeal` places the configured
//! feature through `Feature.place` on the live level; Steel's `FeatureDecorationRunner` only
//! exposes runtime (`LevelAccessor`) placers for tree and huge-mushroom features, so the
//! `VegetationPatch` kind of `moss_patch_bonemeal`/`pale_moss_patch_bonemeal` cannot be placed
//! outside chunk generation (owner: worldgen lane, `worldgen/feature/configured.rs`).

use std::sync::Arc;

use rand::Rng;
use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::vanilla_blocks;
use steel_utils::{BlockPos, BlockStateId};

use crate::behavior::blocks::vegetation::bonemealable::{BonemealAction, Bonemealable};
use crate::behavior::{BlockBehavior, BlockPlaceContext};
use crate::world::{LevelReader, World};

/// `BonemealableFeaturePlacerBlock` behavior.
///
/// Vanilla stores the `Feature` resource key per block at construction time; Steel resolves the
/// feature name from the block: `moss_block` grows `moss_patch_bonemeal` and `pale_moss_block`
/// grows `pale_moss_patch_bonemeal`.
#[block_behavior]
pub struct BonemealableFeaturePlacerBlock {
    block: BlockRef,
}

impl BonemealableFeaturePlacerBlock {
    /// Creates the behavior for `block`.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Returns the configured feature this block grows (vanilla `feature` field).
    const fn configured_feature(&self) -> &'static steel_registry::feature::ConfiguredFeature {
        if std::ptr::eq(self.block, &vanilla_blocks::PALE_MOSS_BLOCK) {
            &steel_registry::vanilla_configured_features::PALE_MOSS_PATCH_BONEMEAL
        } else {
            &steel_registry::vanilla_configured_features::MOSS_PATCH_BONEMEAL
        }
    }
}

impl BlockBehavior for BonemealableFeaturePlacerBlock {
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(self.block.default_state())
    }

    fn as_bonemealable(&self) -> Option<&dyn Bonemealable> {
        Some(self)
    }
}

impl Bonemealable for BonemealableFeaturePlacerBlock {
    /// Vanilla `BonemealableFeaturePlacerBlock.isValidBonemealTarget`.
    fn is_valid_bonemeal_target(
        &self,
        _state: BlockStateId,
        world: &dyn LevelReader,
        pos: BlockPos,
    ) -> bool {
        world.get_block_state(pos.above()).is_air()
    }

    /// Vanilla `BonemealableFeaturePlacerBlock.isBonemealSuccess`.
    fn is_bonemeal_success(
        &self,
        _state: BlockStateId,
        _world: &Arc<World>,
        _rng: &mut dyn Rng,
        _pos: BlockPos,
    ) -> bool {
        true
    }

    /// Vanilla `BonemealableFeaturePlacerBlock.performBonemeal`.
    ///
    /// The configured feature is resolved (see [`Self::configured_feature`]); placing it at
    /// runtime is not yet supported by the worldgen lane (see the module deviation note).
    fn perform_bonemeal(
        &self,
        _state: BlockStateId,
        _world: &Arc<World>,
        _rng: &mut dyn Rng,
        _pos: BlockPos,
    ) {
        let _ = self.configured_feature();
    }

    fn bonemeal_action_type(&self) -> BonemealAction {
        BonemealAction::NeighborSpreader
    }
}
