//! Enchanting table block behavior implementation.
//!
//! Right-clicking opens the enchanting menu and its powered-bookshelf scan is
//! shared with [`EnchantmentKind`](crate::inventory::menu::kinds::EnchantmentKind).

use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::{
    REGISTRY, TaggedRegistryExt as _, vanilla_block_entity_types, vanilla_block_tags::BlockTag,
};
use steel_utils::{BlockPos, BlockStateId, Downcast as _};

use crate::behavior::InventoryAccess;
use crate::behavior::block::{BlockBehavior, BlockEntityCreation};
use crate::behavior::context::{BlockHitResult, BlockPlaceContext, InteractionResult};
use crate::block_entity::BLOCK_ENTITIES;
use crate::block_entity::entities::EnchantingTableBlockEntity;
use crate::entity::ai::path::PathComputationType;
use crate::inventory::menu::kinds::enchantment;
use crate::player::Player;
use crate::world::{LevelReader, World};

/// Number of book-shelf offsets Vanilla scans around an enchanting table.
///
/// Vanilla builds the same list with
/// `BlockPos.betweenClosedStream(-2, 0, -2, 2, 1, 2)` filtered to the positions
/// where `|x| == 2 || |z| == 2`: 2 layers × 16 horizontal positions.
const BOOKSHELF_OFFSET_COUNT: usize = 32;

/// The offsets Vanilla scans for enchantment power.
///
/// Vanilla parity: `EnchantingTableBlock.BOOKSHELF_OFFSETS`.
pub const BOOKSHELF_OFFSETS: [BlockPos; BOOKSHELF_OFFSET_COUNT] = bookshelf_offsets();

/// Behavior for the enchanting table block.
///
/// The table itself holds no state: it opens the enchanting menu and counts the
/// valid bookshelves around it, exactly like Vanilla.
#[block_behavior]
pub struct EnchantingTableBlock {
    block: BlockRef,
}

impl EnchantingTableBlock {
    /// Creates a new enchanting table block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Counts the offsets around `pos` that provide enchantment power.
    ///
    /// Mirrors the loop Vanilla's `EnchantmentMenu.slotsChanged` runs over
    /// [`BOOKSHELF_OFFSETS`]; the count is capped by the offer formulas, not
    /// here.
    #[must_use]
    pub fn bookshelf_count(world: &dyn LevelReader, pos: BlockPos) -> i32 {
        let mut bookcases = 0;
        for offset in BOOKSHELF_OFFSETS {
            if Self::is_valid_bookshelf(world, pos, offset) {
                bookcases += 1;
            }
        }
        bookcases
    }

    /// Returns whether `offset` from the table position contributes enchantment
    /// power.
    ///
    /// Vanilla parity: `EnchantingTableBlock.isValidBookShelf`. The bookshelf
    /// must be an `#enchantment_power_provider` and the block between it and the
    /// table must be an `#enchantment_power_transmitter`.
    #[must_use]
    pub fn is_valid_bookshelf(world: &dyn LevelReader, pos: BlockPos, offset: BlockPos) -> bool {
        let provider = world
            .get_block_state(pos.offset(offset.x(), offset.y(), offset.z()))
            .get_block();
        if !REGISTRY
            .blocks
            .is_in_tag(provider, &BlockTag::ENCHANTMENT_POWER_PROVIDER)
        {
            return false;
        }
        let transmitter = world
            .get_block_state(pos.offset(offset.x() / 2, offset.y(), offset.z() / 2))
            .get_block();
        REGISTRY
            .blocks
            .is_in_tag(transmitter, &BlockTag::ENCHANTMENT_POWER_TRANSMITTER)
    }
}

impl BlockBehavior for EnchantingTableBlock {
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(self.block.default_state())
    }

    fn use_without_item(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        let Some(block_entity) = world.get_block_entity(pos) else {
            return InteractionResult::Pass;
        };
        let Some(table) = block_entity.downcast_ref::<EnchantingTableBlockEntity>() else {
            return InteractionResult::Pass;
        };
        let title = table.display_name();
        let inventory = Arc::clone(&player.inventory);
        player.open_menu(title, move |context| {
            enchantment(inventory, context.container_id, pos, context.world)
        });

        // Vanilla awards no stat when the enchanting table is opened; the
        // `enchant_item` stat is awarded when an enchantment is applied.
        InteractionResult::Success
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
            &vanilla_block_entity_types::ENCHANTING_TABLE,
            level,
            pos,
            state,
        ))
    }

    fn is_pathfindable(
        &self,
        _state: BlockStateId,
        _computation_type: PathComputationType,
    ) -> bool {
        // Vanilla `EnchantingTableBlock.isPathfindable` always returns false.
        false
    }
}

/// Builds Vanilla's book-shelf offset list at compile time.
const fn bookshelf_offsets() -> [BlockPos; BOOKSHELF_OFFSET_COUNT] {
    let mut offsets = [BlockPos::ZERO; BOOKSHELF_OFFSET_COUNT];
    let mut count = 0;
    let mut x = -2;
    while x <= 2 {
        let mut y = 0;
        while y <= 1 {
            let mut z = -2;
            while z <= 2 {
                if x == 2 || x == -2 || z == 2 || z == -2 {
                    offsets[count] = BlockPos::new(x, y, z);
                    count += 1;
                }
                z += 1;
            }
            y += 1;
        }
        x += 1;
    }
    offsets
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn bookshelf_offsets_match_vanillas_filtered_ring() {
        // Vanilla: `betweenClosedStream(-2, 0, -2, 2, 1, 2)` filtered to
        // `|x| == 2 || |z| == 2` yields both layers of the 5x5 perimeter.
        assert_eq!(BOOKSHELF_OFFSETS.len(), 32);

        let unique: HashSet<_> = BOOKSHELF_OFFSETS
            .iter()
            .map(|pos| (pos.x(), pos.y(), pos.z()))
            .collect();
        assert_eq!(unique.len(), BOOKSHELF_OFFSETS.len());

        for pos in BOOKSHELF_OFFSETS {
            let on_ring = pos.x().abs() == 2 || pos.z().abs() == 2;
            assert!(on_ring, "{pos:?} is not on the bookshelf ring");
            assert!(
                (0..=1).contains(&pos.y()),
                "{pos:?} is outside the ring layers"
            );
            assert!(pos.x().abs() <= 2 && pos.z().abs() <= 2);
        }
    }
}
