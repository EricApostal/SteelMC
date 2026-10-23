//! Scoreboard teams.
//!
//! Mirrors vanilla `Team`, `PlayerTeam` and `TeamColor`. The team option set is
//! stored inline in the scoreboard save data the same way vanilla stores it in
//! the `Teams` list of `ScoreboardSaveData`.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use steel_utils::translations;
use text_components::{Modifier, TextComponent, format::Color};

use crate::scoreboard::display::DisplaySlot;

/// Vanilla `TeamColor`: the sixteen fixed team colours.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TeamColor {
    #[serde(rename = "black")]
    Black,
    #[serde(rename = "dark_blue")]
    DarkBlue,
    #[serde(rename = "dark_green")]
    DarkGreen,
    #[serde(rename = "dark_aqua")]
    DarkAqua,
    #[serde(rename = "dark_red")]
    DarkRed,
    #[serde(rename = "dark_purple")]
    DarkPurple,
    #[serde(rename = "gold")]
    Gold,
    #[serde(rename = "gray")]
    Gray,
    #[serde(rename = "dark_gray")]
    DarkGray,
    #[serde(rename = "blue")]
    Blue,
    #[serde(rename = "green")]
    Green,
    #[serde(rename = "aqua")]
    Aqua,
    #[serde(rename = "red")]
    Red,
    #[serde(rename = "light_purple")]
    LightPurple,
    #[serde(rename = "yellow")]
    Yellow,
    #[serde(rename = "white")]
    White,
}

impl TeamColor {
    /// Every team colour in vanilla wire-id order.
    pub const ALL: [Self; 16] = [
        Self::Black,
        Self::DarkBlue,
        Self::DarkGreen,
        Self::DarkAqua,
        Self::DarkRed,
        Self::DarkPurple,
        Self::Gold,
        Self::Gray,
        Self::DarkGray,
        Self::Blue,
        Self::Green,
        Self::Aqua,
        Self::Red,
        Self::LightPurple,
        Self::Yellow,
        Self::White,
    ];

    /// Vanilla `TeamColor.id`, also the wire id.
    #[must_use]
    pub const fn id(self) -> i32 {
        match self {
            Self::Black => 0,
            Self::DarkBlue => 1,
            Self::DarkGreen => 2,
            Self::DarkAqua => 3,
            Self::DarkRed => 4,
            Self::DarkPurple => 5,
            Self::Gold => 6,
            Self::Gray => 7,
            Self::DarkGray => 8,
            Self::Blue => 9,
            Self::Green => 10,
            Self::Aqua => 11,
            Self::Red => 12,
            Self::LightPurple => 13,
            Self::Yellow => 14,
            Self::White => 15,
        }
    }

    /// Vanilla `TeamColor.getSerializedName`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Black => "black",
            Self::DarkBlue => "dark_blue",
            Self::DarkGreen => "dark_green",
            Self::DarkAqua => "dark_aqua",
            Self::DarkRed => "dark_red",
            Self::DarkPurple => "dark_purple",
            Self::Gold => "gold",
            Self::Gray => "gray",
            Self::DarkGray => "dark_gray",
            Self::Blue => "blue",
            Self::Green => "green",
            Self::Aqua => "aqua",
            Self::Red => "red",
            Self::LightPurple => "light_purple",
            Self::Yellow => "yellow",
            Self::White => "white",
        }
    }

    /// Vanilla `TeamColor.byName`.
    #[must_use]
    pub fn from_str(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|color| color.as_str() == name)
    }

    /// Vanilla `ByIdMap` behaviour: unknown ids map to the first colour.
    #[must_use]
    pub const fn by_id(id: i32) -> Self {
        match id {
            1 => Self::DarkBlue,
            2 => Self::DarkGreen,
            3 => Self::DarkAqua,
            4 => Self::DarkRed,
            5 => Self::DarkPurple,
            6 => Self::Gold,
            7 => Self::Gray,
            8 => Self::DarkGray,
            9 => Self::Blue,
            10 => Self::Green,
            11 => Self::Aqua,
            12 => Self::Red,
            13 => Self::LightPurple,
            14 => Self::Yellow,
            15 => Self::White,
            _ => Self::Black,
        }
    }

    /// Vanilla `TeamColor.textColor`.
    #[must_use]
    pub const fn text_color(self) -> Color {
        match self {
            Self::Black => Color::Black,
            Self::DarkBlue => Color::DarkBlue,
            Self::DarkGreen => Color::DarkGreen,
            Self::DarkAqua => Color::DarkAqua,
            Self::DarkRed => Color::DarkRed,
            Self::DarkPurple => Color::DarkPurple,
            Self::Gold => Color::Gold,
            Self::Gray => Color::Gray,
            Self::DarkGray => Color::DarkGray,
            Self::Blue => Color::Blue,
            Self::Green => Color::Green,
            Self::Aqua => Color::Aqua,
            Self::Red => Color::Red,
            Self::LightPurple => Color::LightPurple,
            Self::Yellow => Color::Yellow,
            Self::White => Color::White,
        }
    }

    /// Vanilla `TeamColor.displaySlot`.
    #[must_use]
    pub const fn display_slot(self) -> DisplaySlot {
        match self {
            Self::Black => DisplaySlot::SidebarTeamBlack,
            Self::DarkBlue => DisplaySlot::SidebarTeamDarkBlue,
            Self::DarkGreen => DisplaySlot::SidebarTeamDarkGreen,
            Self::DarkAqua => DisplaySlot::SidebarTeamDarkAqua,
            Self::DarkRed => DisplaySlot::SidebarTeamDarkRed,
            Self::DarkPurple => DisplaySlot::SidebarTeamDarkPurple,
            Self::Gold => DisplaySlot::SidebarTeamGold,
            Self::Gray => DisplaySlot::SidebarTeamGray,
            Self::DarkGray => DisplaySlot::SidebarTeamDarkGray,
            Self::Blue => DisplaySlot::SidebarTeamBlue,
            Self::Green => DisplaySlot::SidebarTeamGreen,
            Self::Aqua => DisplaySlot::SidebarTeamAqua,
            Self::Red => DisplaySlot::SidebarTeamRed,
            Self::LightPurple => DisplaySlot::SidebarTeamLightPurple,
            Self::Yellow => DisplaySlot::SidebarTeamYellow,
            Self::White => DisplaySlot::SidebarTeamWhite,
        }
    }

    /// Vanilla `TeamColor.rgb`.
    #[must_use]
    pub const fn rgb(self) -> i32 {
        match self {
            Self::Black => 0x0000_0000,
            Self::DarkBlue => 0x0000_00AA,
            Self::DarkGreen => 0x0000_AA00,
            Self::DarkAqua => 0x0000_AAAA,
            Self::DarkRed => 0x00AA_0000,
            Self::DarkPurple => 0x00AA_00AA,
            Self::Gold => 0x00FF_AA00,
            Self::Gray => 0x00AA_AAAA,
            Self::DarkGray => 0x0055_5555,
            Self::Blue => 0x0055_55FF,
            Self::Green => 0x0055_FF55,
            Self::Aqua => 0x0055_FFFF,
            Self::Red => 0x00FF_5555,
            Self::LightPurple => 0x00FF_55FF,
            Self::Yellow => 0x00FF_FF55,
            Self::White => 0x00FF_FFFF,
        }
    }
}

/// Vanilla `Team.Visibility`: who may see a teammate's nametag or death message.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum TeamVisibility {
    /// Visible to everyone.
    #[default]
    #[serde(rename = "always")]
    Always,
    /// Hidden from everyone.
    #[serde(rename = "never")]
    Never,
    /// Hidden from players outside the team.
    #[serde(rename = "hideForOtherTeams")]
    HideForOtherTeams,
    /// Hidden from the team's own members.
    #[serde(rename = "hideForOwnTeam")]
    HideForOwnTeam,
}

impl TeamVisibility {
    /// Every visibility in vanilla wire-id order.
    pub const ALL: [Self; 4] = [
        Self::Always,
        Self::Never,
        Self::HideForOtherTeams,
        Self::HideForOwnTeam,
    ];

    /// Vanilla `Team.Visibility` wire id.
    #[must_use]
    pub const fn id(self) -> i32 {
        match self {
            Self::Always => 0,
            Self::Never => 1,
            Self::HideForOtherTeams => 2,
            Self::HideForOwnTeam => 3,
        }
    }

    /// Vanilla `Team.Visibility.getSerializedName`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Never => "never",
            Self::HideForOtherTeams => "hideForOtherTeams",
            Self::HideForOwnTeam => "hideForOwnTeam",
        }
    }

    /// Parses a vanilla visibility name.
    #[must_use]
    pub fn from_str(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.as_str() == name)
    }
}

/// Vanilla `Team.CollisionRule`: how teammates push each other.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum CollisionRule {
    /// Everyone collides.
    #[default]
    #[serde(rename = "always")]
    Always,
    /// Nobody collides.
    #[serde(rename = "never")]
    Never,
    /// Only non-teammates collide.
    #[serde(rename = "pushOtherTeams")]
    PushOtherTeams,
    /// Only teammates collide.
    #[serde(rename = "pushOwnTeam")]
    PushOwnTeam,
}

impl CollisionRule {
    /// Every collision rule in vanilla wire-id order.
    pub const ALL: [Self; 4] = [
        Self::Always,
        Self::Never,
        Self::PushOtherTeams,
        Self::PushOwnTeam,
    ];

    /// Vanilla `Team.CollisionRule` wire id.
    #[must_use]
    pub const fn id(self) -> i32 {
        match self {
            Self::Always => 0,
            Self::Never => 1,
            Self::PushOtherTeams => 2,
            Self::PushOwnTeam => 3,
        }
    }

    /// Vanilla `Team.CollisionRule.getSerializedName`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Never => "never",
            Self::PushOtherTeams => "pushOtherTeams",
            Self::PushOwnTeam => "pushOwnTeam",
        }
    }

    /// Parses a vanilla collision-rule name.
    #[must_use]
    pub fn from_str(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|value| value.as_str() == name)
    }
}

const fn default_true() -> bool {
    true
}

/// Vanilla `PlayerTeam`: a named team and its display options.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerTeam {
    name: String,
    #[serde(default)]
    display_name: TextComponent,
    #[serde(default)]
    prefix: TextComponent,
    #[serde(default)]
    suffix: TextComponent,
    #[serde(default)]
    color: Option<TeamColor>,
    #[serde(default = "default_true")]
    allow_friendly_fire: bool,
    #[serde(default = "default_true")]
    see_friendly_invisibles: bool,
    #[serde(default)]
    name_tag_visibility: TeamVisibility,
    #[serde(default)]
    death_message_visibility: TeamVisibility,
    #[serde(default)]
    collision_rule: CollisionRule,
    #[serde(default)]
    players: BTreeSet<String>,
}

impl PlayerTeam {
    /// Creates a team with vanilla's defaults.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            display_name: TextComponent::plain(name.clone()),
            name,
            prefix: TextComponent::new(),
            suffix: TextComponent::new(),
            color: None,
            allow_friendly_fire: true,
            see_friendly_invisibles: true,
            name_tag_visibility: TeamVisibility::Always,
            death_message_visibility: TeamVisibility::Always,
            collision_rule: CollisionRule::Always,
            players: BTreeSet::new(),
        }
    }

    /// Returns the team name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the raw display name.
    #[must_use]
    pub const fn display_name(&self) -> &TextComponent {
        &self.display_name
    }

    /// Replaces the display name.
    pub fn set_display_name(&mut self, display_name: TextComponent) {
        self.display_name = display_name;
    }

    /// Returns the member name prefix.
    #[must_use]
    pub const fn prefix(&self) -> &TextComponent {
        &self.prefix
    }

    /// Replaces the member name prefix.
    pub fn set_prefix(&mut self, prefix: TextComponent) {
        self.prefix = prefix;
    }

    /// Returns the member name suffix.
    #[must_use]
    pub const fn suffix(&self) -> &TextComponent {
        &self.suffix
    }

    /// Replaces the member name suffix.
    pub fn set_suffix(&mut self, suffix: TextComponent) {
        self.suffix = suffix;
    }

    /// Returns the optional team colour.
    #[must_use]
    pub const fn color(&self) -> Option<TeamColor> {
        self.color
    }

    /// Replaces the optional team colour.
    pub fn set_color(&mut self, color: Option<TeamColor>) {
        self.color = color;
    }

    /// Vanilla `Team.isAllowFriendlyFire`.
    #[must_use]
    pub const fn allow_friendly_fire(&self) -> bool {
        self.allow_friendly_fire
    }

    /// Vanilla `PlayerTeam.setAllowFriendlyFire`.
    pub fn set_allow_friendly_fire(&mut self, allow: bool) {
        self.allow_friendly_fire = allow;
    }

    /// Vanilla `Team.canSeeFriendlyInvisibles`.
    #[must_use]
    pub const fn see_friendly_invisibles(&self) -> bool {
        self.see_friendly_invisibles
    }

    /// Vanilla `PlayerTeam.setSeeFriendlyInvisibles`.
    pub fn set_see_friendly_invisibles(&mut self, see: bool) {
        self.see_friendly_invisibles = see;
    }

    /// Vanilla `Team.getNameTagVisibility`.
    #[must_use]
    pub const fn name_tag_visibility(&self) -> TeamVisibility {
        self.name_tag_visibility
    }

    /// Vanilla `PlayerTeam.setNameTagVisibility`.
    pub fn set_name_tag_visibility(&mut self, visibility: TeamVisibility) {
        self.name_tag_visibility = visibility;
    }

    /// Vanilla `Team.getDeathMessageVisibility`.
    #[must_use]
    pub const fn death_message_visibility(&self) -> TeamVisibility {
        self.death_message_visibility
    }

    /// Vanilla `PlayerTeam.setDeathMessageVisibility`.
    pub fn set_death_message_visibility(&mut self, visibility: TeamVisibility) {
        self.death_message_visibility = visibility;
    }

    /// Vanilla `Team.getCollisionRule`.
    #[must_use]
    pub const fn collision_rule(&self) -> CollisionRule {
        self.collision_rule
    }

    /// Vanilla `PlayerTeam.setCollisionRule`.
    pub fn set_collision_rule(&mut self, rule: CollisionRule) {
        self.collision_rule = rule;
    }

    /// Vanilla `Team.getPlayers`, the member score-holder names.
    #[must_use]
    pub const fn players(&self) -> &BTreeSet<String> {
        &self.players
    }

    /// Vanilla `Team.addPlayer`: returns whether the member was added.
    pub fn add_player(&mut self, holder: impl Into<String>) -> bool {
        self.players.insert(holder.into())
    }

    /// Vanilla `Team.removePlayer`: returns whether the member was removed.
    pub fn remove_player(&mut self, holder: &str) -> bool {
        self.players.remove(holder)
    }

    /// Applies the team colour to a component, if one is set.
    #[must_use]
    pub fn apply_color(&self, component: TextComponent) -> TextComponent {
        match self.color {
            Some(color) => component.color(color.text_color()),
            None => component,
        }
    }

    /// Vanilla `PlayerTeam.getFormattedName`: prefix + member + suffix, coloured.
    #[must_use]
    pub fn formatted_name(&self, member: TextComponent) -> TextComponent {
        let mut result = TextComponent::new();
        result.children.push(self.prefix.clone());
        result.children.push(member);
        result.children.push(self.suffix.clone());
        self.apply_color(result)
    }

    /// Vanilla `PlayerTeam.getFormattedDisplayName`: coloured bracketed display name.
    #[must_use]
    pub fn formatted_display_name(&self) -> TextComponent {
        let styled = self
            .display_name
            .clone()
            .insertion(self.name.clone())
            .hover_event(text_components::interactivity::HoverEvent::show_text(
                self.name.clone(),
            ));
        self.apply_color(wrap_in_square_brackets(styled))
    }
}

/// Vanilla `ComponentUtils.wrapInSquareBrackets`.
#[must_use]
pub fn wrap_in_square_brackets(inner: TextComponent) -> TextComponent {
    TextComponent::translated(translations::CHAT_SQUARE_BRACKETS.message([inner]))
}

/// Vanilla `PlayerTeam.formatNameForTeam`.
#[must_use]
pub fn format_name_for_team(team: Option<&PlayerTeam>, name: TextComponent) -> TextComponent {
    match team {
        Some(team) => team.formatted_name(name),
        None => name,
    }
}

#[cfg(test)]
mod tests {
    use super::{CollisionRule, PlayerTeam, TeamColor, TeamVisibility, format_name_for_team};
    use text_components::TextComponent;

    #[test]
    fn team_colors_match_vanilla_wire_ids() {
        assert_eq!(TeamColor::ALL.len(), 16);
        for (index, color) in TeamColor::ALL.into_iter().enumerate() {
            assert_eq!(color.id(), index as i32);
            assert_eq!(TeamColor::by_id(index as i32), color);
            assert_eq!(TeamColor::from_str(color.as_str()), Some(color));
        }
    }

    #[test]
    fn visibility_and_collision_round_trip_by_name() {
        for visibility in TeamVisibility::ALL {
            assert_eq!(
                TeamVisibility::from_str(visibility.as_str()),
                Some(visibility)
            );
        }
        for rule in CollisionRule::ALL {
            assert_eq!(CollisionRule::from_str(rule.as_str()), Some(rule));
        }
    }

    #[test]
    fn formatting_prefixes_and_suffixes_the_member() {
        let mut team = PlayerTeam::new("red");
        team.set_prefix(TextComponent::plain("<"));
        team.set_suffix(TextComponent::plain(">"));
        team.set_allow_friendly_fire(false);

        let formatted = format_name_for_team(Some(&team), TextComponent::plain("Steve"));
        assert_eq!(formatted.children.len(), 3);
        assert!(!team.allow_friendly_fire());
        assert_eq!(
            format_name_for_team(None, TextComponent::plain("Steve"))
                .children
                .len(),
            0
        );
    }
}
