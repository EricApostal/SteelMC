//! Clientbound player combat enter packet - puts the client into combat HUD state.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_PLAYER_COMBAT_ENTER;

/// Tells the client the player has entered combat.
///
/// Vanilla's codec is `StreamCodec.unit`, so the packet carries no fields.
#[derive(ClientPacket, WriteTo, Clone, Debug, Default)]
#[packet_id(Play = C_PLAYER_COMBAT_ENTER)]
pub struct CPlayerCombatEnter;

impl CPlayerCombatEnter {
    /// Creates a new player combat enter packet.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}
