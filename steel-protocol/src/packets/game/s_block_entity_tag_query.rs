//! Serverbound packet for querying a block entity's NBT data.

use steel_macros::{ReadFrom, ServerPacket};
#[expect(unused_imports)]
use steel_registry::packets::play::S_BLOCK_ENTITY_TAG_QUERY;
use steel_utils::BlockPos;

/// Sent by the client to request the NBT of the block entity at a position.
///
/// Matches vanilla's `ServerboundBlockEntityTagQueryPacket`: a `VarInt`
/// transaction id followed by the packed block position (`i64`).
#[derive(ServerPacket, ReadFrom, Clone, Debug)]
#[packet_id(Play = S_BLOCK_ENTITY_TAG_QUERY)]
pub struct SBlockEntityTagQuery {
    /// Transaction id echoed back in the response.
    #[read(as = VarInt)]
    pub transaction_id: i32,
    /// Position of the block entity to query.
    pub pos: BlockPos,
}
