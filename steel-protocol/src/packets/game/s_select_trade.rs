//! Serverbound packet for selecting a villager trade offer.

use steel_macros::{ReadFrom, ServerPacket};
#[expect(unused_imports)]
use steel_registry::packets::play::S_SELECT_TRADE;

/// Sent when the player clicks an offer in the trading screen.
///
/// Matches vanilla's `ServerboundSelectTradePacket`: a single `VarInt` index
/// into the merchant's offer list.
#[derive(ServerPacket, ReadFrom, Clone, Debug)]
#[packet_id(Play = S_SELECT_TRADE)]
pub struct SSelectTrade {
    /// Index of the selected offer.
    #[read(as = VarInt)]
    pub item: i32,
}
