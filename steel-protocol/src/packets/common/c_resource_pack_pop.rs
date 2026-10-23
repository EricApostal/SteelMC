//! Clientbound resource pack pop packet.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::config::C_RESOURCE_PACK_POP;
use steel_registry::packets::play::C_RESOURCE_PACK_POP as PLAY_C_RESOURCE_PACK_POP;
use uuid::Uuid;

/// Removes a resource pack from the client's stack.
///
/// Mirrors vanilla's `ClientboundResourcePackPopPacket`: an optional UUID
/// (`SafeBuf.writeOptional` -> one presence byte, then the 16 UUID bytes).
/// `None` pops every pack the server pushed.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Config = C_RESOURCE_PACK_POP, Play = PLAY_C_RESOURCE_PACK_POP)]
pub struct CResourcePackPop {
    /// Pack to remove, or `None` to remove all server-pushed packs.
    pub id: Option<Uuid>,
}

impl CResourcePackPop {
    /// Creates a new resource pack pop packet for one specific pack.
    #[must_use]
    pub const fn new(id: Uuid) -> Self {
        Self { id: Some(id) }
    }

    /// Creates a packet that pops every server-pushed pack.
    #[must_use]
    pub const fn all() -> Self {
        Self { id: None }
    }
}
