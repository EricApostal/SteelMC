//! Clientbound debug entity value packet - a debug-renderer subscription value for an entity.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_DEBUG_ENTITY_VALUE;
use steel_utils::serial::WriteTo;

use crate::packets::game::c_debug_block_value::DebugSubscriptionUpdate;

/// Sent to update the client's debug-renderer data for a single entity.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_DEBUG_ENTITY_VALUE)]
pub struct CDebugEntityValue<V: WriteTo> {
    /// ID of the entity the value belongs to.
    #[write(as = VarInt)]
    pub entity_id: i32,
    /// The subscription value that changed for this entity.
    pub update: DebugSubscriptionUpdate<V>,
}

impl<V: WriteTo> CDebugEntityValue<V> {
    /// Creates a new debug entity value packet.
    #[must_use]
    pub const fn new(entity_id: i32, update: DebugSubscriptionUpdate<V>) -> Self {
        Self { entity_id, update }
    }
}
