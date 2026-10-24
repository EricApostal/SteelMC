//! Client presentation packets for scoreboard mutations.
//!
//! Vanilla's `ServerScoreboard` overrides `Scoreboard`'s mutation callbacks and
//! broadcasts the equivalent `Clientbound*Scoreboard*` packets from inside each
//! mutation. Steel queues the same packets on [`super::Scoreboard`] and hands them
//! to the domain's players once per tick, so `/scoreboard` and `/team` bursts reach
//! clients in the order the commands produced them.

use steel_protocol::packets::game::{
    CResetScore, CSetDisplayObjective, CSetObjective, CSetPlayerTeam, CSetScore,
    CollisionRule as WireCollisionRule, NameTagVisibility, NumberFormat, ObjectiveRenderType,
    ScoreboardDisplaySlot, TeamColor as WireTeamColor, TeamOptionFlags, TeamParameters,
};

use crate::player::Player;

use super::{
    DisplaySlot as CoreDisplaySlot, NumberFormat as CoreNumberFormat, PlayerTeam,
    RenderType as CoreRenderType, ScoreState, TeamColor, TeamVisibility,
};

/// A scoreboard client packet.
#[derive(Clone, Debug)]
pub enum ScoreboardPacket {
    /// `ClientboundSetObjectivePacket`.
    Objective(CSetObjective),
    /// `ClientboundSetScorePacket`.
    Score(CSetScore),
    /// `ClientboundResetScorePacket`.
    ResetScore(CResetScore),
    /// `ClientboundSetDisplayObjectivePacket`.
    DisplayObjective(CSetDisplayObjective),
    /// `ClientboundSetPlayerTeamPacket`.
    Team(CSetPlayerTeam),
}

impl ScoreboardPacket {
    /// Vanilla `ServerScoreboard.getStartTrackingPackets` / `onObjectiveAdded`:
    /// `ClientboundSetObjectivePacket(objective, 0)`.
    #[must_use]
    pub(crate) fn objective_add(name: &str, objective: super::ScoreboardObjective) -> Self {
        Self::Objective(CSetObjective::add(
            name.to_owned(),
            objective.display_name().clone(),
            wire_render_type(objective.render_type()),
            wire_number_format(objective.number_format()),
        ))
    }

    /// Vanilla `ServerScoreboard.onObjectiveChanged`:
    /// `ClientboundSetObjectivePacket(objective, 2)`.
    #[must_use]
    pub(crate) fn objective_change(name: &str, objective: super::ScoreboardObjective) -> Self {
        Self::Objective(CSetObjective::change(
            name.to_owned(),
            objective.display_name().clone(),
            wire_render_type(objective.render_type()),
            wire_number_format(objective.number_format()),
        ))
    }

    /// Vanilla `ServerScoreboard.getStopTrackingPackets` / `stopTrackingObjective`:
    /// `ClientboundSetObjectivePacket(objective, 1)`.
    #[must_use]
    pub(crate) fn objective_remove(name: &str) -> Self {
        Self::Objective(CSetObjective::remove(name.to_owned()))
    }

    /// Vanilla `ServerScoreboard.onScoreChanged` and `getStartTrackingPackets`:
    /// `ClientboundSetScorePacket`.
    ///
    /// Both send `Optional.ofNullable(score.numberFormat())`: only the per-score override,
    /// never the objective's format, which travels in `ClientboundSetObjectivePacket`.
    #[must_use]
    pub(crate) fn score_changed(owner: &str, objective: &str, score: &ScoreState) -> Self {
        Self::Score(score_packet(owner, objective, score))
    }

    /// Vanilla `ServerScoreboard.onPlayerScoreRemoved`:
    /// `ClientboundResetScorePacket(owner, objective)`.
    #[must_use]
    pub(crate) fn score_reset(owner: &str, objective: &str) -> Self {
        Self::ResetScore(CResetScore::one(owner.to_owned(), objective.to_owned()))
    }

    /// Vanilla `ServerScoreboard.onPlayerRemoved`:
    /// `ClientboundResetScorePacket(owner, null)`.
    #[must_use]
    pub(crate) fn all_scores_reset(owner: &str) -> Self {
        Self::ResetScore(CResetScore::all(owner.to_owned()))
    }

    /// Vanilla `ClientboundSetDisplayObjectivePacket`, including the
    /// `objective == null` form `ServerScoreboard.setDisplayObjective` sends.
    #[must_use]
    pub(crate) fn display_objective(slot: CoreDisplaySlot, objective: Option<&str>) -> Self {
        Self::DisplayObjective(match objective {
            Some(objective) => {
                CSetDisplayObjective::show(wire_display_slot(slot), objective.to_owned())
            }
            None => CSetDisplayObjective::clear(wire_display_slot(slot)),
        })
    }

    /// Vanilla `ServerScoreboard.onTeamAdded`:
    /// `ClientboundSetPlayerTeamPacket.createAddOrModifyPacket(team, true)`.
    #[must_use]
    pub(crate) fn team_add(name: &str, team: &PlayerTeam) -> Self {
        Self::Team(CSetPlayerTeam::add(
            name.to_owned(),
            team_parameters(team),
            team.players().iter().cloned().collect(),
        ))
    }

    /// Vanilla `ServerScoreboard.onTeamChanged`:
    /// `ClientboundSetPlayerTeamPacket.createAddOrModifyPacket(team, false)`.
    #[must_use]
    pub(crate) fn team_change(name: &str, team: &PlayerTeam) -> Self {
        Self::Team(CSetPlayerTeam::change(
            name.to_owned(),
            team_parameters(team),
        ))
    }

    /// Vanilla `ServerScoreboard.onTeamRemoved`:
    /// `ClientboundSetPlayerTeamPacket.createRemovePacket(team)`.
    #[must_use]
    pub(crate) fn team_remove(name: &str) -> Self {
        Self::Team(CSetPlayerTeam::remove(name.to_owned()))
    }

    /// Vanilla `ServerScoreboard.addPlayerToTeam`:
    /// `createPlayerPacket(team, player, Action.ADD)`.
    #[must_use]
    pub(crate) fn team_player_add(name: &str, player: &str) -> Self {
        Self::Team(CSetPlayerTeam::join(
            name.to_owned(),
            vec![player.to_owned()],
        ))
    }

    /// Vanilla `ServerScoreboard.removePlayerFromTeam`:
    /// `createPlayerPacket(team, player, Action.REMOVE)`.
    #[must_use]
    pub(crate) fn team_player_remove(name: &str, player: &str) -> Self {
        Self::Team(CSetPlayerTeam::leave(
            name.to_owned(),
            vec![player.to_owned()],
        ))
    }

    /// Sends this packet to one player.
    pub fn send_to(&self, player: &Player) {
        match self {
            Self::Objective(packet) => player.send_packet(packet.clone()),
            Self::Score(packet) => player.send_packet(packet.clone()),
            Self::ResetScore(packet) => player.send_packet(packet.clone()),
            Self::DisplayObjective(packet) => player.send_packet(packet.clone()),
            Self::Team(packet) => player.send_packet(packet.clone()),
        }
    }
}

fn score_packet(owner: &str, objective: &str, score: &ScoreState) -> CSetScore {
    CSetScore::new(
        owner.to_owned(),
        objective.to_owned(),
        score.value,
        score.display.clone(),
        wire_number_format(score.number_format.as_ref()),
    )
}

fn team_parameters(team: &PlayerTeam) -> TeamParameters {
    TeamParameters {
        display_name: team.display_name().clone(),
        player_prefix: team.prefix().clone(),
        player_suffix: team.suffix().clone(),
        name_tag_visibility: wire_visibility(team.name_tag_visibility()),
        collision_rule: wire_collision_rule(team.collision_rule()),
        color: team.color().map(wire_team_color),
        options: TeamOptionFlags {
            allow_friendly_fire: team.allow_friendly_fire(),
            can_see_friendly_invisibles: team.see_friendly_invisibles(),
        },
    }
}

/// Vanilla `ObjectiveCriteria.RenderType` -> wire enum.
fn wire_render_type(render_type: CoreRenderType) -> ObjectiveRenderType {
    match render_type {
        CoreRenderType::Integer => ObjectiveRenderType::Integer,
        CoreRenderType::Hearts => ObjectiveRenderType::Hearts,
    }
}

/// Vanilla `DisplaySlot` -> wire enum.
fn wire_display_slot(slot: CoreDisplaySlot) -> ScoreboardDisplaySlot {
    match slot {
        CoreDisplaySlot::List => ScoreboardDisplaySlot::List,
        CoreDisplaySlot::Sidebar => ScoreboardDisplaySlot::Sidebar,
        CoreDisplaySlot::BelowName => ScoreboardDisplaySlot::BelowName,
        CoreDisplaySlot::SidebarTeamBlack => ScoreboardDisplaySlot::TeamBlack,
        CoreDisplaySlot::SidebarTeamDarkBlue => ScoreboardDisplaySlot::TeamDarkBlue,
        CoreDisplaySlot::SidebarTeamDarkGreen => ScoreboardDisplaySlot::TeamDarkGreen,
        CoreDisplaySlot::SidebarTeamDarkAqua => ScoreboardDisplaySlot::TeamDarkAqua,
        CoreDisplaySlot::SidebarTeamDarkRed => ScoreboardDisplaySlot::TeamDarkRed,
        CoreDisplaySlot::SidebarTeamDarkPurple => ScoreboardDisplaySlot::TeamDarkPurple,
        CoreDisplaySlot::SidebarTeamGold => ScoreboardDisplaySlot::TeamGold,
        CoreDisplaySlot::SidebarTeamGray => ScoreboardDisplaySlot::TeamGray,
        CoreDisplaySlot::SidebarTeamDarkGray => ScoreboardDisplaySlot::TeamDarkGray,
        CoreDisplaySlot::SidebarTeamBlue => ScoreboardDisplaySlot::TeamBlue,
        CoreDisplaySlot::SidebarTeamGreen => ScoreboardDisplaySlot::TeamGreen,
        CoreDisplaySlot::SidebarTeamAqua => ScoreboardDisplaySlot::TeamAqua,
        CoreDisplaySlot::SidebarTeamRed => ScoreboardDisplaySlot::TeamRed,
        CoreDisplaySlot::SidebarTeamLightPurple => ScoreboardDisplaySlot::TeamLightPurple,
        CoreDisplaySlot::SidebarTeamYellow => ScoreboardDisplaySlot::TeamYellow,
        CoreDisplaySlot::SidebarTeamWhite => ScoreboardDisplaySlot::TeamWhite,
    }
}

/// Vanilla `TeamColor` -> wire enum.
fn wire_team_color(color: TeamColor) -> WireTeamColor {
    match color {
        TeamColor::Black => WireTeamColor::Black,
        TeamColor::DarkBlue => WireTeamColor::DarkBlue,
        TeamColor::DarkGreen => WireTeamColor::DarkGreen,
        TeamColor::DarkAqua => WireTeamColor::DarkAqua,
        TeamColor::DarkRed => WireTeamColor::DarkRed,
        TeamColor::DarkPurple => WireTeamColor::DarkPurple,
        TeamColor::Gold => WireTeamColor::Gold,
        TeamColor::Gray => WireTeamColor::Gray,
        TeamColor::DarkGray => WireTeamColor::DarkGray,
        TeamColor::Blue => WireTeamColor::Blue,
        TeamColor::Green => WireTeamColor::Green,
        TeamColor::Aqua => WireTeamColor::Aqua,
        TeamColor::Red => WireTeamColor::Red,
        TeamColor::LightPurple => WireTeamColor::LightPurple,
        TeamColor::Yellow => WireTeamColor::Yellow,
        TeamColor::White => WireTeamColor::White,
    }
}

/// Vanilla `Team.Visibility` -> wire enum.
fn wire_visibility(visibility: TeamVisibility) -> NameTagVisibility {
    match visibility {
        TeamVisibility::Always => NameTagVisibility::Always,
        TeamVisibility::Never => NameTagVisibility::Never,
        TeamVisibility::HideForOtherTeams => NameTagVisibility::HideForOtherTeams,
        TeamVisibility::HideForOwnTeam => NameTagVisibility::HideForOwnTeam,
    }
}

/// Vanilla `Team.CollisionRule` -> wire enum.
fn wire_collision_rule(rule: super::CollisionRule) -> WireCollisionRule {
    match rule {
        super::CollisionRule::Always => WireCollisionRule::Always,
        super::CollisionRule::Never => WireCollisionRule::Never,
        super::CollisionRule::PushOtherTeams => WireCollisionRule::PushOtherTeams,
        super::CollisionRule::PushOwnTeam => WireCollisionRule::PushOwnTeam,
    }
}

/// Vanilla `NumberFormat` -> wire enum.
fn wire_number_format(format: Option<&CoreNumberFormat>) -> Option<NumberFormat> {
    format.map(|format| match format {
        CoreNumberFormat::Blank => NumberFormat::Blank,
        CoreNumberFormat::Fixed(value) => NumberFormat::Fixed {
            value: value.clone(),
        },
        CoreNumberFormat::Styled(style) => NumberFormat::styled(style),
    })
}
