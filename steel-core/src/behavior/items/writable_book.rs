//! Writable book item behavior.

use steel_macros::item_behavior;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::vanilla_items;

use crate::behavior::{InteractionResult, ItemBehavior, UseItemContext};

/// Vanilla writable book behavior.
///
/// Vanilla's `WritableBookItem.use` calls `Player.openItemGui` and then awards
/// `Stats.ITEM_USED`. The only `openItemGui` override is
/// `ServerPlayer.openItemGui`, which reacts solely to `WRITTEN_BOOK_CONTENT`
/// and therefore does nothing for a writable book; the edit screen is opened
/// client-side by `LocalPlayer.openItemGui`. The server side of this behavior
/// is consequently the usage stat plus a successful interaction.
#[item_behavior]
pub struct WritableBookItem;

impl ItemBehavior for WritableBookItem {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        context.player.award_stat(
            &vanilla_stat_types::ITEM_USED,
            &vanilla_items::WRITABLE_BOOK,
        );
        InteractionResult::Success
    }
}
