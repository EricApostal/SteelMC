//! Serverbound pong packet - the client's reply to a clientbound ping.

use steel_macros::{ReadFrom, ServerPacket};
#[expect(unused_imports)]
use steel_registry::packets::config::S_PONG;
#[expect(unused_imports)]
use steel_registry::packets::play::S_PONG as PLAY_S_PONG;

/// Sent by the client in response to a `ClientboundPingPacket`.
///
/// Equivalent to vanilla's `ServerboundPongPacket`: a single big-endian `i32`
/// echoed back from the ping (`FriendlyByteBuf::readInt`).
#[derive(ReadFrom, ServerPacket, Clone, Debug)]
#[packet_id(Config = S_PONG, Play = PLAY_S_PONG)]
pub struct SPong {
    /// The id received in the clientbound ping.
    pub id: i32,
}
