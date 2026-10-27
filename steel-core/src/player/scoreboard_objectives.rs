//! The scoreboard criteria auto-update driver.
//!
//! Vanilla keeps the score of every objective whose criterion is derived from
//! player state up to date from three places:
//!
//! - `ServerPlayer.awardStat` pushes a stat straight into every objective
//!   carrying that stat's criterion, which is what moves `minecraft.mined:*`,
//!   `minecraft.killed:*`, `minecraft.crafted:*` and the `minecraft.custom:*`
//!   families.
//! - `ServerPlayer.tick` (through `updateScoreForCriteria`) refreshes the
//!   read-only `health`, `food`, `air`, `armor`, `xp` and `level` objectives from
//!   the player each tick, comparing against the `lastRecorded*` fields so
//!   identical values are not written (and therefore not broadcast) again.
//! - `ServerPlayer.die` and `ServerPlayer.awardKillScore` drive `deathCount`,
//!   `totalKillCount`, `playerKillCount` and the `teamkill.*`/`killedByTeam.*`
//!   criteria.
//!
//! Steel routes all three through this module. `dummy` and `trigger` criteria
//! describe nothing derived from player state, so no driver touches them:
//! `dummy` is command-only and `trigger` belongs to `/trigger`.

use crate::entity::{Entity, LivingEntity};
use crate::scoreboard::{ObjectiveCriteria, ScoreHolder, Scoreboard, ScoreboardError, TeamColor};
use steel_registry::stat::Stat;

use super::Player;

/// Vanilla `ServerPlayer`'s `lastRecorded*` fields: the last player values that
/// were written into the derived criteria.
///
/// The initial values are vanilla's field initialisers: `Float.MIN_VALUE`
/// (the smallest positive subnormal float) for health and `Integer.MIN_VALUE`
/// for the integer values, so the first tick writes every criterion.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RecordedCriteriaValues {
    health_and_absorption: f32,
    food_level: i32,
    air_level: i32,
    armor: i32,
    total_experience: i32,
    level: i32,
}

impl RecordedCriteriaValues {
    /// Vanilla `new ServerPlayer(...)`'s `lastRecorded*` initialisers.
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            health_and_absorption: f32::from_bits(1),
            food_level: i32::MIN,
            air_level: i32::MIN,
            armor: i32::MIN,
            total_experience: i32::MIN,
            level: i32::MIN,
        }
    }
}

impl Default for RecordedCriteriaValues {
    fn default() -> Self {
        Self::new()
    }
}

/// A derived criterion whose changed value has to be written this tick.
struct DerivedCriteria {
    /// The objective criterion the value is written into.
    criteria: ObjectiveCriteria,
    /// The value exactly as vanilla's `updateScoreForCriteria` receives it.
    value: i32,
}

impl Player {
    /// The score holder vanilla passes for the player itself (`this`).
    fn objective_score_holder(&self) -> ScoreHolder {
        ScoreHolder::new(self.scoreboard_name())
    }

    /// Runs `update` against this player's domain scoreboard.
    ///
    /// Detached players (tests, and the window before a player joins a domain)
    /// have no scoreboard, matching how the rest of Steel degrades without a
    /// server. Vanilla always has one.
    fn with_domain_scoreboard<R>(&self, update: impl FnOnce(&Scoreboard) -> R) -> Option<R> {
        let server = self.server.upgrade()?;
        let scoreboard = server.scoreboards.get(self.get_world().domain())?;
        Some(update(scoreboard))
    }

    /// Vanilla `ServerPlayer.awardStat`'s scoreboard half: adds each awarded
    /// stat's count to every objective using that stat's criterion.
    ///
    /// Takes a batch so a tick's stat awards ([`super::Player::tick_stats`]) can
    /// share one stats-counter lock and still drive the scoreboard.
    pub(crate) fn drive_stat_objectives(&self, awards: &[(Stat, i32)]) {
        if awards.is_empty() {
            return;
        }
        let holder = self.objective_score_holder();
        self.with_domain_scoreboard(|scoreboard| {
            for &(stat, count) in awards {
                let criteria = ObjectiveCriteria::from_stat(&stat);
                report_criteria_error(
                    scoreboard.add_to_objectives(&holder, &criteria, count),
                    criteria.name(),
                );
            }
        });
    }

    /// Vanilla `ServerPlayer.resetStat`'s scoreboard half: clears every objective
    /// that tracks `stat`.
    pub(crate) fn clear_stat_objectives(&self, stat: Stat) {
        let holder = self.objective_score_holder();
        let criteria = ObjectiveCriteria::from_stat(&stat);
        self.with_domain_scoreboard(|scoreboard| {
            report_criteria_error(
                scoreboard.update_objectives(&holder, &criteria, 0),
                criteria.name(),
            );
        });
    }

    /// Vanilla `ServerPlayer.die`: advances every `deathCount` objective.
    pub(crate) fn award_death_objectives(&self) {
        self.add_to_objectives(&ObjectiveCriteria::death_count(), 1);
    }

    /// Vanilla `ServerPlayer.awardKillScore`: advances every `totalKillCount`
    /// objective.
    pub(crate) fn award_total_kill_objectives(&self) {
        self.add_to_objectives(&ObjectiveCriteria::total_kill_count(), 1);
    }

    /// Vanilla `ServerPlayer.awardKillScore`'s player branch: advances every
    /// `playerKillCount` objective.
    pub(crate) fn award_player_kill_objectives(&self) {
        self.add_to_objectives(&ObjectiveCriteria::player_kill_count(), 1);
    }

    /// Vanilla `ServerPlayer.handleTeamKill(this, victim, TEAM_KILL)`: the killer's
    /// `teamkill.<victim's team color>` objective advances when the victim is on a
    /// colored team.
    pub(crate) fn award_team_kill_objectives(&self, victim: &dyn Entity) {
        let Some(color) = self.team_color_of(&victim.scoreboard_name()) else {
            return;
        };
        self.add_to_objectives(&ObjectiveCriteria::team_kill(color), 1);
    }

    /// Vanilla `ServerPlayer.handleTeamKill(victim, this, KILLED_BY_TEAM)`: the
    /// victim's `killedByTeam.<killer's team color>` objective advances when the
    /// killer is on a colored team.
    pub(crate) fn award_killed_by_team_objectives(&self, victim: &dyn Entity) {
        let Some(color) = self.team_color_of(&self.scoreboard_name()) else {
            return;
        };
        let victim_holder = ScoreHolder::new(victim.scoreboard_name());
        let criteria = ObjectiveCriteria::killed_by_team(color);
        self.with_domain_scoreboard(|scoreboard| {
            report_criteria_error(
                scoreboard.add_to_objectives(&victim_holder, &criteria, 1),
                criteria.name(),
            );
        });
    }

    /// Vanilla `Scoreboard.getPlayersTeam(...).getColor()` for a score holder.
    fn team_color_of(&self, holder_name: &str) -> Option<TeamColor> {
        let holder = ScoreHolder::new(holder_name.to_owned());
        self.with_domain_scoreboard(|scoreboard| {
            let team = scoreboard.holder_team_name(&holder)?;
            scoreboard.team(&team)?.color()
        })
        .flatten()
    }

    /// Adds `delta` to every objective of this player using `criteria`.
    fn add_to_objectives(&self, criteria: &ObjectiveCriteria, delta: i32) {
        let holder = self.objective_score_holder();
        self.with_domain_scoreboard(|scoreboard| {
            report_criteria_error(
                scoreboard.add_to_objectives(&holder, criteria, delta),
                criteria.name(),
            );
        });
    }

    /// Vanilla `ServerPlayer.tick`'s criteria block: refreshes the read-only
    /// `health`, `food`, `air`, `armor`, `xp` and `level` objectives from the
    /// player's current state.
    ///
    /// Each criterion is written only when its recorded value changed, so an
    /// unchanged tick never touches them.
    pub(crate) fn tick_objective_criteria(&self) {
        // Vanilla reads `getHealth() + getAbsorptionAmount()` and later applies
        // `Mth.ceil` to that float.
        let health_and_absorption = self.get_health() + self.get_absorption_amount();
        let food_level = self.food_data.lock().food_level;
        // Vanilla applies `Mth.ceil` to the integer food/air/armor/experience/level
        // values too, which is the identity for an integer.
        let air_level = self.air_supply();
        let armor = self.get_armor_value();
        let (total_experience, level) = {
            let experience = self.experience.lock();
            (experience.total_points(), experience.level())
        };

        let derived = {
            let mut recorded = self.recorded_criteria.lock();
            let mut derived = Vec::new();
            // Vanilla `ServerPlayer.tick` compares the freshly computed
            // health+absorption against the last recorded value with `!=` on a
            // `float` whose initial value is the `Float.MIN_VALUE` sentinel
            // (`f32::from_bits(1)`), so an exact comparison is the ported
            // behaviour, not an approximation.
            #[expect(
                clippy::float_cmp,
                reason = "vanilla compares the recorded health sentinel with exact float equality"
            )]
            let health_changed = recorded.health_and_absorption != health_and_absorption;
            if health_changed {
                recorded.health_and_absorption = health_and_absorption;
                derived.push(DerivedCriteria {
                    criteria: ObjectiveCriteria::health(),
                    value: health_and_absorption.ceil() as i32,
                });
            }
            if recorded.food_level != food_level {
                recorded.food_level = food_level;
                derived.push(DerivedCriteria {
                    criteria: ObjectiveCriteria::food(),
                    value: food_level,
                });
            }
            if recorded.air_level != air_level {
                recorded.air_level = air_level;
                derived.push(DerivedCriteria {
                    criteria: ObjectiveCriteria::air(),
                    value: air_level,
                });
            }
            if recorded.armor != armor {
                recorded.armor = armor;
                derived.push(DerivedCriteria {
                    criteria: ObjectiveCriteria::armor(),
                    value: armor,
                });
            }
            if recorded.total_experience != total_experience {
                recorded.total_experience = total_experience;
                derived.push(DerivedCriteria {
                    criteria: ObjectiveCriteria::experience(),
                    value: total_experience,
                });
            }
            if recorded.level != level {
                recorded.level = level;
                derived.push(DerivedCriteria {
                    criteria: ObjectiveCriteria::level(),
                    value: level,
                });
            }
            derived
        };
        if derived.is_empty() {
            return;
        }

        let holder = self.objective_score_holder();
        self.with_domain_scoreboard(|scoreboard| {
            for change in derived {
                report_criteria_error(
                    scoreboard.update_objectives(&holder, &change.criteria, change.value),
                    change.criteria.name(),
                );
            }
        });
    }
}

/// The scoreboard rejects only empty holder names, which a player never has, but
/// a dropped update would otherwise be invisible.
fn report_criteria_error(result: Result<(), ScoreboardError>, criteria: &str) {
    if let Err(error) = result {
        log::warn!("failed to update objectives for criteria {criteria}: {error}");
    }
}

#[cfg(test)]
mod tests;
