//! Serverbound packet for teleporting to an entity (spectator mode).

use steel_macros::{ReadFrom, ServerPacket};
#[expect(unused_imports)]
use steel_registry::packets::play::S_TELEPORT_TO_ENTITY;
use uuid::Uuid;

/// Sent by a spectating client to teleport to the given entity.
///
/// Matches vanilla's `ServerboundTeleportToEntityPacket`: a single UUID
/// (two big-endian `i64`s, most-significant bits first).
#[derive(ServerPacket, ReadFrom, Clone, Debug)]
#[packet_id(Play = S_TELEPORT_TO_ENTITY)]
pub struct STeleportToEntity {
    /// UUID of the entity to teleport to.
    pub uuid: Uuid,
}
