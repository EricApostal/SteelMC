//! Item frame item behavior (`ItemFrameItem`).
//!
//! Vanilla reference: `ItemFrameItem`
//! (`/tmp/vanilla/src/world/item/ItemFrameItem.java:10-20`). The class only
//! overrides `mayPlace`, so placement itself is
//! [`HangingEntityItem::use_on_with`] with this override:
//!
//! ```java
//! protected boolean mayPlace(Player player, Direction direction, ItemStack itemStack, BlockPos blockPos) {
//!    return player.level().isInsideBuildHeight(blockPos) && player.mayUseItemAt(blockPos, direction, itemStack);
//! }
//! ```
//!
//! Note that vanilla deliberately drops the horizontal-face requirement of
//! `HangingEntityItem.mayPlace` for item frames; the build-height check is what
//! replaces it. This file mirrors that exactly.

use steel_macros::item_behavior;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_utils::{BlockPos, Direction};

use crate::behavior::ItemBehavior;
use crate::behavior::context::{InteractionResult, UseOnContext};
use crate::behavior::items::HangingEntityItem;
use crate::player::Player;

/// Behavior for `item_frame` and `glow_item_frame`.
#[item_behavior]
pub struct ItemFrameItem {
    /// The item frame entity type this item places.
    #[json_arg(vanilla_entities, json = "type")]
    pub entity_type: EntityTypeRef,
    /// Shared `HangingEntityItem` placement implementation.
    base: HangingEntityItem,
}

impl ItemFrameItem {
    /// Creates an item frame item behavior for `entity_type`.
    #[must_use]
    pub const fn new(entity_type: EntityTypeRef) -> Self {
        Self {
            entity_type,
            base: HangingEntityItem::new(entity_type),
        }
    }

    /// Vanilla `ItemFrameItem.mayPlace`.
    ///
    /// Only requires the target block to be inside the build height and the
    /// adventure-mode placement permission.
    fn may_place(
        player: &Player,
        direction: Direction,
        stack: &ItemStack,
        block_pos: BlockPos,
    ) -> bool {
        let world = player.get_world();
        !world.is_outside_build_height(block_pos.y())
            && player.may_use_item_at(block_pos, direction, stack)
    }
}

impl ItemBehavior for ItemFrameItem {
    fn use_on(&self, context: &mut UseOnContext) -> InteractionResult {
        self.base.use_on_with(context, Self::may_place)
    }
}
