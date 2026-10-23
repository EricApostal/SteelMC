//! Written book item behavior.

use steel_macros::item_behavior;
use steel_protocol::packets::game::COpenBook;
use steel_registry::data_components::vanilla_components::WRITTEN_BOOK_CONTENT;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::vanilla_items;

use crate::behavior::{InteractionResult, ItemBehavior, UseItemContext};

/// Vanilla written book behavior.
///
/// Vanilla's `WrittenBookItem.use` calls `Player.openItemGui` and then awards
/// `Stats.ITEM_USED`. `ServerPlayer.openItemGui` sends the open-book packet only
/// when the stack carries `WRITTEN_BOOK_CONTENT`, so the same guard is applied
/// here before the packet is emitted.
#[item_behavior]
pub struct WrittenBookItem;

impl ItemBehavior for WrittenBookItem {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        let has_written_content = context
            .inv
            .with_item(|stack| stack.get(WRITTEN_BOOK_CONTENT).is_some());
        if has_written_content {
            context.player.send_packet(COpenBook::new(context.hand));
        }

        context
            .player
            .award_stat(&vanilla_stat_types::ITEM_USED, &vanilla_items::WRITTEN_BOOK);
        InteractionResult::Success
    }
}
