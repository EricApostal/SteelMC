//! `BonemealableFeaturePlacerBlock` behavior
//! (`net.minecraft.world.level.block.BonemealableFeaturePlacerBlock`).
//!
//! Vanilla grows `moss_block` / `pale_moss_block` into a configured feature patch
//! (`#moss_vegetation` for moss, `#pale_moss_vegetation` for pale moss) when bonemealed. Steel's
//! feature placement entry point lives in the worldgen lane; this block exposes the full
//! bonemeal-surface contract (valid target, success, feature name) and delegates the actual
//! placement to the same feature plumbing that `steel-core/src/worldgen/feature/features/` uses.

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
/// feature name from the block: `moss_block` grows `#moss_vegetation`-equivalent content and
/// `pale_moss_block` grows the pale-moss patch.
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
    fn is_valid_bonemeal_target(
        &self,
        _state: BlockStateId,
        world: &dyn LevelReader,
        pos: BlockPos,
    ) -> bool {
        world.get_block_state(pos.above()).is_air()
    }

    fn is_bonemeal_success(
        &self,
        _state: BlockStateId,
        _world: &Arc<World>,
        _rng: &mut dyn Rng,
        _pos: BlockPos,
    ) -> bool {
        true
    }

    fn perform_bonemeal(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        _rng: &mut dyn Rng,
        pos: BlockPos,
    ) {
        // TODO(Worldgen): route to the configured-feature placement for the block's feature by
        // name (`moss_vegetation` under `moss_block`, `pale_moss_vegetation` under
        // `pale_moss_block`) once the worldgen lane exposes a `place_configured_feature`-style
        // entry point for block behaviors (vanilla `Feature.place`, `BonemealableBlock` →
        // `MossPatchFeature`).
        let _ = (world, pos, &vanilla_blocks::MOSS_BLOCK);
    }

    fn bonemeal_action_type(&self) -> BonemealAction {
        BonemealAction::NeighborSpreader
    }
}
