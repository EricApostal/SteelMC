//! Clientbound set player team packet.

use std::io::{Error, Result, Write};

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_SET_PLAYER_TEAM;
use steel_utils::{
    codec::VarInt,
    serial::{PrefixedWrite, WriteTo},
};
use text_components::TextComponent;

/// Scoreboard team colour (`TeamColor`, id order).
///
/// Vanilla's `TeamColor` also owns the matching `TextColor` and `DisplaySlot`; the
/// wire codec only sends this id.
#[derive(WriteTo, Clone, Copy, Debug, PartialEq, Eq)]
#[write(as = VarInt)]
pub enum TeamColor {
    /// Black.
    Black = 0,
    /// Dark blue.
    DarkBlue = 1,
    /// Dark green.
    DarkGreen = 2,
    /// Dark aqua.
    DarkAqua = 3,
    /// Dark red.
    DarkRed = 4,
    /// Dark purple.
    DarkPurple = 5,
    /// Gold.
    Gold = 6,
    /// Gray.
    Gray = 7,
    /// Dark gray.
    DarkGray = 8,
    /// Blue.
    Blue = 9,
    /// Green.
    Green = 10,
    /// Aqua.
    Aqua = 11,
    /// Red.
    Red = 12,
    /// Light purple.
    LightPurple = 13,
    /// Yellow.
    Yellow = 14,
    /// White.
    White = 15,
}

/// What players can see of each other's name tags (`Team.Visibility`).
#[derive(WriteTo, Clone, Copy, Debug, PartialEq, Eq)]
#[write(as = VarInt)]
pub enum NameTagVisibility {
    /// Always visible.
    Always = 0,
    /// Never visible.
    Never = 1,
    /// Hidden from players on other teams.
    HideForOtherTeams = 2,
    /// Hidden from players on the same team.
    HideForOwnTeam = 3,
}

/// How players collide with each other (`Team.CollisionRule`).
#[derive(WriteTo, Clone, Copy, Debug, PartialEq, Eq)]
#[write(as = VarInt)]
pub enum CollisionRule {
    /// Always collide.
    Always = 0,
    /// Never collide.
    Never = 1,
    /// Only collide with players on other teams.
    PushOtherTeams = 2,
    /// Only collide with players on the same team.
    PushOwnTeam = 3,
}

/// Packed team option flags, matching `PlayerTeam.packOptions`.
///
/// Bit 0 is allow friendly fire, bit 1 is can see friendly invisibles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TeamOptionFlags {
    /// Whether teammates can damage each other.
    pub allow_friendly_fire: bool,
    /// Whether teammates can see invisible teammates.
    pub can_see_friendly_invisibles: bool,
}

impl TeamOptionFlags {
    /// Packs the flags into the single byte vanilla writes.
    #[must_use]
    pub const fn pack(self) -> u8 {
        let friendly = if self.allow_friendly_fire { 1u8 } else { 0u8 };
        let invisibles = if self.can_see_friendly_invisibles {
            1u8
        } else {
            0u8
        };
        friendly | (invisibles << 1)
    }
}

/// Team metadata sent for method 0 (add) and 2 (change).
///
/// Wire order: display name, prefix, suffix, name tag visibility, collision rule,
/// presence-byte optional colour, option flags byte.
#[derive(Clone, Debug)]
pub struct TeamParameters {
    /// Team display name.
    pub display_name: TextComponent,
    /// Prefix prepended to member names.
    pub player_prefix: TextComponent,
    /// Suffix appended to member names.
    pub player_suffix: TextComponent,
    /// Name tag visibility rule.
    pub name_tag_visibility: NameTagVisibility,
    /// Collision rule.
    pub collision_rule: CollisionRule,
    /// Team colour, if set.
    pub color: Option<TeamColor>,
    /// Packed option flags.
    pub options: TeamOptionFlags,
}

impl WriteTo for TeamParameters {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        WriteTo::write(&self.display_name, writer)?;
        WriteTo::write(&self.player_prefix, writer)?;
        WriteTo::write(&self.player_suffix, writer)?;
        self.name_tag_visibility.write(writer)?;
        self.collision_rule.write(writer)?;
        self.color.write(writer)?;
        self.options.pack().write(writer)?;
        Ok(())
    }
}

/// Which change a [`CSetPlayerTeam`] packet applies.
///
/// Vanilla reads the method as a single byte; parameters are present for methods
/// 0 and 2, the player list for methods 0, 3 and 4.
#[derive(WriteTo, Clone, Copy, Debug, PartialEq, Eq)]
#[write(as = u8)]
pub enum TeamAction {
    /// Create the team (method 0): parameters + player list present.
    Add,
    /// Remove the team (method 1): no parameters, no player list.
    Remove,
    /// Change the team metadata (method 2): parameters only.
    Change,
    /// Add players to the team (method 3): player list only.
    Join,
    /// Remove players from the team (method 4): player list only.
    Leave,
}

impl TeamAction {
    /// The method byte vanilla reads and writes.
    #[must_use]
    pub const fn method_byte(self) -> u8 {
        self as u8
    }

    /// Whether this method carries the parameters block.
    #[must_use]
    pub const fn has_parameters(self) -> bool {
        matches!(self, Self::Add | Self::Change)
    }

    /// Whether this method carries the player list.
    #[must_use]
    pub const fn has_player_list(self) -> bool {
        matches!(self, Self::Add | Self::Join | Self::Leave)
    }
}

/// Creates, updates or removes a scoreboard team.
///
/// Matches vanilla's `ClientboundSetPlayerTeamPacket`. Wire order: team name,
/// method byte, optional parameters block, optional player list. The method byte
/// is the method discriminant: 0 add, 1 remove, 2 change, 3 join, 4 leave.
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(Play = C_SET_PLAYER_TEAM)]
pub struct CSetPlayerTeam {
    /// Team name.
    pub name: String,
    /// Which change this packet applies.
    pub method: TeamAction,
    /// Team metadata (present for add and change).
    pub parameters: Option<TeamParameters>,
    /// Affected players (present for add, join and leave).
    pub players: Vec<String>,
}

impl CSetPlayerTeam {
    /// Creates a team with the given members.
    #[must_use]
    pub fn add(name: impl Into<String>, parameters: TeamParameters, players: Vec<String>) -> Self {
        Self {
            name: name.into(),
            method: TeamAction::Add,
            parameters: Some(parameters),
            players,
        }
    }

    /// Removes a team.
    #[must_use]
    pub fn remove(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            method: TeamAction::Remove,
            parameters: None,
            players: Vec::new(),
        }
    }

    /// Changes a team's metadata.
    #[must_use]
    pub fn change(name: impl Into<String>, parameters: TeamParameters) -> Self {
        Self {
            name: name.into(),
            method: TeamAction::Change,
            parameters: Some(parameters),
            players: Vec::new(),
        }
    }

    /// Adds players to a team.
    #[must_use]
    pub fn join(name: impl Into<String>, players: Vec<String>) -> Self {
        Self {
            name: name.into(),
            method: TeamAction::Join,
            parameters: None,
            players,
        }
    }

    /// Removes players from a team.
    #[must_use]
    pub fn leave(name: impl Into<String>, players: Vec<String>) -> Self {
        Self {
            name: name.into(),
            method: TeamAction::Leave,
            parameters: None,
            players,
        }
    }
}

impl WriteTo for CSetPlayerTeam {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        self.name.write_prefixed::<VarInt>(writer)?;
        let method = self.method.method_byte();
        method.write(writer)?;
        if self.method.has_parameters() {
            // Vanilla throws when the parameters block is missing for these methods.
            let parameters = self.parameters.as_ref().ok_or_else(|| {
                Error::other(format!("Parameters not present, but method is {method}"))
            })?;
            WriteTo::write(parameters, writer)?;
        }
        if self.method.has_player_list() {
            // `PLAYER_LIST_STREAM_CODEC`: VarInt count followed by VarInt-prefixed names.
            VarInt(self.players.len() as i32).write(writer)?;
            for player in &self.players {
                player.write_prefixed::<VarInt>(writer)?;
            }
        }
        Ok(())
    }
}
