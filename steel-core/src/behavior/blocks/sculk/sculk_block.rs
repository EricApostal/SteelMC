//! Vanilla `SculkBlock`.
//!
//! Vanilla reference: `net.minecraft.world.level.block.SculkBlock`. `SculkBlock` only adds two
//! things on top of `DropExperienceBlock`: the `SculkBehaviour` charge-cursor spread rule and a
//! `ConstantInt.of(1)` experience drop. The spread rule needs a *runtime* `SculkSpreader`, which
//! Steel currently only has in its worldgen form
//! (`crate::worldgen::feature::features::sculk_patch`, private and `WorldGenRegion`-based), so the
//! spread half stays a documented dependency while the experience drop is implemented.

use std::sync::Arc;

use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::item_stack::ItemStack;
use steel_utils::value_providers::IntProvider;
use steel_utils::{BlockPos, BlockStateId};

use crate::behavior::{BlockBehavior, BlockPlaceContext, try_drop_experience};
use crate::world::World;

/// Vanilla `SculkBlock.GROWTH_INHIBITOR_RANGE`.
pub const SCULK_GROWTH_INHIBITOR_RANGE: i32 = 4;

/// Vanilla `SculkBlock`.
#[block_behavior]
pub struct SculkBlock {
    block: BlockRef,
}

impl SculkBlock {
    /// Creates sculk behavior for `block`.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }
}

impl BlockBehavior for SculkBlock {
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(self.block.default_state())
    }

    // TODO(Worldgen): Vanilla `SculkBlock.attemptUseCharge` (charge decay, `growth_spawn_cost`,
    // `no_growth_radius`, `additional_decay_rate`, `canPlaceGrowth` with `SCULK_GROWTH_INHIBITORS`)
    // operates on a live `SculkSpreader` created by `SculkSpreader.createLevelSpreader()`. Steel's
    // only spreader lives in `worldgen/feature/features/sculk_patch.rs` and is private, takes a
    // `WorldGenRegion`, and exposes no `ChargeCursor`/`add_cursors` API for a level-bound spreader.
    // This behavior therefore cannot spread sculk patches yet; `SculkCatalystBlockEntity` records
    // its accumulated charge so the wiring can land once that API exists.

    fn spawn_after_break(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        tool: &ItemStack,
        drop_experience: bool,
    ) {
        if drop_experience {
            // Vanilla `SculkBlock` constructor: `ConstantInt.of(1)`.
            try_drop_experience(world, pos, tool, &IntProvider::Constant(1));
        }
    }
}
