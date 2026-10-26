//! Domain-scoped scoreboard state.
//!
//! This module owns the scoreboard data needed by selectors, command execution
//! and client presentation: objectives and their criteria, scores and locks,
//! teams and their options, and display slots. It mirrors vanilla's
//! `world/scores` package, including the `ServerScoreboard` broadcast surface,
//! which is queued here and flushed once per server tick.
//!
//! Vanilla persists the scoreboard as NBT (`ScoreboardSaveData`); Steel persists
//! the same fields through its domain-scoped TOML saved-data boundary.

use std::{
    cmp::Ordering as CmpOrdering,
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    hash::{Hash, Hasher},
    io,
    mem::take,
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};
use steel_utils::locks::{AsyncMutex, SyncMutex, SyncRwLock};
use steel_utils::saved_data::names as saved_data_names;
use text_components::interactivity::HoverEvent;
use text_components::{Modifier as _, TextComponent};
use thiserror::Error;

use crate::player::Player;
use crate::server::Server;
use crate::server::worlds::WorldMap;
use crate::world::World;

mod criteria;
mod display;
mod format;
mod persistence;
mod team;
mod update;

pub use criteria::{CriteriaKind, ObjectiveCriteria, RenderType};
pub use display::DisplaySlot;
pub use format::{NumberFormat, ScoreExtras};
pub use team::{CollisionRule, PlayerTeam, TeamColor, TeamVisibility, format_name_for_team};
pub use update::ScoreboardPacket;

/// Vanilla `Scoreboard.HIDDEN_SCORE_PREFIX`: holders whose name starts with this
/// are hidden from the sidebar.
pub const HIDDEN_SCORE_PREFIX: &str = "#";

/// Vanilla `ScoreHolder.WILDCARD_NAME`.
pub const WILDCARD_NAME: &str = "*";

/// Score holder name stored by the vanilla scoreboard.
#[derive(Clone, Debug)]
pub struct ScoreHolder {
    name: String,
    display_name: Option<TextComponent>,
}

impl ScoreHolder {
    /// Creates a score holder from its scoreboard name.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            display_name: None,
        }
    }

    /// Creates a score holder with a display name override.
    #[must_use]
    pub fn with_display_name(name: impl Into<String>, display_name: Option<TextComponent>) -> Self {
        Self {
            name: name.into(),
            display_name,
        }
    }

    /// Vanilla `ScoreHolder.WILDCARD`.
    #[must_use]
    pub fn wildcard() -> Self {
        Self::new(WILDCARD_NAME)
    }

    /// Returns the scoreboard name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Vanilla `ScoreHolder.getDisplayName`.
    #[must_use]
    pub const fn display_name(&self) -> Option<&TextComponent> {
        self.display_name.as_ref()
    }

    /// Vanilla `ScoreHolder.getFeedbackDisplayName`: the display name with the
    /// scoreboard name as hover text, or the raw name wrapped in a component.
    #[must_use]
    pub fn feedback_display_name(&self) -> TextComponent {
        match &self.display_name {
            Some(display_name) => display_name
                .clone()
                .hover_event(HoverEvent::show_text(self.name.clone())),
            None => TextComponent::plain(self.name.clone()),
        }
    }

    /// Returns whether this holder is hidden from the sidebar.
    #[must_use]
    pub fn is_hidden(&self) -> bool {
        self.name.starts_with(HIDDEN_SCORE_PREFIX)
    }
}

impl PartialEq for ScoreHolder {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for ScoreHolder {}

impl PartialOrd for ScoreHolder {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScoreHolder {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        self.name.cmp(&other.name)
    }
}

impl Hash for ScoreHolder {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

/// Vanilla `Objective`: an objective's identity and display settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScoreboardObjective {
    name: String,
    criteria: ObjectiveCriteria,
    display_name: TextComponent,
    render_type: RenderType,
    display_auto_update: bool,
    number_format: Option<NumberFormat>,
}

impl ScoreboardObjective {
    /// Returns the objective name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Vanilla `Objective.getCriteria`.
    #[must_use]
    pub const fn criteria(&self) -> &ObjectiveCriteria {
        &self.criteria
    }

    /// Vanilla `Objective.getDisplayName`.
    #[must_use]
    pub const fn display_name(&self) -> &TextComponent {
        &self.display_name
    }

    /// Vanilla `Objective.getRenderType`.
    #[must_use]
    pub const fn render_type(&self) -> RenderType {
        self.render_type
    }

    /// Vanilla `Objective.displayAutoUpdate`.
    #[must_use]
    pub const fn display_auto_update(&self) -> bool {
        self.display_auto_update
    }

    /// Vanilla `Objective.numberFormat`.
    #[must_use]
    pub const fn number_format(&self) -> Option<&NumberFormat> {
        self.number_format.as_ref()
    }

    /// Vanilla `ObjectiveCriteria.isReadOnly`.
    #[must_use]
    pub const fn is_read_only(&self) -> bool {
        self.criteria.is_read_only()
    }

    /// Vanilla `Objective.getFormattedDisplayName`: the display name in square
    /// brackets with the objective name as hover text.
    #[must_use]
    pub fn formatted_display_name(&self) -> TextComponent {
        let styled = self
            .display_name
            .clone()
            .hover_event(HoverEvent::show_text(self.name.clone()));
        team::wrap_in_square_brackets(styled)
    }

    /// Returns whether commands may change scores for this objective.
    #[must_use]
    pub const fn is_writable(&self) -> bool {
        !self.criteria.is_read_only()
    }
}

/// Stored score fields used by command execution and presentation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScoreInfo {
    value: i32,
    locked: bool,
    extras: ScoreExtras,
}

impl ScoreInfo {
    /// Returns the integer score value.
    #[must_use]
    pub const fn value(&self) -> i32 {
        self.value
    }

    /// Returns whether `/trigger`-style writes are locked.
    #[must_use]
    pub const fn is_locked(&self) -> bool {
        self.locked
    }

    /// Vanilla `ScoreAccess.display`.
    #[must_use]
    pub const fn display(&self) -> Option<&TextComponent> {
        self.extras.display.as_ref()
    }

    /// Vanilla `ScoreAccess.numberFormatOverride`.
    #[must_use]
    pub const fn number_format(&self) -> Option<&NumberFormat> {
        self.extras.number_format.as_ref()
    }

    /// Returns the stored per-score extras.
    #[must_use]
    pub const fn extras(&self) -> &ScoreExtras {
        &self.extras
    }
}

const fn default_score_locked() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ObjectiveState {
    criteria: ObjectiveCriteria,
    display_name: TextComponent,
    render_type: RenderType,
    display_auto_update: bool,
    number_format: Option<NumberFormat>,
}

impl Default for ObjectiveState {
    fn default() -> Self {
        Self {
            criteria: ObjectiveCriteria::dummy(),
            display_name: TextComponent::new(),
            render_type: RenderType::Integer,
            display_auto_update: false,
            number_format: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ScoreState {
    value: i32,
    #[serde(default = "default_score_locked")]
    locked: bool,
    display: Option<TextComponent>,
    number_format: Option<NumberFormat>,
}

impl Default for ScoreState {
    fn default() -> Self {
        Self {
            value: 0,
            locked: true,
            display: None,
            number_format: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    from = "persistence::ScoreboardWire",
    into = "persistence::ScoreboardWire"
)]
struct PersistentScoreboard {
    objectives: BTreeMap<String, ObjectiveState>,
    scores: BTreeMap<String, BTreeMap<String, ScoreState>>,
    teams: BTreeMap<String, PlayerTeam>,
    holder_teams: BTreeMap<String, String>,
    display_slots: BTreeMap<DisplaySlot, String>,
}

/// Invalid scoreboard operation or persisted state.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ScoreboardError {
    /// Objective names may not be empty.
    #[error("objective name cannot be empty")]
    EmptyObjectiveName,
    /// Team names may not be empty.
    #[error("team name cannot be empty")]
    EmptyTeamName,
    /// Score holder names may not be empty.
    #[error("score holder name cannot be empty")]
    EmptyScoreHolderName,
    /// An objective already exists.
    #[error("objective '{0}' already exists")]
    DuplicateObjective(String),
    /// A team already exists.
    #[error("team '{0}' already exists")]
    DuplicateTeam(String),
    /// The requested objective does not exist.
    #[error("objective '{0}' does not exist")]
    MissingObjective(String),
    /// The requested team does not exist.
    #[error("team '{0}' does not exist")]
    MissingTeam(String),
    /// The objective cannot be written because its criteria are read-only.
    #[error("objective '{0}' is read-only")]
    ReadOnlyObjective(String),
    /// The score holder does not belong to the team.
    #[error("'{0}' is not on team '{1}'")]
    HolderNotInTeam(String, String),
}

struct ScoreboardSaveSnapshot {
    revision: u64,
    state: PersistentScoreboard,
}

/// Command-facing scoreboard for one Steel domain.
pub struct Scoreboard {
    state: SyncRwLock<PersistentScoreboard>,
    revision: AtomicU64,
    saved_revision: AtomicU64,
    /// Objectives every client in this domain currently tracks, mirroring vanilla
    /// `ServerScoreboard.trackedObjectives`: an objective is tracked while at least one
    /// display slot shows it, which is also when vanilla broadcasts its score changes.
    tracked: SyncRwLock<BTreeSet<String>>,
    /// `Clientbound*Scoreboard*` packets queued for the next per-tick flush.
    pending: SyncMutex<Vec<ScoreboardPacket>>,
}

impl Scoreboard {
    /// Creates an empty, clean scoreboard.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: SyncRwLock::new(PersistentScoreboard::default()),
            revision: AtomicU64::new(0),
            saved_revision: AtomicU64::new(0),
            tracked: SyncRwLock::new(BTreeSet::new()),
            pending: SyncMutex::new(Vec::new()),
        }
    }

    fn from_persistent(state: PersistentScoreboard) -> Result<Self, ScoreboardError> {
        validate_persistent_scoreboard(&state)?;
        Ok(Self {
            state: SyncRwLock::new(state),
            revision: AtomicU64::new(0),
            saved_revision: AtomicU64::new(0),
            tracked: SyncRwLock::new(BTreeSet::new()),
            pending: SyncMutex::new(Vec::new()),
        })
    }

    /// Adds a writable `dummy` objective with vanilla's defaults.
    ///
    /// # Errors
    ///
    /// Returns an error if the objective name is empty or already exists.
    pub fn add_objective(
        &self,
        name: impl Into<String>,
    ) -> Result<ScoreboardObjective, ScoreboardError> {
        let name = name.into();
        self.add_objective_full(
            name.clone(),
            ObjectiveCriteria::dummy(),
            TextComponent::plain(name),
            RenderType::Integer,
            false,
            None,
        )
    }

    /// Vanilla `Scoreboard.addObjective`.
    ///
    /// # Errors
    ///
    /// Returns an error if the objective name is empty or already exists.
    pub fn add_objective_full(
        &self,
        name: impl Into<String>,
        criteria: ObjectiveCriteria,
        display_name: TextComponent,
        render_type: RenderType,
        display_auto_update: bool,
        number_format: Option<NumberFormat>,
    ) -> Result<ScoreboardObjective, ScoreboardError> {
        let name = name.into();
        ensure_objective_name(&name)?;
        let state = ObjectiveState {
            criteria,
            display_name,
            render_type,
            display_auto_update,
            number_format,
        };
        let mut persistent = self.state.write();
        if persistent.objectives.contains_key(&name) {
            return Err(ScoreboardError::DuplicateObjective(name));
        }
        let objective = objective_snapshot(&name, &state);
        persistent.objectives.insert(name, state);
        drop(persistent);
        self.mark_dirty();
        // Vanilla `ServerScoreboard.onObjectiveAdded` only marks the scoreboard dirty: the
        // objective reaches clients when `setDisplayObjective` starts tracking it.
        Ok(objective)
    }

    /// Vanilla `Scoreboard.removeObjective`.
    ///
    /// # Errors
    ///
    /// Returns an error if the objective does not exist.
    pub fn remove_objective(&self, name: &str) -> Result<(), ScoreboardError> {
        let mut persistent = self.state.write();
        if persistent.objectives.remove(name).is_none() {
            return Err(ScoreboardError::MissingObjective(name.to_owned()));
        }
        for scores in persistent.scores.values_mut() {
            scores.remove(name);
        }
        persistent.scores.retain(|_, scores| !scores.is_empty());
        // Vanilla clears every slot showing the objective through `setDisplayObjective`:
        // every slot but the last one still leaves the objective displayed elsewhere, so it
        // broadcasts that slot's clear; the last one stops tracking it.
        let mut packets = Vec::new();
        for slot in DisplaySlot::ALL {
            if persistent.display_slots.get(&slot).map(String::as_str) != Some(name) {
                continue;
            }
            persistent.display_slots.remove(&slot);
            if persistent
                .display_slots
                .values()
                .any(|objective| objective == name)
            {
                packets.push(ScoreboardPacket::display_objective(slot, None));
            } else {
                packets.push(ScoreboardPacket::objective_remove(name));
            }
        }
        drop(persistent);
        self.tracked.write().remove(name);
        for packet in packets {
            self.queue(packet);
        }
        self.mark_dirty();
        Ok(())
    }

    /// Returns an objective by name.
    #[must_use]
    pub fn objective(&self, name: &str) -> Option<ScoreboardObjective> {
        self.state
            .read()
            .objectives
            .get(name)
            .map(|state| objective_snapshot(name, state))
    }

    /// Returns objective names in stable order.
    #[must_use]
    pub fn objective_names(&self) -> Vec<String> {
        self.state.read().objectives.keys().cloned().collect()
    }

    /// Vanilla `Objective.setDisplayName`.
    ///
    /// # Errors
    ///
    /// Returns an error if the objective does not exist.
    pub fn set_objective_display_name(
        &self,
        name: &str,
        display_name: TextComponent,
    ) -> Result<(), ScoreboardError> {
        self.mutate_objective(name, |state| state.display_name = display_name)
    }

    /// Vanilla `Objective.setRenderType`.
    ///
    /// # Errors
    ///
    /// Returns an error if the objective does not exist.
    pub fn set_objective_render_type(
        &self,
        name: &str,
        render_type: RenderType,
    ) -> Result<(), ScoreboardError> {
        self.mutate_objective(name, |state| state.render_type = render_type)
    }

    /// Vanilla `Objective.setDisplayAutoUpdate`.
    ///
    /// # Errors
    ///
    /// Returns an error if the objective does not exist.
    pub fn set_objective_display_auto_update(
        &self,
        name: &str,
        display_auto_update: bool,
    ) -> Result<(), ScoreboardError> {
        self.mutate_objective(name, |state| {
            state.display_auto_update = display_auto_update;
        })
    }

    /// Vanilla `Objective.setNumberFormat`.
    ///
    /// # Errors
    ///
    /// Returns an error if the objective does not exist.
    pub fn set_objective_number_format(
        &self,
        name: &str,
        number_format: Option<NumberFormat>,
    ) -> Result<(), ScoreboardError> {
        self.mutate_objective(name, |state| state.number_format = number_format)
    }

    /// Returns the objective shown in a display slot.
    #[must_use]
    pub fn display_objective(&self, slot: DisplaySlot) -> Option<String> {
        self.state.read().display_slots.get(&slot).cloned()
    }

    /// Vanilla `Scoreboard.setDisplayObjective` + `ServerScoreboard.setDisplayObjective`.
    ///
    /// The slot contents change first. When the replaced objective no longer shows in any
    /// slot it stops being tracked, which sends `ClientboundSetObjectivePacket(objective, 1)`;
    /// while it is still shown elsewhere, vanilla broadcasts the new slot contents. A new
    /// objective that no client tracks yet is started with the full
    /// [`Self::get_start_tracking_packets`] bundle, because a client that never saw the
    /// objective would otherwise clear the slot when it cannot resolve the name.
    ///
    /// # Errors
    ///
    /// Returns an error if a non-`None` objective does not exist.
    pub fn set_display_objective(
        &self,
        slot: DisplaySlot,
        objective: Option<&ScoreboardObjective>,
    ) -> Result<(), ScoreboardError> {
        let mut persistent = self.state.write();
        let new_name = match objective {
            Some(objective) => {
                if !persistent.objectives.contains_key(objective.name()) {
                    return Err(ScoreboardError::MissingObjective(
                        objective.name().to_owned(),
                    ));
                }
                Some(objective.name().to_owned())
            }
            None => None,
        };
        let old_name = match &new_name {
            Some(name) => persistent.display_slots.insert(slot, name.clone()),
            None => persistent.display_slots.remove(&slot),
        };

        let mut tracked = self.tracked.write();
        let mut packets = Vec::new();
        if old_name != new_name
            && let Some(old) = old_name.as_deref()
        {
            if persistent
                .display_slots
                .values()
                .any(|objective| objective == old)
            {
                packets.push(ScoreboardPacket::display_objective(
                    slot,
                    new_name.as_deref(),
                ));
            } else {
                tracked.remove(old);
                packets.push(ScoreboardPacket::objective_remove(old));
            }
        }
        if let Some(new) = new_name.as_deref() {
            if tracked.contains(new) {
                packets.push(ScoreboardPacket::display_objective(slot, Some(new)));
            } else {
                tracked.insert(new.to_owned());
                packets.extend(start_tracking_packets(&persistent, new));
            }
        }
        drop(tracked);
        drop(persistent);

        for packet in packets {
            self.queue(packet);
        }
        self.mark_dirty();
        Ok(())
    }

    /// Adds a team with vanilla's default options.
    ///
    /// # Errors
    ///
    /// Returns an error if the team name is empty or already exists.
    pub fn add_team(&self, name: impl Into<String>) -> Result<PlayerTeam, ScoreboardError> {
        let name = name.into();
        ensure_team_name(&name)?;
        let mut persistent = self.state.write();
        if persistent.teams.contains_key(&name) {
            return Err(ScoreboardError::DuplicateTeam(name));
        }
        let team = PlayerTeam::new(name.clone());
        persistent.teams.insert(name.clone(), team.clone());
        drop(persistent);
        self.mark_dirty();
        // Vanilla `ServerScoreboard.onTeamAdded`.
        self.queue(ScoreboardPacket::team_add(&name, &team));
        Ok(team)
    }

    /// Vanilla `Scoreboard.removePlayerTeam`.
    ///
    /// # Errors
    ///
    /// Returns an error if the team does not exist.
    pub fn remove_team(&self, name: &str) -> Result<(), ScoreboardError> {
        let mut persistent = self.state.write();
        if persistent.teams.remove(name).is_none() {
            return Err(ScoreboardError::MissingTeam(name.to_owned()));
        }
        persistent.holder_teams.retain(|_, team| team != name);
        drop(persistent);
        self.mark_dirty();
        // Vanilla `ServerScoreboard.onTeamRemoved`.
        self.queue(ScoreboardPacket::team_remove(name));
        Ok(())
    }

    /// Vanilla `PlayerTeam.setDisplayName`.
    ///
    /// # Errors
    ///
    /// Returns an error if the team does not exist.
    pub fn set_team_display_name(
        &self,
        name: &str,
        display_name: TextComponent,
    ) -> Result<(), ScoreboardError> {
        self.mutate_team(name, |team| team.set_display_name(display_name))
    }

    /// Vanilla `PlayerTeam.setPlayerPrefix`.
    ///
    /// # Errors
    ///
    /// Returns an error if the team does not exist.
    pub fn set_team_prefix(
        &self,
        name: &str,
        prefix: TextComponent,
    ) -> Result<(), ScoreboardError> {
        self.mutate_team(name, |team| team.set_prefix(prefix))
    }

    /// Vanilla `PlayerTeam.setPlayerSuffix`.
    ///
    /// # Errors
    ///
    /// Returns an error if the team does not exist.
    pub fn set_team_suffix(
        &self,
        name: &str,
        suffix: TextComponent,
    ) -> Result<(), ScoreboardError> {
        self.mutate_team(name, |team| team.set_suffix(suffix))
    }

    /// Vanilla `PlayerTeam.setColor`; `None` is the command's `reset` form.
    ///
    /// # Errors
    ///
    /// Returns an error if the team does not exist.
    pub fn set_team_color(
        &self,
        name: &str,
        color: Option<TeamColor>,
    ) -> Result<(), ScoreboardError> {
        self.mutate_team(name, |team| team.set_color(color))
    }

    /// Vanilla `PlayerTeam.setAllowFriendlyFire`.
    ///
    /// # Errors
    ///
    /// Returns an error if the team does not exist.
    pub fn set_team_friendly_fire(&self, name: &str, allowed: bool) -> Result<(), ScoreboardError> {
        self.mutate_team(name, |team| team.set_allow_friendly_fire(allowed))
    }

    /// Vanilla `PlayerTeam.setSeeFriendlyInvisibles`.
    ///
    /// # Errors
    ///
    /// Returns an error if the team does not exist.
    pub fn set_team_see_friendly_invisibles(
        &self,
        name: &str,
        allowed: bool,
    ) -> Result<(), ScoreboardError> {
        self.mutate_team(name, |team| team.set_see_friendly_invisibles(allowed))
    }

    /// Vanilla `PlayerTeam.setNameTagVisibility`.
    ///
    /// # Errors
    ///
    /// Returns an error if the team does not exist.
    pub fn set_team_nametag_visibility(
        &self,
        name: &str,
        visibility: TeamVisibility,
    ) -> Result<(), ScoreboardError> {
        self.mutate_team(name, |team| team.set_name_tag_visibility(visibility))
    }

    /// Vanilla `PlayerTeam.setDeathMessageVisibility`.
    ///
    /// # Errors
    ///
    /// Returns an error if the team does not exist.
    pub fn set_team_death_message_visibility(
        &self,
        name: &str,
        visibility: TeamVisibility,
    ) -> Result<(), ScoreboardError> {
        self.mutate_team(name, |team| team.set_death_message_visibility(visibility))
    }

    /// Vanilla `PlayerTeam.setCollisionRule`.
    ///
    /// # Errors
    ///
    /// Returns an error if the team does not exist.
    pub fn set_team_collision_rule(
        &self,
        name: &str,
        rule: CollisionRule,
    ) -> Result<(), ScoreboardError> {
        self.mutate_team(name, |team| team.set_collision_rule(rule))
    }

    /// Returns a team's options.
    #[must_use]
    pub fn team(&self, name: &str) -> Option<PlayerTeam> {
        self.state.read().teams.get(name).cloned()
    }

    /// Returns team names in stable order.
    #[must_use]
    pub fn team_names(&self) -> Vec<String> {
        self.state.read().teams.keys().cloned().collect()
    }

    /// Returns the holders on a team in stable order.
    #[must_use]
    pub fn team_holders(&self, name: &str) -> Vec<String> {
        self.state
            .read()
            .teams
            .get(name)
            .map_or_else(Vec::new, |team| team.players().iter().cloned().collect())
    }

    /// Vanilla `PlayerTeam.getFormattedDisplayName`.
    #[must_use]
    pub fn team_display_name(&self, name: &str) -> Option<TextComponent> {
        self.state
            .read()
            .teams
            .get(name)
            .map(PlayerTeam::formatted_display_name)
    }

    /// Vanilla `Team.formatNameForTeam` for a holder's current team.
    #[must_use]
    pub fn formatted_holder_name(&self, holder: &ScoreHolder) -> TextComponent {
        let persistent = self.state.read();
        let team = persistent
            .holder_teams
            .get(holder.name())
            .and_then(|name| persistent.teams.get(name));
        format_name_for_team(
            team,
            holder
                .display_name()
                .cloned()
                .unwrap_or_else(|| TextComponent::plain(holder.name().to_owned())),
        )
    }

    /// Returns the current team name for a score holder.
    #[must_use]
    pub fn holder_team_name(&self, holder: &ScoreHolder) -> Option<String> {
        self.state.read().holder_teams.get(holder.name()).cloned()
    }

    /// Vanilla `Team.addPlayer`, replacing any prior membership.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty holder or a team that does not exist.
    pub fn add_holder_to_team(
        &self,
        holder: &ScoreHolder,
        team: &PlayerTeam,
    ) -> Result<(), ScoreboardError> {
        ensure_holder_name(holder.name())?;
        let mut persistent = self.state.write();
        if !persistent.teams.contains_key(team.name()) {
            return Err(ScoreboardError::MissingTeam(team.name().to_owned()));
        }
        let previous = persistent
            .holder_teams
            .insert(holder.name().to_owned(), team.name().to_owned());
        if previous.as_deref() == Some(team.name()) {
            return Ok(());
        }
        if let Some(previous) = &previous
            && let Some(previous_team) = persistent.teams.get_mut(previous)
        {
            previous_team.remove_player(holder.name());
        }
        if let Some(entry) = persistent.teams.get_mut(team.name()) {
            entry.add_player(holder.name().to_owned());
        }
        let name = team.name().to_owned();
        drop(persistent);
        self.mark_dirty();
        // Vanilla `Scoreboard.addPlayerToTeam`: moving between teams sends the old team's
        // remove before the new team's add (`ServerScoreboard.removePlayerFromTeam` and
        // `ServerScoreboard.addPlayerToTeam`).
        if let Some(previous) = previous {
            self.queue(ScoreboardPacket::team_player_remove(
                &previous,
                holder.name(),
            ));
        }
        self.queue(ScoreboardPacket::team_player_add(&name, holder.name()));
        Ok(())
    }

    /// Vanilla `Team.removePlayer`.
    ///
    /// # Errors
    ///
    /// Returns an error if the team does not exist or the holder is not on it.
    pub fn remove_holder_from_team(
        &self,
        holder: &ScoreHolder,
        team: &PlayerTeam,
    ) -> Result<(), ScoreboardError> {
        let mut persistent = self.state.write();
        let Some(entry) = persistent.teams.get_mut(team.name()) else {
            return Err(ScoreboardError::MissingTeam(team.name().to_owned()));
        };
        if !entry.remove_player(holder.name()) {
            return Err(ScoreboardError::HolderNotInTeam(
                holder.name().to_owned(),
                team.name().to_owned(),
            ));
        }
        persistent.holder_teams.remove(holder.name());
        let name = team.name().to_owned();
        drop(persistent);
        self.mark_dirty();
        // Vanilla `ServerScoreboard.removePlayerFromTeam`.
        self.queue(ScoreboardPacket::team_player_remove(&name, holder.name()));
        Ok(())
    }

    /// Returns tracked score holders in stable order.
    #[must_use]
    pub fn tracked_holders(&self) -> Vec<ScoreHolder> {
        self.state
            .read()
            .scores
            .keys()
            .map(|name| ScoreHolder::new(name.to_owned()))
            .collect()
    }

    /// Returns the complete score entry for a holder and objective.
    #[must_use]
    pub fn score_entry(
        &self,
        holder: &ScoreHolder,
        objective: &ScoreboardObjective,
    ) -> Option<ScoreInfo> {
        self.state
            .read()
            .scores
            .get(holder.name())
            .and_then(|scores| scores.get(objective.name()))
            .map(ScoreState::to_info)
    }

    /// Returns the integer score for a holder and objective.
    #[must_use]
    pub fn score(&self, holder: &ScoreHolder, objective: &ScoreboardObjective) -> Option<i32> {
        self.score_entry(holder, objective).map(|info| info.value)
    }

    /// Returns every objective a holder has a score in, in stable order.
    #[must_use]
    pub fn list_player_scores(&self, holder: &ScoreHolder) -> Vec<(String, ScoreInfo)> {
        self.state
            .read()
            .scores
            .get(holder.name())
            .map_or_else(Vec::new, |scores| {
                scores
                    .iter()
                    .map(|(objective, state)| (objective.clone(), state.to_info()))
                    .collect()
            })
    }

    /// Vanilla `Scoreboard.getOrCreatePlayerScore`.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty holder, a missing objective, or a read-only
    /// objective when `force_writable` is `false`.
    pub fn get_or_create_score(
        &self,
        holder: &ScoreHolder,
        objective: &ScoreboardObjective,
        force_writable: bool,
    ) -> Result<i32, ScoreboardError> {
        ensure_holder_name(holder.name())?;
        let mut persistent = self.state.write();
        let state = ensure_objective_exists(&persistent, objective)?;
        if !force_writable && state.criteria.is_read_only() {
            return Err(ScoreboardError::ReadOnlyObjective(
                objective.name().to_owned(),
            ));
        }
        let scores = persistent
            .scores
            .entry(holder.name().to_owned())
            .or_default();
        if !scores.contains_key(objective.name()) {
            scores.insert(objective.name().to_owned(), ScoreState::default());
        }
        Ok(scores[objective.name()].value)
    }

    /// Sets a holder's score, preserving its lock state when already present.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty holder, a missing objective, or a read-only objective.
    pub fn set_score(
        &self,
        holder: &ScoreHolder,
        objective: &ScoreboardObjective,
        value: i32,
    ) -> Result<(), ScoreboardError> {
        self.set_score_inner(holder, objective, value, false)
    }

    /// Sets a holder's score even when the objective's criteria are read-only.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty holder or a missing objective.
    pub fn set_score_forced(
        &self,
        holder: &ScoreHolder,
        objective: &ScoreboardObjective,
        value: i32,
    ) -> Result<(), ScoreboardError> {
        self.set_score_inner(holder, objective, value, true)
    }

    fn set_score_inner(
        &self,
        holder: &ScoreHolder,
        objective: &ScoreboardObjective,
        value: i32,
        force_writable: bool,
    ) -> Result<(), ScoreboardError> {
        ensure_holder_name(holder.name())?;
        let mut persistent = self.state.write();
        let state = ensure_objective_exists(&persistent, objective)?;
        if !force_writable && state.criteria.is_read_only() {
            return Err(ScoreboardError::ReadOnlyObjective(
                objective.name().to_owned(),
            ));
        }
        let scores = persistent
            .scores
            .entry(holder.name().to_owned())
            .or_default();
        match scores.entry(objective.name().to_owned()) {
            Entry::Vacant(entry) => {
                entry.insert(ScoreState {
                    value,
                    ..ScoreState::default()
                });
            }
            Entry::Occupied(mut entry) => {
                if entry.get().value == value {
                    return Ok(());
                }
                entry.get_mut().value = value;
            }
        }
        drop(persistent);
        self.mark_dirty();
        self.queue_score_change(holder.name(), objective.name());
        Ok(())
    }

    /// Vanilla `ScoreAccess.lock` / `unlock`.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty holder or a missing objective.
    pub fn set_score_locked(
        &self,
        holder: &ScoreHolder,
        objective: &ScoreboardObjective,
        locked: bool,
    ) -> Result<(), ScoreboardError> {
        ensure_holder_name(holder.name())?;
        let mut persistent = self.state.write();
        ensure_objective_exists(&persistent, objective)?;
        let scores = persistent
            .scores
            .entry(holder.name().to_owned())
            .or_default();
        match scores.entry(objective.name().to_owned()) {
            Entry::Vacant(entry) => {
                entry.insert(ScoreState {
                    locked,
                    ..ScoreState::default()
                });
            }
            Entry::Occupied(mut entry) => {
                if entry.get().locked == locked {
                    return Ok(());
                }
                entry.get_mut().locked = locked;
            }
        }
        drop(persistent);
        self.mark_dirty();
        Ok(())
    }

    /// Vanilla `ScoreAccess.display`.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty holder or a missing objective.
    pub fn set_score_display(
        &self,
        holder: &ScoreHolder,
        objective: &ScoreboardObjective,
        display: Option<TextComponent>,
    ) -> Result<(), ScoreboardError> {
        self.mutate_score(holder, objective, |state| state.display = display)?;
        self.queue_score_change(holder.name(), objective.name());
        Ok(())
    }

    /// Vanilla `ScoreAccess.numberFormatOverride`.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty holder or a missing objective.
    pub fn set_score_number_format(
        &self,
        holder: &ScoreHolder,
        objective: &ScoreboardObjective,
        number_format: Option<NumberFormat>,
    ) -> Result<(), ScoreboardError> {
        self.mutate_score(holder, objective, |state| {
            state.number_format = number_format;
        })?;
        self.queue_score_change(holder.name(), objective.name());
        Ok(())
    }

    /// Vanilla `Scoreboard.resetSinglePlayerScore` / `resetAllPlayerScores`.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty holder or a missing objective.
    pub fn reset_scores(
        &self,
        holder: &ScoreHolder,
        objective: Option<&ScoreboardObjective>,
    ) -> Result<(), ScoreboardError> {
        ensure_holder_name(holder.name())?;
        let mut persistent = self.state.write();
        let mut packets = Vec::new();
        if let Some(objective) = objective {
            ensure_objective_exists(&persistent, objective)?;
            // Vanilla `Scoreboard.resetSinglePlayerScore`: an emptied holder goes through
            // `onPlayerRemoved`, an objective that still has scores through
            // `onPlayerScoreRemoved`, and a reset that removed nothing broadcasts nothing.
            if let Some(scores) = persistent.scores.get_mut(holder.name()) {
                let removed = scores.remove(objective.name()).is_some();
                if scores.is_empty() {
                    persistent.scores.remove(holder.name());
                    if !self.tracked.read().is_empty() {
                        packets.push(ScoreboardPacket::all_scores_reset(holder.name()));
                    }
                } else if removed && self.tracked.read().contains(objective.name()) {
                    packets.push(ScoreboardPacket::score_reset(
                        holder.name(),
                        objective.name(),
                    ));
                }
            }
        } else {
            // Vanilla `Scoreboard.resetAllPlayerScores` -> `onPlayerRemoved`, which only
            // fires when the holder actually had scores.
            if persistent.scores.remove(holder.name()).is_some() && !self.tracked.read().is_empty()
            {
                packets.push(ScoreboardPacket::all_scores_reset(holder.name()));
            }
        }
        drop(persistent);
        self.mark_dirty();
        for packet in packets {
            self.queue(packet);
        }
        Ok(())
    }

    /// Vanilla `Scoreboard.entityRemoved` / `resetAllPlayerScores` for a whole holder,
    /// including team membership.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty holder.
    pub fn reset_holder(&self, holder: &ScoreHolder) -> Result<(), ScoreboardError> {
        ensure_holder_name(holder.name())?;
        let mut persistent = self.state.write();
        let removed_scores = persistent.scores.remove(holder.name()).is_some();
        let team = persistent.holder_teams.remove(holder.name());
        if let Some(team) = &team
            && let Some(entry) = persistent.teams.get_mut(team)
        {
            entry.remove_player(holder.name());
        }
        let tracked_empty = self.tracked.read().is_empty();
        drop(persistent);
        if removed_scores || team.is_some() {
            self.mark_dirty();
        }
        // Vanilla `resetAllPlayerScores` only calls `onPlayerRemoved` when the holder had
        // scores, and `Scoreboard.removePlayerFromTeam` only broadcasts when it was on a team.
        if removed_scores && !tracked_empty {
            self.queue(ScoreboardPacket::all_scores_reset(holder.name()));
        }
        if let Some(team) = team {
            self.queue(ScoreboardPacket::team_player_remove(&team, holder.name()));
        }
        Ok(())
    }

    /// Returns objective names that have a score for the holder.
    #[must_use]
    pub fn holder_objectives(&self, holder: &ScoreHolder) -> BTreeSet<String> {
        self.state
            .read()
            .scores
            .get(holder.name())
            .map_or_else(BTreeSet::new, |scores| scores.keys().cloned().collect())
    }

    /// Vanilla `ServerScoreboard.forAllObjectives`: applies `value` to every
    /// objective using `criteria` and held by `holder`, ignoring read-only flags.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty holder.
    pub fn update_stat(
        &self,
        holder: &ScoreHolder,
        criteria: &ObjectiveCriteria,
        value: i32,
    ) -> Result<(), ScoreboardError> {
        self.for_all_objectives(holder, criteria, |current| value - current)
    }

    /// Vanilla `ServerScoreboard.addToAllObjectives`: adds `delta` to every
    /// objective using `criteria` and held by `holder`.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty holder.
    pub fn add_to_stat(
        &self,
        holder: &ScoreHolder,
        criteria: &ObjectiveCriteria,
        delta: i32,
    ) -> Result<(), ScoreboardError> {
        self.for_all_objectives(holder, criteria, |_| delta)
    }

    fn for_all_objectives(
        &self,
        holder: &ScoreHolder,
        criteria: &ObjectiveCriteria,
        delta: impl Fn(i32) -> i32,
    ) -> Result<(), ScoreboardError> {
        ensure_holder_name(holder.name())?;
        let matches = self
            .state
            .read()
            .objectives
            .iter()
            .filter(|(_, state)| &state.criteria == criteria)
            .map(|(name, state)| objective_snapshot(name, state))
            .collect::<Vec<_>>();
        for objective in matches {
            let current = self.score(holder, &objective).unwrap_or_default();
            let next = current.saturating_add(delta(current));
            if self.score(holder, &objective) != Some(next) {
                self.set_score_forced(holder, &objective, next)?;
            }
        }
        Ok(())
    }

    fn mutate_objective(
        &self,
        name: &str,
        mutate: impl FnOnce(&mut ObjectiveState),
    ) -> Result<(), ScoreboardError> {
        let mut persistent = self.state.write();
        let Some(state) = persistent.objectives.get_mut(name) else {
            return Err(ScoreboardError::MissingObjective(name.to_owned()));
        };
        mutate(state);
        let snapshot = objective_snapshot(name, state);
        drop(persistent);
        self.mark_dirty();
        // Vanilla `ServerScoreboard.onObjectiveChanged` only broadcasts while the
        // objective is tracked by at least one display slot.
        if self.tracked.read().contains(name) {
            self.queue(ScoreboardPacket::objective_change(name, snapshot));
        }
        Ok(())
    }

    fn mutate_team(
        &self,
        name: &str,
        mutate: impl FnOnce(&mut PlayerTeam),
    ) -> Result<(), ScoreboardError> {
        let mut persistent = self.state.write();
        let Some(team) = persistent.teams.get_mut(name) else {
            return Err(ScoreboardError::MissingTeam(name.to_owned()));
        };
        mutate(team);
        let snapshot = team.clone();
        drop(persistent);
        self.mark_dirty();
        // Vanilla `ServerScoreboard.onTeamChanged` broadcasts unconditionally.
        self.queue(ScoreboardPacket::team_change(name, &snapshot));
        Ok(())
    }

    fn mutate_score(
        &self,
        holder: &ScoreHolder,
        objective: &ScoreboardObjective,
        mutate: impl FnOnce(&mut ScoreState),
    ) -> Result<(), ScoreboardError> {
        ensure_holder_name(holder.name())?;
        let mut persistent = self.state.write();
        ensure_objective_exists(&persistent, objective)?;
        let scores = persistent
            .scores
            .entry(holder.name().to_owned())
            .or_default();
        let state = scores.entry(objective.name().to_owned()).or_default();
        mutate(state);
        drop(persistent);
        self.mark_dirty();
        Ok(())
    }

    /// Vanilla `ServerScoreboard.onScoreChanged`: a score broadcast only happens while the
    /// objective is tracked by a display slot. Vanilla `broadcast` is reached from
    /// `ScoreAccess.set`/`display`/`numberFormatOverride` with the authoritative value
    /// already written, so the packet is built from the stored score here.
    fn queue_score_change(&self, owner: &str, objective: &str) {
        let tracked = self.tracked.read().contains(objective);
        if !tracked {
            return;
        }
        let persistent = self.state.read();
        let Some(score) = persistent
            .scores
            .get(owner)
            .and_then(|scores| scores.get(objective))
        else {
            return;
        };
        let packet = ScoreboardPacket::score_changed(owner, objective, score);
        drop(persistent);
        self.queue(packet);
    }

    fn queue(&self, packet: ScoreboardPacket) {
        self.pending.lock().push(packet);
    }

    /// Takes the client packets queued since the last flush, in mutation order.
    #[must_use]
    pub fn pending_packets(&self) -> Vec<ScoreboardPacket> {
        take(&mut *self.pending.lock())
    }

    /// Builds the vanilla `ServerScoreboard.getStartTrackingPackets` for one objective.
    ///
    /// The objective is created first, then every display slot currently showing it,
    /// then every score recorded in it.
    #[must_use]
    pub fn get_start_tracking_packets(&self, objective: &str) -> Vec<ScoreboardPacket> {
        start_tracking_packets(&self.state.read(), objective)
    }

    /// Vanilla `PlayerList.updateEntireScoreboard`, the state a joining player needs.
    ///
    /// Every team is created first, then each objective currently in a display slot is
    /// started exactly once through [`Self::get_start_tracking_packets`], in
    /// `DisplaySlot` order.
    #[must_use]
    pub fn initial_sync_packets(&self) -> Vec<ScoreboardPacket> {
        let persistent = self.state.read();
        let mut packets = Vec::new();
        for (name, team) in &persistent.teams {
            packets.push(ScoreboardPacket::team_add(name, team));
        }
        let mut started: BTreeSet<&str> = BTreeSet::new();
        for slot in DisplaySlot::ALL {
            let Some(objective) = persistent.display_slots.get(&slot) else {
                continue;
            };
            if started.insert(objective.as_str()) {
                packets.extend(start_tracking_packets(&persistent, objective));
            }
        }
        packets
    }

    fn mark_dirty(&self) {
        self.revision.fetch_add(1, Ordering::Release);
    }

    fn pending_save(&self) -> Option<ScoreboardSaveSnapshot> {
        let state = self.state.read();
        let revision = self.revision.load(Ordering::Acquire);
        if revision == self.saved_revision.load(Ordering::Acquire) {
            return None;
        }
        Some(ScoreboardSaveSnapshot {
            revision,
            state: state.clone(),
        })
    }

    fn mark_saved(&self, revision: u64) {
        self.saved_revision.fetch_max(revision, Ordering::Release);
    }
}

impl Default for Scoreboard {
    fn default() -> Self {
        Self::new()
    }
}

impl ScoreState {
    fn to_info(&self) -> ScoreInfo {
        ScoreInfo {
            value: self.value,
            locked: self.locked,
            extras: ScoreExtras {
                display: self.display.clone(),
                number_format: self.number_format.clone(),
            },
        }
    }
}

fn start_tracking_packets(state: &PersistentScoreboard, objective: &str) -> Vec<ScoreboardPacket> {
    let Some(objective_state) = state.objectives.get(objective) else {
        return Vec::new();
    };
    let mut packets = vec![ScoreboardPacket::objective_add(
        objective,
        objective_snapshot(objective, objective_state),
    )];
    for slot in DisplaySlot::ALL {
        if state.display_slots.get(&slot).map(String::as_str) == Some(objective) {
            packets.push(ScoreboardPacket::display_objective(slot, Some(objective)));
        }
    }
    for (owner, scores) in &state.scores {
        if let Some(score) = scores.get(objective) {
            packets.push(ScoreboardPacket::score_changed(owner, objective, score));
        }
    }
    packets
}

fn objective_snapshot(name: &str, state: &ObjectiveState) -> ScoreboardObjective {
    ScoreboardObjective {
        name: name.to_owned(),
        criteria: state.criteria.clone(),
        display_name: state.display_name.clone(),
        render_type: state.render_type,
        display_auto_update: state.display_auto_update,
        number_format: state.number_format.clone(),
    }
}

fn validate_persistent_scoreboard(state: &PersistentScoreboard) -> Result<(), ScoreboardError> {
    for name in state.objectives.keys() {
        ensure_objective_name(name)?;
    }
    for name in state.teams.keys() {
        ensure_team_name(name)?;
    }
    for (holder, scores) in &state.scores {
        ensure_holder_name(holder)?;
        for objective in scores.keys() {
            if !state.objectives.contains_key(objective) {
                return Err(ScoreboardError::MissingObjective(objective.clone()));
            }
        }
    }
    for (holder, team) in &state.holder_teams {
        ensure_holder_name(holder)?;
        if !state.teams.contains_key(team) {
            return Err(ScoreboardError::MissingTeam(team.clone()));
        }
    }
    for objective in state.display_slots.values() {
        if !state.objectives.contains_key(objective) {
            return Err(ScoreboardError::MissingObjective(objective.clone()));
        }
    }
    Ok(())
}

const fn ensure_objective_name(name: &str) -> Result<(), ScoreboardError> {
    if name.is_empty() {
        Err(ScoreboardError::EmptyObjectiveName)
    } else {
        Ok(())
    }
}

const fn ensure_team_name(name: &str) -> Result<(), ScoreboardError> {
    if name.is_empty() {
        Err(ScoreboardError::EmptyTeamName)
    } else {
        Ok(())
    }
}

const fn ensure_holder_name(name: &str) -> Result<(), ScoreboardError> {
    if name.is_empty() {
        Err(ScoreboardError::EmptyScoreHolderName)
    } else {
        Ok(())
    }
}

fn ensure_objective_exists<'a>(
    state: &'a PersistentScoreboard,
    objective: &ScoreboardObjective,
) -> Result<&'a ObjectiveState, ScoreboardError> {
    state
        .objectives
        .get(objective.name())
        .ok_or_else(|| ScoreboardError::MissingObjective(objective.name().to_owned()))
}

/// Loaded scoreboards keyed by Steel domain.
pub struct DomainScoreboards {
    scoreboards: BTreeMap<String, Scoreboard>,
    save_lock: AsyncMutex<()>,
}

impl DomainScoreboards {
    /// Loads one scoreboard through each domain's default world saved-data boundary.
    pub async fn load(worlds: &WorldMap) -> io::Result<Self> {
        let mut domains = worlds.domain_names().collect::<Vec<_>>();
        domains.sort_unstable();
        let mut scoreboards = BTreeMap::new();
        for domain in domains {
            let world = domain_default_world(worlds, domain)?;
            let persistent: PersistentScoreboard = world
                .saved_data
                .load_or_default(saved_data_names::SCOREBOARD)
                .await
                .map_err(|error| scoreboard_io_error(domain, error))?;
            let scoreboard = Scoreboard::from_persistent(persistent).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid scoreboard for domain '{domain}': {error}"),
                )
            })?;
            scoreboards.insert(domain.to_owned(), scoreboard);
        }
        Ok(Self {
            scoreboards,
            save_lock: AsyncMutex::new(()),
        })
    }

    /// Returns the scoreboard for a domain.
    #[must_use]
    pub fn get(&self, domain: &str) -> Option<&Scoreboard> {
        self.scoreboards.get(domain)
    }

    /// Saves every dirty domain scoreboard and returns the number written.
    pub async fn save(&self, worlds: &WorldMap) -> io::Result<usize> {
        let _save_guard = self.save_lock.lock().await;
        let mut saved = 0;
        for (domain, scoreboard) in &self.scoreboards {
            let Some(snapshot) = scoreboard.pending_save() else {
                continue;
            };
            let world = domain_default_world(worlds, domain)?;
            world
                .saved_data
                .save(saved_data_names::SCOREBOARD, &snapshot.state)
                .await
                .map_err(|error| scoreboard_io_error(domain, error))?;
            scoreboard.mark_saved(snapshot.revision);
            saved += 1;
        }
        Ok(saved)
    }

    /// Flushes queued scoreboard presentation updates to every player in each
    /// domain, mirroring vanilla `ServerScoreboard`'s broadcast surface.
    pub fn flush_updates(&self, worlds: &WorldMap) -> usize {
        let mut sent = 0;
        for (domain, scoreboard) in &self.scoreboards {
            let packets = scoreboard.pending_packets();
            if packets.is_empty() {
                continue;
            }
            for world in worlds.worlds_in_domain(domain) {
                world.players.iter_players(|_, player| {
                    for packet in &packets {
                        packet.send_to(player);
                    }
                    true
                });
            }
            sent += packets.len();
        }
        sent
    }

    /// Sends the scoreboard state to one joining player, mirroring vanilla
    /// `PlayerList.updateEntireScoreboard`.
    pub fn send_initial_state(&self, domain: &str, player: &Player) -> usize {
        let Some(scoreboard) = self.scoreboards.get(domain) else {
            return 0;
        };
        let packets = scoreboard.initial_sync_packets();
        for packet in &packets {
            packet.send_to(player);
        }
        packets.len()
    }
}

/// Per-tick flush of the scoreboard presentation.
///
/// Vanilla `ServerScoreboard` broadcasts from inside each mutation; Steel records the same
/// operations and hands them to the owning domain's players once per tick, after commands and
/// joins have run, so a `/scoreboard` or `/team` burst reaches clients as one batch.
///
/// Vanilla equivalent: `ServerScoreboard`'s own `broadcastAll` calls, plus
/// `PlayerList.updateEntireScoreboard` for a joining player.
pub fn tick_scoreboards(server: &Server) {
    server.scoreboards.flush_updates(&server.worlds);
}

fn domain_default_world<'a>(worlds: &'a WorldMap, domain: &str) -> io::Result<&'a World> {
    worlds
        .default_world(domain)
        .map(AsRef::as_ref)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("domain '{domain}' has no loaded default world"),
            )
        })
}

fn scoreboard_io_error(domain: &str, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("scoreboard I/O failed for domain '{domain}': {error}"),
    )
}

#[cfg(test)]
mod tests {
    use std::{
        env::temp_dir,
        time::{SystemTime, UNIX_EPOCH},
    };
    use tokio::fs;

    use steel_protocol::packets::game::ScoreboardDisplaySlot;
    use steel_utils::saved_data::{SavedDataManager, names as saved_data_names};
    use text_components::TextComponent;

    use super::{
        AsyncMutex, CriteriaKind, DisplaySlot, DomainScoreboards, NumberFormat, ObjectiveCriteria,
        PersistentScoreboard, RenderType, ScoreHolder, Scoreboard, ScoreboardError,
        ScoreboardPacket, TeamColor, TeamVisibility,
    };

    /// Vanilla `NumberFormat.Fixed` over a plain contents component.
    fn fixed_number_format() -> NumberFormat {
        NumberFormat::Fixed(TextComponent::plain("many".to_owned()))
    }

    #[test]
    fn score_value_and_lock_state_are_independent() {
        let scoreboard = Scoreboard::new();
        let objective = scoreboard
            .add_objective("kills")
            .expect("objective should be added");
        let holder = ScoreHolder::new("Steve");

        scoreboard
            .set_score(&holder, &objective, 7)
            .expect("score should be writable");
        scoreboard
            .set_score_locked(&holder, &objective, false)
            .expect("score lock should change");
        scoreboard
            .set_score(&holder, &objective, 9)
            .expect("score should remain writable");

        let entry = scoreboard
            .score_entry(&holder, &objective)
            .expect("score should exist");
        assert_eq!(entry.value(), 9);
        assert!(!entry.is_locked());
    }

    #[test]
    fn read_only_objective_rejects_score_writes() {
        let scoreboard = Scoreboard::new();
        let objective = scoreboard
            .add_objective_full(
                "health",
                ObjectiveCriteria::parse("health").expect("health criteria should parse"),
                TextComponent::plain("Health".to_owned()),
                RenderType::Hearts,
                true,
                None,
            )
            .expect("objective should be added");

        assert_eq!(
            scoreboard.set_score(&ScoreHolder::new("Steve"), &objective, 20),
            Err(ScoreboardError::ReadOnlyObjective("health".to_owned()))
        );
        assert!(objective.is_read_only());
        assert!(objective.display_auto_update());
        assert_eq!(objective.render_type(), RenderType::Hearts);
    }

    #[test]
    fn team_assignment_replaces_prior_membership() {
        let scoreboard = Scoreboard::new();
        let red = scoreboard
            .add_team("red")
            .expect("red team should be added");
        let blue = scoreboard
            .add_team("blue")
            .expect("blue team should be added");
        let holder = ScoreHolder::new("Steve");

        scoreboard
            .add_holder_to_team(&holder, &red)
            .expect("holder should join red");
        scoreboard
            .add_holder_to_team(&holder, &blue)
            .expect("holder should move to blue");

        assert_eq!(
            scoreboard.holder_team_name(&holder).as_deref(),
            Some("blue")
        );
        assert_eq!(scoreboard.team_holders("red"), [] as [String; 0]);
        assert_eq!(scoreboard.team_holders("blue"), vec!["Steve".to_owned()]);
    }

    #[test]
    fn team_options_round_trip_and_format_members() {
        let scoreboard = Scoreboard::new();
        scoreboard.add_team("red").expect("team should be added");
        let holder = ScoreHolder::new("Steve");
        let red = scoreboard.team("red").expect("team should exist");
        scoreboard
            .add_holder_to_team(&holder, &red)
            .expect("holder should join");

        let mut team = red;
        team.set_color(Some(TeamColor::Red));
        team.set_name_tag_visibility(TeamVisibility::HideForOtherTeams);
        team.set_prefix(TextComponent::plain("[".to_owned()));

        // The handle is a snapshot; mutating it does not write back. Commands go
        // through the setters below instead.
        assert_eq!(scoreboard.team("red").expect("team").color(), None);

        let formatted = scoreboard.formatted_holder_name(&holder);
        // Vanilla `PlayerTeam.decorateName` always wraps the member as prefix + name +
        // suffix; the detached handle above left the registered team at its defaults.
        assert_eq!(formatted.children.len(), 3);
        assert_eq!(
            formatted.children[1],
            TextComponent::plain("Steve".to_owned())
        );
        let _ = team;
    }

    #[test]
    fn display_slots_track_objectives_and_clear_on_removal() {
        let scoreboard = Scoreboard::new();
        let objective = scoreboard
            .add_objective("kills")
            .expect("objective should be added");
        scoreboard
            .set_display_objective(DisplaySlot::Sidebar, Some(&objective))
            .expect("display slot should be set");
        assert_eq!(
            scoreboard.display_objective(DisplaySlot::Sidebar),
            Some("kills".to_owned())
        );

        scoreboard
            .remove_objective("kills")
            .expect("objective should be removed");
        assert_eq!(scoreboard.display_objective(DisplaySlot::Sidebar), None);
    }

    #[test]
    fn stat_criteria_update_every_matching_objective() {
        steel_registry::init_vanilla_registry();
        let scoreboard = Scoreboard::new();
        let criteria = ObjectiveCriteria::parse("minecraft.mined:minecraft.stone")
            .expect("stat criteria should parse");
        let objective = scoreboard
            .add_objective_full(
                "stone",
                criteria.clone(),
                TextComponent::new(),
                RenderType::Integer,
                false,
                None,
            )
            .expect("objective should be added");
        let holder = ScoreHolder::new("Steve");

        scoreboard
            .add_to_stat(&holder, &criteria, 3)
            .expect("stat should update");
        assert_eq!(scoreboard.score(&holder, &objective), Some(3));
        scoreboard
            .update_stat(&holder, &criteria, 2)
            .expect("stat should update");
        assert_eq!(scoreboard.score(&holder, &objective), Some(2));
    }

    #[test]
    fn trigger_scores_are_identified_by_criteria_kind() {
        let scoreboard = Scoreboard::new();
        let objective = scoreboard
            .add_objective_full(
                "wave",
                ObjectiveCriteria::trigger(),
                TextComponent::new(),
                RenderType::Integer,
                false,
                None,
            )
            .expect("objective should be added");
        assert_eq!(objective.criteria().kind(), CriteriaKind::Trigger);
    }

    #[test]
    fn hidden_holders_use_the_vanilla_prefix() {
        assert!(ScoreHolder::new("#hidden").is_hidden());
        assert!(!ScoreHolder::new("Steve").is_hidden());
    }

    #[test]
    fn start_tracking_packets_cover_the_objective_its_slots_and_its_scores() {
        let scoreboard = Scoreboard::new();
        let objective = scoreboard
            .add_objective("kills")
            .expect("objective should be added");
        let holder = ScoreHolder::new("Steve");
        scoreboard
            .set_score(&holder, &objective, 5)
            .expect("score should be set");
        scoreboard
            .set_display_objective(DisplaySlot::Sidebar, Some(&objective))
            .expect("display slot should be set");
        scoreboard.add_team("red").expect("team should be added");
        // A different objective's score must not leak into this objective's bundle.
        let other = scoreboard
            .add_objective("deaths")
            .expect("objective should be added");
        scoreboard
            .set_score(&holder, &other, 2)
            .expect("score should be set");

        // Vanilla `ServerScoreboard.getStartTrackingPackets` emits the objective
        // itself, then one display-slot packet per slot showing it, then its scores.
        assert_eq!(scoreboard.get_start_tracking_packets("kills").len(), 3);
        assert_eq!(scoreboard.get_start_tracking_packets("deaths").len(), 2);
        assert!(
            scoreboard.get_start_tracking_packets("missing").is_empty(),
            "an unknown objective starts no tracking"
        );
        // Teams, then the displayed objective's bundle: objective + its slot + its score.
        assert_eq!(scoreboard.initial_sync_packets().len(), 4);
    }

    #[test]
    fn objective_number_format_and_extras_round_trip() {
        let scoreboard = Scoreboard::new();
        let objective = scoreboard
            .add_objective_full(
                "kills",
                ObjectiveCriteria::dummy(),
                TextComponent::plain("Kills".to_owned()),
                RenderType::Integer,
                false,
                Some(NumberFormat::Blank),
            )
            .expect("objective should be added");
        assert!(matches!(
            objective.number_format(),
            Some(NumberFormat::Blank)
        ));

        let holder = ScoreHolder::new("Steve");
        scoreboard
            .set_score_display(
                &holder,
                &objective,
                Some(TextComponent::plain("deaths".to_owned())),
            )
            .expect("display should be set");
        let entry = scoreboard
            .score_entry(&holder, &objective)
            .expect("score should exist");
        assert_eq!(
            entry.display(),
            Some(&TextComponent::plain("deaths".to_owned()))
        );
    }

    #[tokio::test]
    async fn persisted_scoreboard_round_trips_and_becomes_clean_after_save() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after Unix epoch")
            .as_nanos();
        let path = temp_dir().join(format!("steel-scoreboard-{unique}"));
        let manager = SavedDataManager::new(Some(&path));
        let scoreboard = Scoreboard::new();
        let objective = scoreboard
            .add_objective("kills")
            .expect("objective should be added");
        let holder = ScoreHolder::new("Steve");
        scoreboard
            .set_score(&holder, &objective, 5)
            .expect("score should be writable");
        scoreboard.add_team("red").expect("team should be added");
        scoreboard
            .set_display_objective(DisplaySlot::Sidebar, Some(&objective))
            .expect("display slot should be set");

        let snapshot = scoreboard
            .pending_save()
            .expect("scoreboard should be dirty");
        manager
            .save(saved_data_names::SCOREBOARD, &snapshot.state)
            .await
            .expect("scoreboard should save");
        scoreboard.mark_saved(snapshot.revision);
        assert!(scoreboard.pending_save().is_none());

        let persistent: PersistentScoreboard = manager
            .load_or_default(saved_data_names::SCOREBOARD)
            .await
            .expect("scoreboard should load");
        let restored = Scoreboard::from_persistent(persistent).expect("scoreboard should validate");
        let restored_objective = restored
            .objective("kills")
            .expect("objective should persist");
        assert_eq!(restored.score(&holder, &restored_objective), Some(5));
        assert_eq!(restored.team_names(), vec!["red".to_owned()]);
        assert_eq!(
            restored.display_objective(DisplaySlot::Sidebar),
            Some("kills".to_owned())
        );

        fs::remove_dir_all(path)
            .await
            .expect("temporary scoreboard directory should be removed");
    }

    #[test]
    fn mutation_after_snapshot_remains_dirty_when_snapshot_is_marked_saved() {
        let scoreboard = Scoreboard::new();
        scoreboard
            .add_objective("kills")
            .expect("objective should be added");
        let snapshot = scoreboard
            .pending_save()
            .expect("scoreboard should be dirty");

        scoreboard
            .add_objective("deaths")
            .expect("second objective should be added");
        scoreboard.mark_saved(snapshot.revision);

        let pending = scoreboard
            .pending_save()
            .expect("newer mutation should remain dirty");
        assert!(pending.revision > snapshot.revision);
        assert!(pending.state.objectives.contains_key("deaths"));
    }

    #[test]
    fn domains_keep_independent_scoreboards() {
        let scoreboards = DomainScoreboards {
            scoreboards: [
                ("alpha".to_owned(), Scoreboard::new()),
                ("beta".to_owned(), Scoreboard::new()),
            ]
            .into_iter()
            .collect(),
            save_lock: AsyncMutex::new(()),
        };
        scoreboards
            .get("alpha")
            .expect("alpha scoreboard should exist")
            .add_objective("kills")
            .expect("alpha objective should be added");

        assert!(
            scoreboards
                .get("beta")
                .expect("beta scoreboard should exist")
                .objective("kills")
                .is_none()
        );
    }

    /// Vanilla `ServerScoreboard`: creating an objective only marks the scoreboard dirty, so
    /// nothing is queued until a display slot starts tracking it — and the queue drains once.
    #[test]
    fn untracked_objectives_queue_nothing_until_a_display_slot_shows_them() {
        let scoreboard = Scoreboard::new();
        let objective = scoreboard
            .add_objective("kills")
            .expect("objective should be added");
        assert!(scoreboard.pending_packets().is_empty());

        scoreboard
            .set_display_objective(DisplaySlot::Sidebar, Some(&objective))
            .expect("display slot should be set");
        let first = scoreboard.pending_packets();
        assert_eq!(
            first.len(),
            2,
            "start tracking sends the objective and the slot"
        );
        assert!(scoreboard.pending_packets().is_empty());
    }

    /// Vanilla `ServerScoreboard` broadcast surface: creating an objective, scoring,
    /// and showing it in the sidebar queues exactly the `ClientboundSetObjective`,
    /// `ClientboundSetScore`, and `ClientboundSetDisplayObjective` packets the next
    /// tick delivers to every client.
    #[test]
    fn sidebar_objective_mutations_reach_the_client_packets() {
        let scoreboard = Scoreboard::new();
        let objective = scoreboard
            .add_objective_full(
                "kills",
                ObjectiveCriteria::dummy(),
                TextComponent::plain("Kills".to_owned()),
                RenderType::Integer,
                false,
                None,
            )
            .expect("objective should be added");
        let holder = ScoreHolder::new("Steve");
        scoreboard
            .set_score(&holder, &objective, 7)
            .expect("score should be writable");
        scoreboard
            .set_display_objective(DisplaySlot::Sidebar, Some(&objective))
            .expect("display slot should be set");

        let names: Vec<&str> = scoreboard
            .pending_packets()
            .iter()
            .map(|packet| match packet {
                ScoreboardPacket::Objective(packet) => {
                    assert_eq!(packet.objective_name, "kills");
                    assert_eq!(
                        packet.display_name,
                        TextComponent::plain("Kills".to_owned())
                    );
                    "objective-add"
                }
                ScoreboardPacket::Score(packet) => {
                    assert_eq!(packet.objective_name, "kills");
                    assert_eq!(packet.owner, "Steve");
                    assert_eq!(packet.score, 7);
                    "score"
                }
                ScoreboardPacket::DisplayObjective(packet) => {
                    assert!(matches!(packet.slot, ScoreboardDisplaySlot::Sidebar));
                    assert_eq!(packet.objective_name.as_deref(), Some("kills"));
                    "display-slot"
                }
                other => panic!("unexpected packet: {other:?}"),
            })
            .collect();
        // Vanilla `ServerScoreboard.getStartTrackingPackets` order: the objective, then the
        // display slot that tracks it, then the scores already recorded in it. The objective
        // and its score are held back until the slot makes the objective tracked.
        assert_eq!(names, ["objective-add", "display-slot", "score"]);
        assert!(scoreboard.pending_packets().is_empty());
    }

    /// Vanilla `ServerScoreboard.onTeamAdded/onTeamChanged`: a team's display options
    /// reach clients in `ClientboundSetPlayerTeamPacket` add/change packets carrying
    /// the same parameters.
    #[test]
    fn team_display_properties_reach_the_client_packets() {
        use steel_protocol::packets::game::{TeamAction, TeamColor as WireTeamColor};

        let scoreboard = Scoreboard::new();
        scoreboard.add_team("red").expect("team should be added");
        let holder = ScoreHolder::new("Steve");
        let red = scoreboard.team("red").expect("team should exist");
        scoreboard
            .add_holder_to_team(&holder, &red)
            .expect("holder should join red");
        scoreboard
            .set_team_display_name("red", TextComponent::plain("Red Team".to_owned()))
            .expect("display name should change");
        scoreboard
            .set_team_color("red", Some(TeamColor::Red))
            .expect("color should change");
        scoreboard
            .set_team_prefix("red", TextComponent::plain("[".to_owned()))
            .expect("prefix should change");

        let mut kinds: Vec<&str> = Vec::new();
        let mut last_change = None;
        for packet in scoreboard.pending_packets() {
            let ScoreboardPacket::Team(packet) = packet else {
                panic!("expected only team packets, got {packet:?}");
            };
            assert_eq!(packet.name, "red");
            match packet.method {
                TeamAction::Add => {
                    let params = packet.parameters.expect("add carries parameters");
                    // Vanilla `/team add <name>` starts with the name as the display name.
                    assert_eq!(params.display_name, TextComponent::plain("red"));
                    assert_eq!(
                        packet.players,
                        Vec::<String>::new(),
                        "a freshly created team has no members"
                    );
                    kinds.push("add");
                }
                TeamAction::Join => {
                    assert!(packet.parameters.is_none());
                    assert_eq!(packet.players, vec!["Steve".to_owned()]);
                    kinds.push("join");
                }
                TeamAction::Change => {
                    last_change = packet.parameters;
                    kinds.push("change");
                }
                action => panic!("unexpected team packet action: {action:?}"),
            }
        }
        // Vanilla's packet mix for this burst: `createAddOrModifyPacket(team, true)` on
        // creation, one `createPlayerPacket(team, player, ADD)` for the join, and one
        // `createAddOrModifyPacket(team, false)` per option change. Every change re-sends the
        // full parameters block, so the last one carries all three new values.
        assert_eq!(kinds, ["add", "join", "change", "change", "change"]);
        let params = last_change.expect("change carries parameters");
        assert_eq!(
            params.display_name,
            TextComponent::plain("Red Team".to_owned())
        );
        assert_eq!(params.player_prefix, TextComponent::plain("[".to_owned()));
        assert_eq!(params.color, Some(WireTeamColor::Red));
    }

    /// Vanilla `PlayerList.updateEntireScoreboard` + `getStartTrackingPackets`: the
    /// join sync creates every team first, then starts each displayed objective once
    /// with its display slots and scores, never resending one objective.
    #[test]
    fn initial_sync_sends_teams_then_displayed_objectives_once() {
        let scoreboard = Scoreboard::new();
        let kills = scoreboard
            .add_objective("kills")
            .expect("objective should be added");
        let deaths = scoreboard
            .add_objective("deaths")
            .expect("objective should be added");
        let holder = ScoreHolder::new("Steve");
        scoreboard
            .set_score(&holder, &kills, 3)
            .expect("score should be writable");
        scoreboard
            .set_score(&holder, &deaths, 1)
            .expect("hidden scores should be writable");
        scoreboard
            .set_display_objective(DisplaySlot::Sidebar, Some(&kills))
            .expect("display slot should be set");
        scoreboard
            .set_display_objective(DisplaySlot::List, Some(&kills))
            .expect("display slot should be set");
        scoreboard.add_team("red").expect("team should be added");

        let packet_kinds: Vec<String> = scoreboard
            .initial_sync_packets()
            .iter()
            .map(|packet| match packet {
                ScoreboardPacket::Team(packet) => {
                    assert_eq!(packet.name, "red");
                    "team".to_owned()
                }
                ScoreboardPacket::Objective(packet) => {
                    format!("objective:{}", packet.objective_name)
                }
                ScoreboardPacket::Score(packet) => {
                    format!("score:{}:{}", packet.objective_name, packet.owner)
                }
                ScoreboardPacket::DisplayObjective(_) => "display-slot".to_owned(),
                ScoreboardPacket::ResetScore(packet) => {
                    panic!("unexpected packet: {packet:?}")
                }
            })
            .collect();
        // Teams first, then the displayed objective's create + both of its display slots +
        // its score exactly once; the undisplaced `deaths` objective stays server-side.
        assert_eq!(
            packet_kinds,
            [
                "team",
                "objective:kills",
                "display-slot",
                "display-slot",
                "score:kills:Steve",
            ]
        );
    }

    /// The objective's `Fixed` number format travels in the objective packet (vanilla
    /// `ClientboundSetObjectivePacket`), while scores carry only their own override.
    #[test]
    fn objective_format_and_score_override_travel_in_separate_packets() {
        let scoreboard = Scoreboard::new();
        let objective = scoreboard
            .add_objective_full(
                "styled",
                ObjectiveCriteria::dummy(),
                TextComponent::new(),
                RenderType::Integer,
                false,
                Some(fixed_number_format()),
            )
            .expect("objective should be added");
        let holder = ScoreHolder::new("Steve");
        scoreboard
            .set_score(&holder, &objective, 4)
            .expect("score should be writable");
        scoreboard
            .set_display_objective(DisplaySlot::Sidebar, Some(&objective))
            .expect("display slot should be set");

        let mut objective_packet = None;
        let mut score_packet = None;
        for packet in scoreboard.pending_packets() {
            match packet {
                ScoreboardPacket::Objective(packet) => objective_packet = Some(packet),
                ScoreboardPacket::Score(packet) => score_packet = Some(packet),
                ScoreboardPacket::DisplayObjective(_) => {}
                other => panic!("unexpected packet: {other:?}"),
            }
        }
        let objective_packet = objective_packet.expect("start tracking sends the objective");
        let score_packet = score_packet.expect("start tracking sends the score");
        assert!(objective_packet.number_format.is_some());
        // Vanilla `ServerScoreboard.onScoreChanged` sends only the per-score override;
        // the objective's format must not leak into the score packet.
        assert!(score_packet.number_format.is_none());
    }

    /// Vanilla `ServerScoreboard.setDisplayObjective`: showing an objective no client has
    /// seen starts tracking it — the objective, its display slots, and its scores — rather
    /// than only moving the slot. A bare `ClientboundSetDisplayObjectivePacket` for an
    /// unknown name is resolved to `null` by the client, which clears the sidebar.
    #[test]
    fn displaying_a_previously_untracked_objective_starts_tracking_it() {
        use steel_protocol::packets::game::ObjectiveMethod;

        let scoreboard = Scoreboard::new();
        let first = scoreboard
            .add_objective("first")
            .expect("objective should be added");
        let second = scoreboard
            .add_objective("second")
            .expect("objective should be added");
        let holder = ScoreHolder::new("Steve");
        scoreboard
            .set_score(&holder, &second, 9)
            .expect("score should be writable");

        // Nothing reaches clients while the objective is not displayed.
        assert!(scoreboard.pending_packets().is_empty());

        scoreboard
            .set_display_objective(DisplaySlot::Sidebar, Some(&first))
            .expect("display slot should be set");
        scoreboard
            .set_display_objective(DisplaySlot::Sidebar, Some(&second))
            .expect("display slot should be set");

        let kinds: Vec<String> = scoreboard
            .pending_packets()
            .iter()
            .map(|packet| match packet {
                ScoreboardPacket::Objective(packet) => match packet.method {
                    ObjectiveMethod::Add => format!("add:{}", packet.objective_name),
                    ObjectiveMethod::Remove => format!("remove:{}", packet.objective_name),
                    ObjectiveMethod::Change => format!("change:{}", packet.objective_name),
                },
                ScoreboardPacket::DisplayObjective(packet) => {
                    format!("slot:{}", packet.objective_name.as_deref().unwrap_or("-"))
                }
                ScoreboardPacket::Score(packet) => {
                    format!("score:{}:{}", packet.objective_name, packet.owner)
                }
                ScoreboardPacket::ResetScore(packet) => format!("reset:{}", packet.owner),
                ScoreboardPacket::Team(packet) => format!("team:{}", packet.name),
            })
            .collect();
        // `first` starts tracking, then the swap stops it and starts `second`.
        assert_eq!(
            kinds,
            [
                "add:first",
                "slot:first",
                "remove:first",
                "add:second",
                "slot:second",
                "score:second:Steve",
            ]
        );

        // Re-setting the slot to the same objective only re-sends the slot contents.
        scoreboard
            .set_display_objective(DisplaySlot::Sidebar, Some(&second))
            .expect("display slot should be set");
        let repeat = scoreboard.pending_packets();
        assert_eq!(repeat.len(), 1);
        assert!(matches!(repeat[0], ScoreboardPacket::DisplayObjective(_)));
    }

    /// Clearing a slot: while the objective still shows in another slot the slot clear is
    /// broadcast; when it leaves every slot it stops being tracked
    /// (`ClientboundSetObjectivePacket(objective, 1)`), exactly like vanilla.
    #[test]
    fn clearing_a_display_slot_stops_tracking_only_on_the_last_slot() {
        use steel_protocol::packets::game::ObjectiveMethod;

        let scoreboard = Scoreboard::new();
        let objective = scoreboard
            .add_objective("kills")
            .expect("objective should be added");
        scoreboard
            .set_display_objective(DisplaySlot::Sidebar, Some(&objective))
            .expect("display slot should be set");
        scoreboard
            .set_display_objective(DisplaySlot::List, Some(&objective))
            .expect("display slot should be set");
        let _ = scoreboard.pending_packets();

        scoreboard
            .set_display_objective(DisplaySlot::Sidebar, None)
            .expect("display slot should be cleared");
        let first_clear = scoreboard.pending_packets();
        assert_eq!(first_clear.len(), 1);
        assert!(
            matches!(
                &first_clear[0],
                ScoreboardPacket::DisplayObjective(packet) if packet.objective_name.is_none()
            ),
            "a slot clear is broadcast while the objective is still displayed"
        );

        scoreboard
            .set_display_objective(DisplaySlot::List, None)
            .expect("display slot should be cleared");
        let last_clear = scoreboard.pending_packets();
        assert_eq!(last_clear.len(), 1);
        assert!(
            matches!(
                &last_clear[0],
                ScoreboardPacket::Objective(packet)
                    if packet.method == ObjectiveMethod::Remove
                        && packet.objective_name == "kills"
            ),
            "the last slot stops tracking the objective"
        );
    }

    /// The per-tick flush writes the queued packets to each client in the domain through
    /// `ScoreboardPacket::send_to`, which is what `DomainScoreboards::flush_updates` calls.
    #[test]
    fn queued_packets_are_written_to_the_client_connection() {
        use std::sync::Arc;

        use steel_registry::init_vanilla_registry;

        use crate::entity::boss::event::tests::recording_player;
        use crate::test_support::fresh_test_world;

        init_vanilla_registry();
        let fixture = fresh_test_world("scoreboard_flush_delivery");
        let (player, connection) = recording_player(&fixture.world, "SidebarViewer");

        let scoreboard = Scoreboard::new();
        let objective = scoreboard
            .add_objective("kills")
            .expect("objective should be added");
        let holder = ScoreHolder::new("Steve");
        scoreboard
            .set_score(&holder, &objective, 1)
            .expect("score should be writable");
        scoreboard
            .set_display_objective(DisplaySlot::Sidebar, Some(&objective))
            .expect("display slot should be set");

        let packets = scoreboard.pending_packets();
        assert_eq!(
            packets.len(),
            3,
            "a sidebar objective writes its whole bundle"
        );
        for packet in &packets {
            packet.send_to(&player);
        }
        assert_eq!(
            connection.sent(),
            3,
            "every queued packet reaches the client connection"
        );
        assert!(scoreboard.pending_packets().is_empty());
        let _ = Arc::clone(&player);
    }
}
