//! Clientbound set score packet.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_SET_SCORE;
use text_components::TextComponent;

use super::c_set_objective::NumberFormat;

/// Sets a player's score in an objective.
///
/// Matches vanilla's `ClientboundSetScorePacket`. Wire order: owner, objective
/// name, score, optional display component, optional [`NumberFormat`].
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_SET_SCORE)]
pub struct CSetScore {
    /// Score holder name (player name, entity UUID string or `*`).
    #[write(as = Prefixed(VarInt))]
    pub owner: String,
    /// Name of the objective the score belongs to.
    #[write(as = Prefixed(VarInt))]
    pub objective_name: String,
    /// The score value.
    #[write(as = VarInt)]
    pub score: i32,
    /// Component used as the score's display name, if overridden.
    pub display: Option<TextComponent>,
    /// Number format used to render the score, if overridden.
    pub number_format: Option<NumberFormat>,
}

impl CSetScore {
    /// Creates a set score packet.
    #[must_use]
    pub fn new(
        owner: impl Into<String>,
        objective_name: impl Into<String>,
        score: i32,
        display: Option<TextComponent>,
        number_format: Option<NumberFormat>,
    ) -> Self {
        Self {
            owner: owner.into(),
            objective_name: objective_name.into(),
            score,
            display,
            number_format,
        }
    }
}
