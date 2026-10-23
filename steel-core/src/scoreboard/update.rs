//! Client presentation updates for scoreboard mutations.
//!
//! Vanilla's `ServerScoreboard` overrides `Scoreboard`'s mutation callbacks and
//! broadcasts one packet per change. Steel records the same changes as a small
//! queue on [`super::Scoreboard`] and converts them into client packets when the
//! server flushes the queue, so the authoritative state is always read at flush
//! time (`/scoreboard` and `/team` mutate in bursts within one tick).

use steel_protocol::packets::game::{
    CResetScore, CSetDisplayObjective, CSetObjective, CSetPlayerTeam, CSetScore,
    CollisionRule as WireCollisionRule, NameTagVisibility, NumberFormat, ObjectiveRenderType,
    ScoreboardDisplaySlot, TeamColor as WireTeamColor, TeamOptionFlags, TeamParameters,
};

use crate::player::Player;

use super::{
    DisplaySlot as CoreDisplaySlot, NumberFormat as CoreNumberFormat, ObjectiveState,
    PersistentScoreboard, PlayerTeam, RenderType as CoreRenderType, ScoreState, TeamColor,
    TeamVisibility,
};

/// A scoreboard change queued for client presentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScoreboardUpdate {
    /// An objective was created.
    ObjectiveAdded {
        /// Objective name.
        name: String,
    },
    /// An objective's display settings changed.
    ObjectiveChanged {
        /// Objective name.
        name: String,
    },
    /// An objective was removed.
    ObjectiveRemoved {
        /// Objective name.
        name: String,
    },
    /// A holder's score changed in an objective.
    ScoreChanged {
        /// Score holder name.
        owner: String,
        /// Objective name.
        objective: String,
    },
    /// A holder's score was reset, optionally for a single objective.
    ScoreRemoved {
        /// Score holder name.
        owner: String,
        /// Objective name, or `None` to reset every objective.
        objective: Option<String>,
    },
    /// A display slot changed.
    DisplaySlotChanged {
        /// Slot that changed.
        slot: CoreDisplaySlot,
    },
    /// A team was created.
    TeamAdded {
        /// Team name.
        name: String,
    },
    /// A team's display options changed.
    TeamChanged {
        /// Team name.
        name: String,
    },
    /// A team was removed.
    TeamRemoved {
        /// Team name.
        name: String,
    },
    /// Team membership changed.
    TeamPlayersChanged {
        /// Team name.
        name: String,
        /// Holders added to the team.
        joined: Vec<String>,
        /// Holders removed from the team.
        left: Vec<String>,
    },
}

impl ScoreboardUpdate {
    /// Converts this update into the client packets vanilla would broadcast.
    #[must_use]
    pub fn packets(&self, state: &PersistentScoreboard) -> Vec<ScoreboardPacket> {
        match self {
            Self::ObjectiveAdded { name } => {
                let Some(objective) = state.objectives.get(name) else {
                    return Vec::new();
                };
                vec![ScoreboardPacket::objective_add(
                    name,
                    super::objective_snapshot(name, objective),
                )]
            }
            Self::ObjectiveChanged { name } => {
                let Some(objective) = state.objectives.get(name) else {
                    return Vec::new();
                };
                vec![ScoreboardPacket::Objective(CSetObjective::change(
                    name.clone(),
                    objective.display_name.clone(),
                    wire_render_type(objective.render_type),
                    wire_number_format(objective.number_format.as_ref()),
                ))]
            }
            Self::ObjectiveRemoved { name } => {
                vec![ScoreboardPacket::Objective(CSetObjective::remove(
                    name.clone(),
                ))]
            }
            Self::ScoreChanged { owner, objective } => {
                let Some(score) = state
                    .scores
                    .get(owner)
                    .and_then(|scores| scores.get(objective))
                else {
                    return Vec::new();
                };
                vec![ScoreboardPacket::Score(score_packet(
                    owner, objective, score, state,
                ))]
            }
            Self::ScoreRemoved { owner, objective } => match objective {
                Some(objective) => vec![ScoreboardPacket::ResetScore(CResetScore::one(
                    owner.clone(),
                    objective.clone(),
                ))],
                None => vec![ScoreboardPacket::ResetScore(CResetScore::all(
                    owner.clone(),
                ))],
            },
            Self::DisplaySlotChanged { slot } => vec![ScoreboardPacket::DisplayObjective(
                match state.display_slots.get(slot) {
                    Some(objective) => {
                        CSetDisplayObjective::show(wire_display_slot(*slot), objective.clone())
                    }
                    None => CSetDisplayObjective::clear(wire_display_slot(*slot)),
                },
            )],
            Self::TeamAdded { name } => {
                let Some(team) = state.teams.get(name) else {
                    return Vec::new();
                };
                vec![ScoreboardPacket::Team(CSetPlayerTeam::add(
                    name.clone(),
                    team_parameters(team),
                    team.players().iter().cloned().collect(),
                ))]
            }
            Self::TeamChanged { name } => {
                let Some(team) = state.teams.get(name) else {
                    return Vec::new();
                };
                vec![ScoreboardPacket::Team(CSetPlayerTeam::change(
                    name.clone(),
                    team_parameters(team),
                ))]
            }
            Self::TeamRemoved { name } => {
                vec![ScoreboardPacket::Team(CSetPlayerTeam::remove(name.clone()))]
            }
            Self::TeamPlayersChanged { name, joined, left } => {
                let mut packets = Vec::new();
                if !left.is_empty() {
                    packets.push(ScoreboardPacket::Team(CSetPlayerTeam::leave(
                        name.clone(),
                        left.clone(),
                    )));
                }
                if !joined.is_empty() {
                    packets.push(ScoreboardPacket::Team(CSetPlayerTeam::join(
                        name.clone(),
                        joined.clone(),
                    )));
                }
                packets
            }
        }
    }
}

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
    /// Builds the objective-add packet for a full-state sync.
    #[must_use]
    pub(crate) fn objective_add(name: &str, objective: super::ScoreboardObjective) -> Self {
        Self::Objective(CSetObjective::add(
            name.to_owned(),
            objective.display_name().clone(),
            wire_render_type(objective.render_type()),
            wire_number_format(objective.number_format()),
        ))
    }

    /// Builds the score packet for a full-state sync.
    #[must_use]
    pub(crate) fn score_changed(
        owner: &str,
        objective: &str,
        score: &ScoreState,
        objective_state: Option<&ObjectiveState>,
    ) -> Self {
        Self::Score(CSetScore::new(
            owner.to_owned(),
            objective.to_owned(),
            score.value,
            score.display.clone(),
            wire_number_format(
                score
                    .number_format
                    .as_ref()
                    .or(objective_state.and_then(|state| state.number_format.as_ref())),
            ),
        ))
    }

    /// Builds the display-slot packet for a full-state sync.
    #[must_use]
    pub(crate) fn display_objective(slot: CoreDisplaySlot, objective: Option<&str>) -> Self {
        Self::DisplayObjective(match objective {
            Some(objective) => {
                CSetDisplayObjective::show(wire_display_slot(slot), objective.to_owned())
            }
            None => CSetDisplayObjective::clear(wire_display_slot(slot)),
        })
    }

    /// Builds the team-add packet for a full-state sync.
    #[must_use]
    pub(crate) fn team_add(name: &str, team: &PlayerTeam) -> Self {
        Self::Team(CSetPlayerTeam::add(
            name.to_owned(),
            team_parameters(team),
            team.players().iter().cloned().collect(),
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

fn score_packet(
    owner: &str,
    objective: &str,
    score: &ScoreState,
    state: &PersistentScoreboard,
) -> CSetScore {
    let number_format = score.number_format.as_ref().or_else(|| {
        state
            .objectives
            .get(objective)
            .and_then(|objective| objective.number_format.as_ref())
    });
    CSetScore::new(
        owner.to_owned(),
        objective.to_owned(),
        score.value,
        score.display.clone(),
        wire_number_format(number_format),
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
