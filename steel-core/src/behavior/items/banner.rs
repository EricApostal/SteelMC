//! Banner item behavior implementation.
//!
//! Vanilla's `BannerItem` extends `StandingAndWallBlockItem`, hardcoding
//! [`Direction::Down`] as the attachment direction and adding `getColor()`.
//!
//! **Vanilla differences:** `getColor()` has no Steel equivalent because the
//! registry does not model vanilla's `AbstractBannerBlock` (or any other
//! banner-color accessor), so the dye color of a banner block cannot be read
//! from data. Placement behavior is identical to vanilla.

use crate::behavior::ItemBehavior;
use crate::behavior::context::{InteractionResult, UseOnContext};
use crate::behavior::items::StandingAndWallBlockItem;
use steel_macros::item_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::properties::Direction;

/// Behavior for banner items, which place a standing or wall banner.
///
/// Vanilla constructs the item with `Direction.DOWN`; the direction is still
/// read from the item data (`attachment_direction`) so the generator passes the
/// same value vanilla hardcodes.
///
/// Vanilla also validates that both blocks are `AbstractBannerBlock` instances.
/// Steel's registry has no such block type, so that check has no counterpart.
#[item_behavior]
pub struct BannerItem {
    /// The block placed when looking toward `attachment_direction` (e.g. `white_banner`).
    #[json_arg(vanilla_blocks, json = "block")]
    pub standing_block: BlockRef,
    /// The block placed otherwise (e.g. `white_wall_banner`).
    #[json_arg(vanilla_blocks, json = "wall_block")]
    pub wall_block: BlockRef,
    /// The direction that triggers the standing banner placement (`Down` for banners).
    #[json_arg(
        r#enum = "Direction",
        module = "steel_registry::blocks::properties",
        json = "attachment_direction"
    )]
    pub attachment_direction: Direction,

    /// Placement half delegated to the vanilla superclass behavior.
    pub base: StandingAndWallBlockItem,
}

impl BannerItem {
    /// Creates a new banner item behavior.
    ///
    /// # Arguments
    /// * `standing_block` - Block placed when looking toward `attachment_direction`
    /// * `wall_block` - Block placed when looking away from `attachment_direction`
    /// * `attachment_direction` - Direction that triggers the standing banner
    #[must_use]
    pub const fn new(
        standing_block: BlockRef,
        wall_block: BlockRef,
        attachment_direction: Direction,
    ) -> Self {
        Self {
            standing_block,
            wall_block,
            attachment_direction,
            base: StandingAndWallBlockItem::new(standing_block, wall_block, attachment_direction),
        }
    }
}

impl ItemBehavior for BannerItem {
    fn use_on(&self, context: &mut UseOnContext) -> InteractionResult {
        self.base.use_on(context)
    }
}
