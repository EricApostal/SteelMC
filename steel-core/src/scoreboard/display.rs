//! Scoreboard display slots.
//!
//! Mirrors vanilla `DisplaySlot`: the three shared slots plus one sidebar slot
//! per team colour.

use serde::{Deserialize, Serialize};

use crate::scoreboard::team::TeamColor;

/// Vanilla `DisplaySlot`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum DisplaySlot {
    /// Player-list slot.
    #[serde(rename = "list")]
    List,
    /// Main sidebar slot.
    #[serde(rename = "sidebar")]
    Sidebar,
    /// Slot below the player's name.
    #[serde(rename = "below_name")]
    BelowName,
    /// Sidebar shown to members of the black team.
    #[serde(rename = "sidebar.team.black")]
    SidebarTeamBlack,
    /// Sidebar shown to members of the dark blue team.
    #[serde(rename = "sidebar.team.dark_blue")]
    SidebarTeamDarkBlue,
    /// Sidebar shown to members of the dark green team.
    #[serde(rename = "sidebar.team.dark_green")]
    SidebarTeamDarkGreen,
    /// Sidebar shown to members of the dark aqua team.
    #[serde(rename = "sidebar.team.dark_aqua")]
    SidebarTeamDarkAqua,
    /// Sidebar shown to members of the dark red team.
    #[serde(rename = "sidebar.team.dark_red")]
    SidebarTeamDarkRed,
    /// Sidebar shown to members of the dark purple team.
    #[serde(rename = "sidebar.team.dark_purple")]
    SidebarTeamDarkPurple,
    /// Sidebar shown to members of the gold team.
    #[serde(rename = "sidebar.team.gold")]
    SidebarTeamGold,
    /// Sidebar shown to members of the gray team.
    #[serde(rename = "sidebar.team.gray")]
    SidebarTeamGray,
    /// Sidebar shown to members of the dark gray team.
    #[serde(rename = "sidebar.team.dark_gray")]
    SidebarTeamDarkGray,
    /// Sidebar shown to members of the blue team.
    #[serde(rename = "sidebar.team.blue")]
    SidebarTeamBlue,
    /// Sidebar shown to members of the green team.
    #[serde(rename = "sidebar.team.green")]
    SidebarTeamGreen,
    /// Sidebar shown to members of the aqua team.
    #[serde(rename = "sidebar.team.aqua")]
    SidebarTeamAqua,
    /// Sidebar shown to members of the red team.
    #[serde(rename = "sidebar.team.red")]
    SidebarTeamRed,
    /// Sidebar shown to members of the light purple team.
    #[serde(rename = "sidebar.team.light_purple")]
    SidebarTeamLightPurple,
    /// Sidebar shown to members of the yellow team.
    #[serde(rename = "sidebar.team.yellow")]
    SidebarTeamYellow,
    /// Sidebar shown to members of the white team.
    #[serde(rename = "sidebar.team.white")]
    SidebarTeamWhite,
}

impl DisplaySlot {
    /// Every display slot in vanilla id order.
    pub const ALL: [Self; 19] = [
        Self::List,
        Self::Sidebar,
        Self::BelowName,
        Self::SidebarTeamBlack,
        Self::SidebarTeamDarkBlue,
        Self::SidebarTeamDarkGreen,
        Self::SidebarTeamDarkAqua,
        Self::SidebarTeamDarkRed,
        Self::SidebarTeamDarkPurple,
        Self::SidebarTeamGold,
        Self::SidebarTeamGray,
        Self::SidebarTeamDarkGray,
        Self::SidebarTeamBlue,
        Self::SidebarTeamGreen,
        Self::SidebarTeamAqua,
        Self::SidebarTeamRed,
        Self::SidebarTeamLightPurple,
        Self::SidebarTeamYellow,
        Self::SidebarTeamWhite,
    ];

    /// Vanilla `DisplaySlot.id`, also the wire id.
    #[must_use]
    pub const fn id(self) -> i32 {
        match self {
            Self::List => 0,
            Self::Sidebar => 1,
            Self::BelowName => 2,
            Self::SidebarTeamBlack => 3,
            Self::SidebarTeamDarkBlue => 4,
            Self::SidebarTeamDarkGreen => 5,
            Self::SidebarTeamDarkAqua => 6,
            Self::SidebarTeamDarkRed => 7,
            Self::SidebarTeamDarkPurple => 8,
            Self::SidebarTeamGold => 9,
            Self::SidebarTeamGray => 10,
            Self::SidebarTeamDarkGray => 11,
            Self::SidebarTeamBlue => 12,
            Self::SidebarTeamGreen => 13,
            Self::SidebarTeamAqua => 14,
            Self::SidebarTeamRed => 15,
            Self::SidebarTeamLightPurple => 16,
            Self::SidebarTeamYellow => 17,
            Self::SidebarTeamWhite => 18,
        }
    }

    /// Vanilla `DisplaySlot.getSerializedName`, also the command name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Sidebar => "sidebar",
            Self::BelowName => "below_name",
            Self::SidebarTeamBlack => "sidebar.team.black",
            Self::SidebarTeamDarkBlue => "sidebar.team.dark_blue",
            Self::SidebarTeamDarkGreen => "sidebar.team.dark_green",
            Self::SidebarTeamDarkAqua => "sidebar.team.dark_aqua",
            Self::SidebarTeamDarkRed => "sidebar.team.dark_red",
            Self::SidebarTeamDarkPurple => "sidebar.team.dark_purple",
            Self::SidebarTeamGold => "sidebar.team.gold",
            Self::SidebarTeamGray => "sidebar.team.gray",
            Self::SidebarTeamDarkGray => "sidebar.team.dark_gray",
            Self::SidebarTeamBlue => "sidebar.team.blue",
            Self::SidebarTeamGreen => "sidebar.team.green",
            Self::SidebarTeamAqua => "sidebar.team.aqua",
            Self::SidebarTeamRed => "sidebar.team.red",
            Self::SidebarTeamLightPurple => "sidebar.team.light_purple",
            Self::SidebarTeamYellow => "sidebar.team.yellow",
            Self::SidebarTeamWhite => "sidebar.team.white",
        }
    }

    /// Parses a vanilla display-slot name.
    #[must_use]
    pub fn from_str(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|slot| slot.as_str() == name)
    }

    /// Vanilla `DisplaySlot.BY_ID` behaviour: unknown ids map to `List`.
    #[must_use]
    pub const fn by_id(id: i32) -> Self {
        match id {
            1 => Self::Sidebar,
            2 => Self::BelowName,
            3 => Self::SidebarTeamBlack,
            4 => Self::SidebarTeamDarkBlue,
            5 => Self::SidebarTeamDarkGreen,
            6 => Self::SidebarTeamDarkAqua,
            7 => Self::SidebarTeamDarkRed,
            8 => Self::SidebarTeamDarkPurple,
            9 => Self::SidebarTeamGold,
            10 => Self::SidebarTeamGray,
            11 => Self::SidebarTeamDarkGray,
            12 => Self::SidebarTeamBlue,
            13 => Self::SidebarTeamGreen,
            14 => Self::SidebarTeamAqua,
            15 => Self::SidebarTeamRed,
            16 => Self::SidebarTeamLightPurple,
            17 => Self::SidebarTeamYellow,
            18 => Self::SidebarTeamWhite,
            _ => Self::List,
        }
    }

    /// Returns the team colour whose sidebar this slot is, if any.
    #[must_use]
    pub const fn team_color(self) -> Option<TeamColor> {
        match self {
            Self::SidebarTeamBlack => Some(TeamColor::Black),
            Self::SidebarTeamDarkBlue => Some(TeamColor::DarkBlue),
            Self::SidebarTeamDarkGreen => Some(TeamColor::DarkGreen),
            Self::SidebarTeamDarkAqua => Some(TeamColor::DarkAqua),
            Self::SidebarTeamDarkRed => Some(TeamColor::DarkRed),
            Self::SidebarTeamDarkPurple => Some(TeamColor::DarkPurple),
            Self::SidebarTeamGold => Some(TeamColor::Gold),
            Self::SidebarTeamGray => Some(TeamColor::Gray),
            Self::SidebarTeamDarkGray => Some(TeamColor::DarkGray),
            Self::SidebarTeamBlue => Some(TeamColor::Blue),
            Self::SidebarTeamGreen => Some(TeamColor::Green),
            Self::SidebarTeamAqua => Some(TeamColor::Aqua),
            Self::SidebarTeamRed => Some(TeamColor::Red),
            Self::SidebarTeamLightPurple => Some(TeamColor::LightPurple),
            Self::SidebarTeamYellow => Some(TeamColor::Yellow),
            Self::SidebarTeamWhite => Some(TeamColor::White),
            Self::List | Self::Sidebar | Self::BelowName => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DisplaySlot;

    #[test]
    fn display_slots_match_vanilla_ids_and_names() {
        assert_eq!(DisplaySlot::ALL.len(), 19);
        for (index, slot) in DisplaySlot::ALL.into_iter().enumerate() {
            assert_eq!(slot.id(), index as i32);
            assert_eq!(DisplaySlot::by_id(index as i32), slot);
            assert_eq!(DisplaySlot::from_str(slot.as_str()), Some(slot));
        }
        assert_eq!(DisplaySlot::by_id(99), DisplaySlot::List);
    }

    #[test]
    fn team_sidebar_slots_expose_their_colour() {
        assert!(DisplaySlot::Sidebar.team_color().is_none());
        assert_eq!(
            DisplaySlot::SidebarTeamRed
                .team_color()
                .map(|color| color.rgb()),
            Some(0x00FF_5555)
        );
    }
}
