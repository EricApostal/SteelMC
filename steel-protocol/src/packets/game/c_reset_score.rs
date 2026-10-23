//! Clientbound reset score packet.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_RESET_SCORE;

/// Resets a player's score, optionally only for one objective.
///
/// Matches vanilla's `ClientboundResetScorePacket`. Wire order: owner, then a
/// `writeNullable`-style optional objective name.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_RESET_SCORE)]
pub struct CResetScore {
    /// Score holder name (player name, entity UUID string or `*`).
    #[write(as = Prefixed(VarInt))]
    pub owner: String,
    /// Name of the objective to reset, or `None` to reset every objective.
    #[write(as = Prefixed(VarInt))]
    pub objective_name: Option<String>,
}

impl CResetScore {
    /// Resets all of a score holder's scores.
    #[must_use]
    pub fn all(owner: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            objective_name: None,
        }
    }

    /// Resets a score holder's score in one objective.
    #[must_use]
    pub fn one(owner: impl Into<String>, objective_name: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            objective_name: Some(objective_name.into()),
        }
    }
}
