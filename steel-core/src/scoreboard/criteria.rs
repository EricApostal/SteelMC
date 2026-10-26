//! Objective criteria and render types.
//!
//! Mirrors vanilla `ObjectiveCriteria` and its `RenderType`: the fixed custom
//! criteria, the per-team-color kill criteria, and the `stat_type:value`
//! dynamic lookups into the stat registries.

use serde::{Deserialize, Serialize};
use steel_registry::{REGISTRY, RegistryExt};
use steel_utils::Identifier;

use crate::scoreboard::team::TeamColor;

/// Vanilla `ObjectiveCriteria.RenderType`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenderType {
    /// Plain integer rendering.
    #[default]
    #[serde(rename = "integer")]
    Integer,
    /// Heart-row rendering used by health.
    #[serde(rename = "hearts")]
    Hearts,
}

impl RenderType {
    /// Vanilla `RenderType.getId`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Integer => "integer",
            Self::Hearts => "hearts",
        }
    }

    /// Vanilla `RenderType.CODEC` name lookup.
    #[must_use]
    #[expect(
        clippy::should_implement_trait,
        reason = "mirrors the vanilla criteria registry name lookup; called by save-data and command parsers"
    )]
    pub fn from_str(name: &str) -> Option<Self> {
        match name {
            "integer" => Some(Self::Integer),
            "hearts" => Some(Self::Hearts),
            _ => None,
        }
    }

    /// Parses a render type name, defaulting to integer like vanilla.
    #[must_use]
    pub fn by_name_or_default(name: &str) -> Self {
        Self::from_str(name).unwrap_or(Self::Integer)
    }
}

/// Which kind of gameplay event advances an objective.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CriteriaKind {
    /// `dummy`: only changed by commands.
    Dummy,
    /// `trigger`: player-driven one-shot scores.
    Trigger,
    /// `deathCount`: player deaths.
    DeathCount,
    /// `playerKillCount`: player kills of players.
    PlayerKillCount,
    /// `totalKillCount`: player kills of anything.
    TotalKillCount,
    /// `health`: living health, read-only.
    Health,
    /// `food`: hunger level, read-only.
    Food,
    /// `air`: remaining air, read-only.
    Air,
    /// `armor`: armor value, read-only.
    Armor,
    /// `xp`: experience level progress, read-only.
    Experience,
    /// `level`: experience level, read-only.
    Level,
    /// `teamkill.<color>`: kills of a team's color.
    TeamKill(TeamColor),
    /// `killedByTeam.<color>`: deaths caused by a team's color.
    KilledByTeam(TeamColor),
    /// `stat_type:value`: any stat in the stat-type registries.
    Stat,
}

/// Vanilla `ObjectiveCriteria`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectiveCriteria {
    name: String,
    read_only: bool,
    render_type: RenderType,
    kind: CriteriaKind,
}

impl ObjectiveCriteria {
    /// Parses a vanilla criteria name, mirroring `ObjectiveCriteria.byName`.
    ///
    /// Returns `None` when the name is not a known custom criterion and does not
    /// resolve a stat through the stat-type registries.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        if let Some(known) = Self::custom(name) {
            return Some(known);
        }
        if let Some(known) = Self::team_criteria(name) {
            return Some(known);
        }
        let (stat_type_name, _) = name.split_once(':')?;
        if !Self::stat_exists(name) {
            return None;
        }
        let _ = stat_type_name;
        Some(Self::from_parts(
            name,
            false,
            RenderType::Integer,
            CriteriaKind::Stat,
        ))
    }

    /// Returns the vanilla criteria name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Vanilla `ObjectiveCriteria.isReadOnly`.
    #[must_use]
    pub const fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Vanilla `ObjectiveCriteria.getDefaultRenderType`.
    #[must_use]
    pub const fn render_type(&self) -> RenderType {
        self.render_type
    }

    /// Returns which gameplay event advances this objective.
    #[must_use]
    pub const fn kind(&self) -> CriteriaKind {
        self.kind
    }

    /// Vanilla `ObjectiveCriteria.DUMMY`.
    #[must_use]
    pub fn dummy() -> Self {
        Self::from_parts("dummy", false, RenderType::Integer, CriteriaKind::Dummy)
    }

    /// Vanilla `ObjectiveCriteria.TRIGGER`.
    #[must_use]
    pub fn trigger() -> Self {
        Self::from_parts("trigger", false, RenderType::Integer, CriteriaKind::Trigger)
    }

    /// Resolves a stat criterion to its `(stat type, stat value)` identifiers.
    ///
    /// Only meaningful when [`Self::kind`] is [`CriteriaKind::Stat`].
    #[must_use]
    pub fn stat_ids(&self) -> Option<(Identifier, Identifier)> {
        if self.kind != CriteriaKind::Stat {
            return None;
        }
        let (stat_type, stat_value) = self.name.split_once(':')?;
        Some((
            identifier_by_separator(stat_type),
            identifier_by_separator(stat_value),
        ))
    }

    /// All registered custom-criteria names, like `getCustomCriteriaNames`.
    #[must_use]
    pub fn custom_criteria_names() -> Vec<String> {
        let mut names = [
            "dummy",
            "trigger",
            "deathCount",
            "playerKillCount",
            "totalKillCount",
            "health",
            "food",
            "air",
            "armor",
            "xp",
            "level",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        for color in TeamColor::ALL {
            names.push(format!("teamkill.{}", color.as_str()));
            names.push(format!("killedByTeam.{}", color.as_str()));
        }
        names
    }

    fn from_parts(
        name: &str,
        read_only: bool,
        render_type: RenderType,
        kind: CriteriaKind,
    ) -> Self {
        Self {
            name: name.to_owned(),
            read_only,
            render_type,
            kind,
        }
    }

    fn custom(name: &str) -> Option<Self> {
        let (read_only, render_type, kind) = match name {
            "dummy" => (false, RenderType::Integer, CriteriaKind::Dummy),
            "trigger" => (false, RenderType::Integer, CriteriaKind::Trigger),
            "deathCount" => (false, RenderType::Integer, CriteriaKind::DeathCount),
            "playerKillCount" => (false, RenderType::Integer, CriteriaKind::PlayerKillCount),
            "totalKillCount" => (false, RenderType::Integer, CriteriaKind::TotalKillCount),
            "health" => (true, RenderType::Hearts, CriteriaKind::Health),
            "food" => (true, RenderType::Integer, CriteriaKind::Food),
            "air" => (true, RenderType::Integer, CriteriaKind::Air),
            "armor" => (true, RenderType::Integer, CriteriaKind::Armor),
            "xp" => (true, RenderType::Integer, CriteriaKind::Experience),
            "level" => (true, RenderType::Integer, CriteriaKind::Level),
            _ => return None,
        };
        Some(Self::from_parts(name, read_only, render_type, kind))
    }

    fn team_criteria(name: &str) -> Option<Self> {
        let (prefix, color_name) = name.split_once('.')?;
        let color = TeamColor::from_str(color_name)?;
        let kind = match prefix {
            "teamkill" => CriteriaKind::TeamKill(color),
            "killedByTeam" => CriteriaKind::KilledByTeam(color),
            _ => return None,
        };
        Some(Self::from_parts(name, false, RenderType::Integer, kind))
    }

    /// Vanilla `ObjectiveCriteria.byName` stat branch.
    fn stat_exists(name: &str) -> bool {
        let Some((stat_type, stat_value)) = name.split_once(':') else {
            return false;
        };
        let stat_type_id = identifier_by_separator(stat_type);
        let Some(entry) = REGISTRY.stat_types.by_key(&stat_type_id) else {
            return false;
        };
        entry
            .value_from_key(&identifier_by_separator(stat_value))
            .is_some()
    }
}

/// Vanilla `Identifier.bySeparator(identifier, '.')`: the first separator becomes
/// the namespace separator, an empty namespace falls back to `minecraft`, and a
/// name without a separator also uses the default namespace.
fn identifier_by_separator(name: &str) -> Identifier {
    match name.split_once('.') {
        Some(("", path)) => Identifier::new(Identifier::VANILLA_NAMESPACE, path.to_owned()),
        Some((namespace, path)) => Identifier::new(namespace.to_owned(), path.to_owned()),
        None => Identifier::new(Identifier::VANILLA_NAMESPACE, name.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::{CriteriaKind, ObjectiveCriteria, RenderType};
    use crate::scoreboard::team::TeamColor;

    #[test]
    fn parses_all_custom_criteria() {
        for name in ObjectiveCriteria::custom_criteria_names() {
            assert!(
                ObjectiveCriteria::parse(&name).is_some(),
                "criteria {name} should parse"
            );
        }
        assert!(ObjectiveCriteria::parse("missing").is_none());
        assert!(ObjectiveCriteria::parse("teamkill.off_white").is_none());
    }

    #[test]
    fn read_only_criteria_match_vanilla() {
        for name in ["health", "food", "air", "armor", "xp", "level"] {
            let criteria = ObjectiveCriteria::parse(name).expect("read-only criteria should parse");
            assert!(criteria.is_read_only());
        }
        assert_eq!(
            ObjectiveCriteria::parse("health")
                .expect("health should parse")
                .render_type(),
            RenderType::Hearts
        );
        assert!(!ObjectiveCriteria::dummy().is_read_only());
        assert!(!ObjectiveCriteria::trigger().is_read_only());
    }

    #[test]
    fn team_criteria_bind_colors() {
        let criteria = ObjectiveCriteria::parse("teamkill.red").expect("teamkill should parse");
        assert_eq!(criteria.kind(), CriteriaKind::TeamKill(TeamColor::Red));
        assert!(ObjectiveCriteria::parse("killedByTeam.blue").is_some());
        assert!(ObjectiveCriteria::parse("teamkill").is_none());
    }

    #[test]
    fn stat_criteria_resolve_through_the_registries() {
        steel_registry::init_vanilla_registry();
        let criteria =
            ObjectiveCriteria::parse("minecraft.mined:minecraft.stone").expect("stat should parse");
        let (stat_type, stat_value) = criteria.stat_ids().expect("stat ids should resolve");
        assert_eq!(stat_type.namespace.as_ref(), "minecraft");
        assert_eq!(stat_type.path.as_ref(), "mined");
        assert_eq!(stat_value.path.as_ref(), "stone");
        assert!(ObjectiveCriteria::parse("minecraft.mined:minecraft.not_a_block").is_none());
    }

    #[test]
    fn render_type_defaults_to_integer() {
        assert_eq!(RenderType::by_name_or_default("hearts"), RenderType::Hearts);
        assert_eq!(RenderType::by_name_or_default("other"), RenderType::Integer);
    }
}
