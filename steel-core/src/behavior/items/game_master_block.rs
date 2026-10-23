//! Game master block item behavior implementation.
//!
//! Vanilla's `GameMasterBlockItem` overrides `BlockItem.getPlacementState` so
//! that non-game-master players get no placement state at all, which makes the
//! placement fail before anything is written to the world.
//!
//! **Vanilla differences:** vanilla consults `Player.canUseGameMasterBlocks()`
//! (operator permission level >= 2). Steel has no operator-level model yet, so
//! this uses [`Player::is_operator`], which is the nearest available predicate.

use crate::behavior::ItemBehavior;
use crate::behavior::context::{InteractionResult, UseOnContext};
use crate::behavior::items::BlockItem;
use crate::behavior::items::block_item::SurvivalCheck;
use crate::player::Player;
use steel_macros::item_behavior;
use steel_registry::blocks::BlockRef;

/// Behavior for the operator-only "game master" blocks (command blocks,
/// structure block, jigsaw, test block).
///
/// Placement mirrors vanilla `GameMasterBlockItem`: the item behaves exactly
/// like `BlockItem`, except that world placement is rejected for players
/// without game-master permissions.
#[item_behavior]
pub struct GameMasterBlockItem {
    /// The block this item places.
    #[json_arg(vanilla_blocks, json = "block")]
    pub block: BlockRef,

    /// Base placement behavior delegated to unchanged.
    pub base: BlockItem,
}

impl GameMasterBlockItem {
    /// Creates a new game master block item behavior for the given block.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self {
            block,
            base: BlockItem::new(block),
        }
    }

    /// Mirrors vanilla's `Player.canUseGameMasterBlocks()` gate inside
    /// `GameMasterBlockItem.getPlacementState`.
    #[must_use]
    fn can_place(player: &Player) -> bool {
        player.is_operator()
    }
}

impl ItemBehavior for GameMasterBlockItem {
    fn use_on(&self, context: &mut UseOnContext) -> InteractionResult {
        // Vanilla's override returns a null placement state, which makes
        // `BlockItem.place` fail without touching the world.
        let allowed = Self::can_place(context.player);
        self.base.place_with_policy(
            context.build_place_context(),
            Some,
            SurvivalCheck::Required,
            BlockItem::place_block,
            self.block.config.sound_type.place_sound,
            |place_context| {
                if !allowed {
                    return None;
                }
                self.base.get_state_for_placement(place_context)
            },
        )
    }

    /// Vanilla `BlockItem`'s container check, inherited unchanged.
    fn can_fit_inside_container_items(&self) -> bool {
        self.base.can_fit_inside_container_items()
    }
}
