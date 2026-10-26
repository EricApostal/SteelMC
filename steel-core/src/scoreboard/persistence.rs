//! Vanilla-shaped scoreboard persistence.
//!
//! Steel stores the scoreboard through its TOML saved-data boundary, but keeps
//! vanilla's `ScoreboardSaveData` key names and nesting so the persisted shape
//! maps 1:1 onto the vanilla NBT:
//!
//! ```text
//! Objectives:   [{ Name, CriteriaName, DisplayName, RenderType, display_auto_update, format }]
//! PlayerScores: [{ Name, Objective, Score: { Score, Locked, display, format } }]
//! DisplaySlots: { <slot name>: <objective name> }
//! Teams:        [{ Name, DisplayName, TeamColor, AllowFriendlyFire, SeeFriendlyInvisibles,
//!                  MemberNamePrefix, MemberNameSuffix, NameTagVisibility,
//!                  DeathMessageVisibility, CollisionRule, Players }]
//! ```
//!
//! Team membership lives only in `Teams[].Players`, exactly as in vanilla; the
//! holder→team index is rebuilt on load.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use text_components::TextComponent;

use super::{
    CollisionRule, DisplaySlot, NumberFormat, ObjectiveCriteria, ObjectiveState,
    PersistentScoreboard, PlayerTeam, RenderType, ScoreState, TeamColor, TeamVisibility,
};

/// Persisted `ScoreboardSaveData` shape.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct ScoreboardWire {
    #[serde(rename = "Objectives")]
    objectives: Vec<ObjectiveWire>,
    #[serde(rename = "PlayerScores")]
    player_scores: Vec<ScoreWire>,
    #[serde(rename = "DisplaySlots")]
    display_slots: BTreeMap<String, String>,
    #[serde(rename = "Teams")]
    teams: Vec<TeamWire>,
}

/// Vanilla `Objective.Packed`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObjectiveWire {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "CriteriaName", default = "default_criteria")]
    criteria_name: String,
    #[serde(rename = "DisplayName")]
    display_name: TextComponent,
    #[serde(rename = "RenderType", default)]
    render_type: RenderType,
    #[serde(rename = "display_auto_update", default)]
    display_auto_update: bool,
    #[serde(rename = "format", default)]
    format: Option<NumberFormat>,
}

fn default_criteria() -> String {
    "dummy".to_owned()
}

/// Vanilla `Scoreboard.PackedScore`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScoreWire {
    #[serde(rename = "Name")]
    owner: String,
    #[serde(rename = "Objective")]
    objective: String,
    #[serde(rename = "Score")]
    score: ScorePackedWire,
}

/// Vanilla `Score.Packed`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScorePackedWire {
    #[serde(rename = "Score", default)]
    value: i32,
    #[serde(rename = "Locked", default)]
    locked: bool,
    #[serde(rename = "display", default)]
    display: Option<TextComponent>,
    #[serde(rename = "format", default)]
    number_format: Option<NumberFormat>,
}

/// Vanilla `PlayerTeam.Packed`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamWire {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "DisplayName", default)]
    display_name: Option<TextComponent>,
    #[serde(rename = "TeamColor", default)]
    color: Option<TeamColor>,
    #[serde(rename = "AllowFriendlyFire", default = "default_true")]
    allow_friendly_fire: bool,
    #[serde(rename = "SeeFriendlyInvisibles", default = "default_true")]
    see_friendly_invisibles: bool,
    #[serde(rename = "MemberNamePrefix", default)]
    prefix: Option<TextComponent>,
    #[serde(rename = "MemberNameSuffix", default)]
    suffix: Option<TextComponent>,
    #[serde(rename = "NameTagVisibility", default)]
    name_tag_visibility: TeamVisibility,
    #[serde(rename = "DeathMessageVisibility", default)]
    death_message_visibility: TeamVisibility,
    #[serde(rename = "CollisionRule", default)]
    collision_rule: CollisionRule,
    #[serde(rename = "Players", default)]
    players: BTreeSet<String>,
}

const fn default_true() -> bool {
    true
}

impl From<ScoreboardWire> for PersistentScoreboard {
    fn from(wire: ScoreboardWire) -> Self {
        let mut state = Self::default();
        for objective in wire.objectives {
            let criteria = ObjectiveCriteria::parse(&objective.criteria_name)
                .unwrap_or_else(ObjectiveCriteria::dummy);
            state.objectives.insert(
                objective.name,
                ObjectiveState {
                    criteria,
                    display_name: objective.display_name,
                    render_type: objective.render_type,
                    display_auto_update: objective.display_auto_update,
                    number_format: objective.format,
                },
            );
        }
        for score in wire.player_scores {
            // Vanilla drops scores for unknown objectives with a log line.
            if !state.objectives.contains_key(&score.objective) {
                continue;
            }
            state.scores.entry(score.owner).or_default().insert(
                score.objective,
                ScoreState {
                    value: score.score.value,
                    locked: score.score.locked,
                    display: score.score.display,
                    number_format: score.score.number_format,
                },
            );
        }
        for (slot, objective) in wire.display_slots {
            let Some(slot) = DisplaySlot::from_str(&slot) else {
                continue;
            };
            if state.objectives.contains_key(&objective) {
                state.display_slots.insert(slot, objective);
            }
        }
        for team in wire.teams {
            let mut player_team = PlayerTeam::new(team.name.clone());
            if let Some(display_name) = team.display_name {
                player_team.set_display_name(display_name);
            }
            if let Some(prefix) = team.prefix {
                player_team.set_prefix(prefix);
            }
            if let Some(suffix) = team.suffix {
                player_team.set_suffix(suffix);
            }
            player_team.set_color(team.color);
            player_team.set_allow_friendly_fire(team.allow_friendly_fire);
            player_team.set_see_friendly_invisibles(team.see_friendly_invisibles);
            player_team.set_name_tag_visibility(team.name_tag_visibility);
            player_team.set_death_message_visibility(team.death_message_visibility);
            player_team.set_collision_rule(team.collision_rule);
            for player in team.players {
                player_team.add_player(player.clone());
                state.holder_teams.insert(player, team.name.clone());
            }
            state.teams.insert(team.name, player_team);
        }
        state
    }
}

impl From<&PersistentScoreboard> for ScoreboardWire {
    fn from(state: &PersistentScoreboard) -> Self {
        Self {
            objectives: state
                .objectives
                .iter()
                .map(|(name, objective)| ObjectiveWire {
                    name: name.clone(),
                    criteria_name: objective.criteria.name().to_owned(),
                    display_name: objective.display_name.clone(),
                    render_type: objective.render_type,
                    display_auto_update: objective.display_auto_update,
                    format: objective.number_format.clone(),
                })
                .collect(),
            player_scores: state
                .scores
                .iter()
                .flat_map(|(owner, scores)| {
                    scores.iter().map(move |(objective, score)| ScoreWire {
                        owner: owner.clone(),
                        objective: objective.clone(),
                        score: ScorePackedWire {
                            value: score.value,
                            locked: score.locked,
                            display: score.display.clone(),
                            number_format: score.number_format.clone(),
                        },
                    })
                })
                .collect(),
            display_slots: state
                .display_slots
                .iter()
                .map(|(slot, objective)| (slot.as_str().to_owned(), objective.clone()))
                .collect(),
            teams: state
                .teams
                .iter()
                .map(|(name, team)| TeamWire {
                    name: name.clone(),
                    display_name: Some(team.display_name().clone()),
                    color: team.color(),
                    allow_friendly_fire: team.allow_friendly_fire(),
                    see_friendly_invisibles: team.see_friendly_invisibles(),
                    prefix: Some(team.prefix().clone()),
                    suffix: Some(team.suffix().clone()),
                    name_tag_visibility: team.name_tag_visibility(),
                    death_message_visibility: team.death_message_visibility(),
                    collision_rule: team.collision_rule(),
                    players: team.players().clone(),
                })
                .collect(),
        }
    }
}

impl From<PersistentScoreboard> for ScoreboardWire {
    fn from(state: PersistentScoreboard) -> Self {
        Self::from(&state)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{ScoreboardWire, default_criteria};
    use crate::scoreboard::{
        ObjectiveCriteria, ObjectiveState, PersistentScoreboard, RenderType, ScoreState,
    };

    #[test]
    fn wire_uses_vanilla_save_data_keys() {
        let mut state = PersistentScoreboard::default();
        state.objectives.insert(
            "kills".to_owned(),
            ObjectiveState {
                criteria: ObjectiveCriteria::dummy(),
                display_name: text_components::TextComponent::plain("Kills".to_owned()),
                render_type: RenderType::Integer,
                display_auto_update: false,
                number_format: None,
            },
        );
        let mut scores = BTreeMap::new();
        scores.insert(
            "kills".to_owned(),
            ScoreState {
                value: 4,
                ..ScoreState::default()
            },
        );
        state.scores.insert("Steve".to_owned(), scores);

        let wire = ScoreboardWire::from(&state);
        let text = toml::to_string(&wire).expect("scoreboard should serialize");
        assert!(text.contains("[[Objectives]]"));
        assert!(text.contains("CriteriaName = \"dummy\""));
        assert!(text.contains("[[PlayerScores]]"));
        assert!(text.contains("Objective = \"kills\""));

        let restored: ScoreboardWire = toml::from_str(&text).expect("scoreboard should parse");
        let round_tripped = PersistentScoreboard::from(restored);
        assert_eq!(round_tripped.scores["Steve"]["kills"].value, 4);
    }

    #[test]
    fn missing_criteria_defaults_to_dummy() {
        assert_eq!(default_criteria(), "dummy");
    }

    #[test]
    fn unknown_objectives_and_slots_are_dropped_on_load() {
        let text = r#"
[[PlayerScores]]
Name = "Steve"
Objective = "missing"
Score = { Score = 3, Locked = true }
"#;
        let wire: ScoreboardWire = toml::from_str(text).expect("wire should parse");
        let state = PersistentScoreboard::from(wire);
        assert!(state.scores.is_empty());
    }

    #[test]
    fn team_membership_rebuilds_the_holder_index() {
        let text = r#"
[[Teams]]
Name = "red"
TeamColor = "red"
Players = ["Steve"]
"#;
        let wire: ScoreboardWire = toml::from_str(text).expect("wire should parse");
        let state = PersistentScoreboard::from(wire);
        assert_eq!(
            state.holder_teams.get("Steve").map(String::as_str),
            Some("red")
        );
        assert!(state.teams["red"].players().contains("Steve"));
    }
}
