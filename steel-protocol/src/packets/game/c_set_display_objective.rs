//! Clientbound set display objective packet.

use std::io::{Result, Write};

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_SET_DISPLAY_OBJECTIVE;
use steel_utils::{
    codec::VarInt,
    serial::{PrefixedWrite, WriteTo},
};

/// Scoreboard display slot (`DisplaySlot::BY_ID` order).
#[derive(WriteTo, Clone, Copy, Debug, PartialEq, Eq)]
#[write(as = VarInt)]
pub enum ScoreboardDisplaySlot {
    /// Player list.
    List = 0,
    /// Sidebar.
    Sidebar = 1,
    /// Below-name health bar.
    BelowName = 2,
    /// `sidebar.team.black`.
    TeamBlack = 3,
    /// `sidebar.team.dark_blue`.
    TeamDarkBlue = 4,
    /// `sidebar.team.dark_green`.
    TeamDarkGreen = 5,
    /// `sidebar.team.dark_aqua`.
    TeamDarkAqua = 6,
    /// `sidebar.team.dark_red`.
    TeamDarkRed = 7,
    /// `sidebar.team.dark_purple`.
    TeamDarkPurple = 8,
    /// `sidebar.team.gold`.
    TeamGold = 9,
    /// `sidebar.team.gray`.
    TeamGray = 10,
    /// `sidebar.team.dark_gray`.
    TeamDarkGray = 11,
    /// `sidebar.team.blue`.
    TeamBlue = 12,
    /// `sidebar.team.green`.
    TeamGreen = 13,
    /// `sidebar.team.aqua`.
    TeamAqua = 14,
    /// `sidebar.team.red`.
    TeamRed = 15,
    /// `sidebar.team.light_purple`.
    TeamLightPurple = 16,
    /// `sidebar.team.yellow`.
    TeamYellow = 17,
    /// `sidebar.team.white`.
    TeamWhite = 18,
}

/// Shows an objective in a display slot, or clears it (`None`).
///
/// Matches vanilla's `ClientboundSetDisplayObjectivePacket`. Clearing a slot is
/// encoded as an empty objective name.
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(Play = C_SET_DISPLAY_OBJECTIVE)]
pub struct CSetDisplayObjective {
    /// Slot the objective is shown in.
    pub slot: ScoreboardDisplaySlot,
    /// Name of the objective, or `None` to clear the slot.
    pub objective_name: Option<String>,
}

impl CSetDisplayObjective {
    /// Shows an objective in a slot.
    #[must_use]
    pub fn show(slot: ScoreboardDisplaySlot, objective_name: impl Into<String>) -> Self {
        Self {
            slot,
            objective_name: Some(objective_name.into()),
        }
    }

    /// Clears a slot.
    #[must_use]
    pub const fn clear(slot: ScoreboardDisplaySlot) -> Self {
        Self {
            slot,
            objective_name: None,
        }
    }
}

impl WriteTo for CSetDisplayObjective {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        self.slot.write(writer)?;
        match &self.objective_name {
            Some(objective_name) => objective_name.write_prefixed::<VarInt>(writer)?,
            None => "".write_prefixed::<VarInt>(writer)?,
        }
        Ok(())
    }
}
