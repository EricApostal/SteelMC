//! Clientbound cookie request packet.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::config::C_COOKIE_REQUEST;
use steel_registry::packets::login::C_COOKIE_REQUEST as LOGIN_C_COOKIE_REQUEST;
use steel_registry::packets::play::C_COOKIE_REQUEST as PLAY_C_COOKIE_REQUEST;
use steel_utils::Identifier;

/// Asks the client to send back the value it stored under `key`.
///
/// Mirrors vanilla's `ClientboundCookieRequestPacket`: a single identifier,
/// written as a VarInt-prefixed UTF-8 string (`FriendlyByteBuf.writeIdentifier`).
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(
    Login = LOGIN_C_COOKIE_REQUEST,
    Config = C_COOKIE_REQUEST,
    Play = PLAY_C_COOKIE_REQUEST
)]
pub struct CCookieRequest {
    /// Cookie key whose stored value the server wants the client to return.
    pub key: Identifier,
}

impl CCookieRequest {
    /// Creates a new cookie request packet.
    #[must_use]
    pub const fn new(key: Identifier) -> Self {
        Self { key }
    }
}
