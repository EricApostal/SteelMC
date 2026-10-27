//! Objective criteria and render types.
//!
//! Mirrors vanilla `ObjectiveCriteria` and its `RenderType`: the fixed custom
//! criteria, the per-team-color kill criteria, and the `stat_type:value`
//! dynamic lookups into the stat registries.

use serde::{Deserialize, Serialize};
use steel_registry::stat::Stat;
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

    /// Vanilla `ObjectiveCriteria.DEATH_COUNT`.
    #[must_use]
    pub fn death_count() -> Self {
        Self::from_parts(
            "deathCount",
            false,
            RenderType::Integer,
            CriteriaKind::DeathCount,
        )
    }

    /// Vanilla `ObjectiveCriteria.KILL_COUNT_PLAYERS`.
    #[must_use]
    pub fn player_kill_count() -> Self {
        Self::from_parts(
            "playerKillCount",
            false,
            RenderType::Integer,
            CriteriaKind::PlayerKillCount,
        )
    }

    /// Vanilla `ObjectiveCriteria.KILL_COUNT_ALL`.
    #[must_use]
    pub fn total_kill_count() -> Self {
        Self::from_parts(
            "totalKillCount",
            false,
            RenderType::Integer,
            CriteriaKind::TotalKillCount,
        )
    }

    /// Vanilla `ObjectiveCriteria.HEALTH`.
    #[must_use]
    pub fn health() -> Self {
        Self::from_parts("health", true, RenderType::Hearts, CriteriaKind::Health)
    }

    /// Vanilla `ObjectiveCriteria.FOOD`.
    #[must_use]
    pub fn food() -> Self {
        Self::from_parts("food", true, RenderType::Integer, CriteriaKind::Food)
    }

    /// Vanilla `ObjectiveCriteria.AIR`.
    #[must_use]
    pub fn air() -> Self {
        Self::from_parts("air", true, RenderType::Integer, CriteriaKind::Air)
    }

    /// Vanilla `ObjectiveCriteria.ARMOR`.
    #[must_use]
    pub fn armor() -> Self {
        Self::from_parts("armor", true, RenderType::Integer, CriteriaKind::Armor)
    }

    /// Vanilla `ObjectiveCriteria.EXPERIENCE`.
    #[must_use]
    pub fn experience() -> Self {
        Self::from_parts("xp", true, RenderType::Integer, CriteriaKind::Experience)
    }

    /// Vanilla `ObjectiveCriteria.LEVEL`.
    #[must_use]
    pub fn level() -> Self {
        Self::from_parts("level", true, RenderType::Integer, CriteriaKind::Level)
    }

    /// Vanilla `ObjectiveCriteria.TEAM_KILL` for one color.
    #[must_use]
    pub fn team_kill(color: TeamColor) -> Self {
        Self::from_parts(
            &format!("teamkill.{}", color.as_str()),
            false,
            RenderType::Integer,
            CriteriaKind::TeamKill(color),
        )
    }

    /// Vanilla `ObjectiveCriteria.KILLED_BY_TEAM` for one color.
    #[must_use]
    pub fn killed_by_team(color: TeamColor) -> Self {
        Self::from_parts(
            &format!("killedByTeam.{}", color.as_str()),
            false,
            RenderType::Integer,
            CriteriaKind::KilledByTeam(color),
        )
    }

    /// Vanilla `ObjectiveCriteria.getStat`: the criteria of an objective backed by
    /// a stat in one of the stat-type registries.
    ///
    /// The name is spelled the way `ObjectiveCriteriaArgument` suggests it, i.e.
    /// with the namespace separators of both identifiers replaced by `.`
    /// (`minecraft.mined:minecraft.stone`). Only [`Self::same_criteria`] decides
    /// whether two stat criteria are the same objective criterion, so the other
    /// spellings vanilla accepts (`mined:stone`) still compare equal.
    #[must_use]
    pub fn from_stat(stat: &Stat) -> Self {
        Self::from_parts(
            &format!(
                "{}:{}",
                dotted_identifier(stat.stat_type_key()),
                dotted_identifier(stat.stat_value_key())
            ),
            false,
            RenderType::Integer,
            CriteriaKind::Stat,
        )
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

    /// Vanilla `Scoreboard`'s criterion identity.
    ///
    /// Vanilla keys its objectives by the `Criteria` object itself, and every
    /// spelling `ObjectiveCriteria.byName` accepts for a stat resolves to that
    /// single `Stat`, so `mined:stone` and `minecraft.mined:minecraft.stone` are
    /// one criterion. Steel models each criterion as the name it was parsed from,
    /// so stat criteria compare by their resolved `(stat type, stat value)` pair
    /// and every other criterion compares by name.
    #[must_use]
    pub fn same_criteria(&self, other: &Self) -> bool {
        match (self.stat_ids(), other.stat_ids()) {
            (Some(left), Some(right)) => left == right,
            _ => self.name == other.name,
        }
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

    /// Vanilla `ObjectiveCriteria.CUSTOM_CRITERIA`: the fixed custom names.
    ///
    /// Every branch delegates to the constructor that owns the criterion's flags
    /// so the parse table and the driver's constructors cannot drift apart.
    fn custom(name: &str) -> Option<Self> {
        Some(match name {
            "dummy" => Self::dummy(),
            "trigger" => Self::trigger(),
            "deathCount" => Self::death_count(),
            "playerKillCount" => Self::player_kill_count(),
            "totalKillCount" => Self::total_kill_count(),
            "health" => Self::health(),
            "food" => Self::food(),
            "air" => Self::air(),
            "armor" => Self::armor(),
            "xp" => Self::experience(),
            "level" => Self::level(),
            _ => return None,
        })
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

/// The inverse of [`identifier_by_separator`] for the `minecraft` namespace: a
/// vanilla identifier's own namespace is left implicit so the criteria name is
/// spelled the way `ObjectiveCriteriaArgument` suggests it.
fn dotted_identifier(identifier: &Identifier) -> String {
    if identifier.namespace == Identifier::VANILLA_NAMESPACE {
        identifier.path.to_string()
    } else {
        format!("{}.{}", identifier.namespace, identifier.path)
    }
}

#[cfg(test)]
mod tests {
    use super::{CriteriaKind, ObjectiveCriteria, RenderType};
    use crate::scoreboard::team::TeamColor;
    use steel_registry::stat::vanilla_stat_types;
    use steel_registry::vanilla_blocks;

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

    #[test]
    fn constructors_match_the_parse_table() {
        // The driver's constructors must own exactly the names `parse` resolves,
        // otherwise the driver's `forAllObjectives` calls would miss them.
        for criteria in [
            ObjectiveCriteria::dummy(),
            ObjectiveCriteria::trigger(),
            ObjectiveCriteria::death_count(),
            ObjectiveCriteria::player_kill_count(),
            ObjectiveCriteria::total_kill_count(),
            ObjectiveCriteria::health(),
            ObjectiveCriteria::food(),
            ObjectiveCriteria::air(),
            ObjectiveCriteria::armor(),
            ObjectiveCriteria::experience(),
            ObjectiveCriteria::level(),
            ObjectiveCriteria::team_kill(TeamColor::Red),
            ObjectiveCriteria::killed_by_team(TeamColor::Blue),
        ] {
            let parsed = ObjectiveCriteria::parse(criteria.name())
                .unwrap_or_else(|| panic!("{} should parse", criteria.name()));
            assert!(parsed.same_criteria(&criteria));
            assert_eq!(parsed.is_read_only(), criteria.is_read_only());
            assert_eq!(parsed.render_type(), criteria.render_type());
        }
    }

    #[test]
    fn same_criteria_covers_equivalent_stat_spellings() {
        steel_registry::init_vanilla_registry();
        let stat = vanilla_stat_types::BLOCK_MINED.get(&vanilla_blocks::STONE);
        let criteria = ObjectiveCriteria::from_stat(&stat);
        assert_eq!(criteria.stat_ids().map(|(left, _)| left), {
            let parsed = ObjectiveCriteria::parse("minecraft.mined:minecraft.stone")
                .expect("stat should parse");
            parsed.stat_ids().map(|(left, _)| left)
        });
        let spelled = ObjectiveCriteria::parse("mined:stone").expect("short spelling should parse");
        // Vanilla resolves both spellings to the same `Stat`.
        assert!(criteria.same_criteria(&spelled));
        let other = ObjectiveCriteria::parse("minecraft.mined:minecraft.dirt")
            .expect("other stat should parse");
        assert!(!criteria.same_criteria(&other));
        let custom = ObjectiveCriteria::death_count();
        assert!(!custom.same_criteria(&criteria));
        assert!(!criteria.same_criteria(&custom));
    }
}
