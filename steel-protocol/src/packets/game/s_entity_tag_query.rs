//! Serverbound packet for querying an entity's NBT data.

use steel_macros::{ReadFrom, ServerPacket};
#[expect(unused_imports)]
use steel_registry::packets::play::S_ENTITY_TAG_QUERY;

/// Sent by the client to request the NBT of an entity.
///
/// Matches vanilla's `ServerboundEntityTagQueryPacket`: a `VarInt`
/// transaction id followed by the `VarInt` entity id.
#[derive(ServerPacket, ReadFrom, Clone, Debug)]
#[packet_id(Play = S_ENTITY_TAG_QUERY)]
pub struct SEntityTagQuery {
    /// Transaction id echoed back in the response.
    #[read(as = VarInt)]
    pub transaction_id: i32,
    /// Id of the entity to query.
    #[read(as = VarInt)]
    pub entity_id: i32,
}
