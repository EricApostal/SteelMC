//! Serverbound packet for selecting the displayed item in a bundle.

use steel_macros::{ReadFrom, ServerPacket};
#[expect(unused_imports)]
use steel_registry::packets::play::S_BUNDLE_ITEM_SELECTED;

/// Sent when the player scrolls to change the selected item shown on a bundle.
///
/// Matches vanilla's `ServerboundSelectBundleItemPacket`: the container
/// slot id and the selected item index, both `VarInt`s. Vanilla rejects a
/// `selected_item_index` that is negative and not `-1` (deselect); the
/// handler must enforce that bound since the wire read itself accepts any
/// `VarInt`.
#[derive(ServerPacket, ReadFrom, Clone, Debug)]
#[packet_id(Play = S_BUNDLE_ITEM_SELECTED)]
pub struct SBundleItemSelected {
    /// Container slot id holding the bundle.
    #[read(as = VarInt)]
    pub slot_id: i32,
    /// Index of the selected item, or `-1` to clear the selection.
    #[read(as = VarInt)]
    pub selected_item_index: i32,
}
