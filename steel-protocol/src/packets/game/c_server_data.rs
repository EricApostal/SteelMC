//! Clientbound server data packet.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_SERVER_DATA;
use text_components::TextComponent;

/// Sends the server's MOTD and favicon to the client.
///
/// Wire order mirrors vanilla: the MOTD component (trusted, context-free NBT
/// component codec) followed by `ByteBufCodecs.BYTE_ARRAY.apply(optional)` - a
/// presence boolean and, when present, a VarInt-prefixed byte buffer.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_SERVER_DATA)]
pub struct CServerData {
    /// Message of the day shown in the client's server list.
    pub motd: TextComponent,
    /// Raw PNG favicon bytes, absent when the server has no icon.
    pub icon_bytes: Option<Vec<u8>>,
}

impl CServerData {
    /// Creates a new server data packet.
    #[must_use]
    pub const fn new(motd: TextComponent, icon_bytes: Option<Vec<u8>>) -> Self {
        Self { motd, icon_bytes }
    }
}
