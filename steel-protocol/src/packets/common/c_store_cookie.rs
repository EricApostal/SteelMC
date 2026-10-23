//! Clientbound store cookie packet.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::config::C_STORE_COOKIE;
use steel_registry::packets::play::C_STORE_COOKIE as PLAY_C_STORE_COOKIE;
use steel_utils::Identifier;

/// Asks the client to store `payload` under `key`.
///
/// Mirrors vanilla's `ClientboundStoreCookiePacket`: an identifier followed by
/// `ByteBufCodecs.byteArray(5120)`, i.e. a VarInt byte count and the raw bytes.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Config = C_STORE_COOKIE, Play = PLAY_C_STORE_COOKIE)]
pub struct CStoreCookie {
    /// Cookie key the payload is stored under.
    pub key: Identifier,
    /// Opaque payload, at most 5120 bytes (vanilla `MAX_PAYLOAD_SIZE`).
    #[write(as = Prefixed(VarInt), bound = 5120)]
    pub payload: Box<[u8]>,
}

impl CStoreCookie {
    /// Creates a new store cookie packet.
    #[must_use]
    pub const fn new(key: Identifier, payload: Box<[u8]>) -> Self {
        Self { key, payload }
    }
}
