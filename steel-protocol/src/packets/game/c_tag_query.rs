//! Clientbound tag query packet - answers a block entity or entity NBT tag query.

use std::io::{Result, Write};

use steel_macros::ClientPacket;
use steel_registry::packets::play::C_TAG_QUERY;
use steel_utils::{
    codec::VarInt,
    serial::{OptionalNbt, WriteTo},
};

/// Response to `ServerboundBlockEntityTagQueryPacket` / `ServerboundEntityTagQueryPacket`.
///
/// Vanilla marks this packet skippable (`Packet::isSkippable`), and Steel has no wire-level
/// equivalent flag, so nothing is emitted for it.
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(Play = C_TAG_QUERY)]
pub struct CTagQuery {
    /// Transaction ID echoed from the client's query packet.
    pub transaction_id: i32,
    /// The queried NBT compound, or nothing when the query found no data. `OptionalNbt` writes the
    /// compound tag (type byte included) or a single END tag for the absent case, matching
    /// vanilla's `FriendlyByteBuf.writeNbt(nullable)`.
    pub tag: OptionalNbt,
}

impl CTagQuery {
    /// Creates a new tag query response packet.
    #[must_use]
    pub const fn new(transaction_id: i32, tag: OptionalNbt) -> Self {
        Self {
            transaction_id,
            tag,
        }
    }
}

impl WriteTo for CTagQuery {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        VarInt(self.transaction_id).write(writer)?;
        self.tag.write(writer)
    }
}
