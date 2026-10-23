//! Clientbound reset chat packet.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::config::C_RESET_CHAT;

/// Clears the client's chat state while it is still downloading a world.
///
/// Mirrors vanilla's `ClientboundResetChatPacket`, whose codec is
/// `StreamCodec.unit(INSTANCE)` — an empty payload.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Config = C_RESET_CHAT)]
pub struct CResetChat {}
