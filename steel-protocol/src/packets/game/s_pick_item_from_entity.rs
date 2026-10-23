//! Serverbound packet for picking an item from an entity.

use steel_macros::{ReadFrom, ServerPacket};
#[expect(unused_imports)]
use steel_registry::packets::play::S_PICK_ITEM_FROM_ENTITY;

/// Sent when the player uses the pick-block key on an entity.
///
/// Matches vanilla's `ServerboundPickItemFromEntityPacket`: the target
/// entity id as a `VarInt`, then whether NBT data should be included.
#[derive(ServerPacket, ReadFrom, Clone, Debug)]
#[packet_id(Play = S_PICK_ITEM_FROM_ENTITY)]
pub struct SPickItemFromEntity {
    /// Id of the entity to pick the item from.
    #[read(as = VarInt)]
    pub id: i32,
    /// Whether block-entity-style NBT data should be included.
    pub include_data: bool,
}
