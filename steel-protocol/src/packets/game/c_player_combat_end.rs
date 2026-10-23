//! Clientbound player combat end packet - ends the client-side combat HUD state.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_PLAYER_COMBAT_END;

/// Tells the client the player's current combat encounter has ended.
///
/// Vanilla encodes this with a hand-written codec (`Packet.codec`), but the wire
/// shape is just the combat duration as a `VarInt`.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_PLAYER_COMBAT_END)]
pub struct CPlayerCombatEnd {
    /// Combat duration in ticks, as reported by `CombatTracker.getCombatDuration()`.
    #[write(as = VarInt)]
    pub duration: i32,
}
