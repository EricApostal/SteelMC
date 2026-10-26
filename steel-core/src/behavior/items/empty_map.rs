//! Empty map item behavior (`EmptyMapItem`).
//!
//! Vanilla `EmptyMapItem.use` (`EmptyMapItem.java:24-45`) is one composite
//! effect: consume one empty map, award `Stats.ITEM_USED`, play
//! `SoundEvents.UI_CARTOGRAPHY_TABLE_TAKE_RESULT` at the player, then mint a
//! filled map for the player's block x/z (`MapItem.create(level, x, z, 0, true,
//! false)`). An emptied hand receives the map directly; otherwise it goes to the
//! inventory and overflows to the ground.

use steel_macros::item_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::sound_events;
use steel_registry::stat::vanilla_stat_types;

use super::map::MapItem;
use crate::behavior::context::{InteractionResult, UseItemContext};
use crate::behavior::item::ItemBehavior;
use crate::behavior::item_utils::create_filled_result;
use crate::entity::Entity as _;

/// Behavior for the empty map item.
#[item_behavior]
pub struct EmptyMapItem;

impl ItemBehavior for EmptyMapItem {
    fn use_item(&self, context: &mut UseItemContext<'_>) -> InteractionResult {
        let position = context.player.block_position();
        let map = MapItem::create(context.world, position.x(), position.z(), 0, true, false);
        context.inv.with_item(|item| {
            context
                .player
                .award_stat(&vanilla_stat_types::ITEM_USED, item.item());
        });
        context.world.play_sound_at(
            &sound_events::UI_CARTOGRAPHY_TABLE_TAKE_RESULT,
            SoundSource::Players,
            context.player.position(),
            1.0,
            1.0,
            None,
        );
        create_filled_result(context, map, false);
        InteractionResult::Success
    }
}
