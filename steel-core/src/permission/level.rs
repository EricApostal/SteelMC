//! Vanilla command permission levels (op levels 0-4) and their set semantics.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as DeError, de::Visitor};
use steel_utils::entity_events::EntityStatus;

/// Vanilla's numeric operator permission level.
///
/// Mirrors `net.minecraft.server.permissions.PermissionLevel`; Steel keeps its
/// named permission keys as an additional grant source, so this enum only
/// defines the vanilla level ordering used by `CommandLevel` requirements.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PermissionLevel {
    /// Level 0 - every player, no operator powers.
    #[default]
    All,
    /// Level 1 - moderators.
    Moderators,
    /// Level 2 - gamemasters.
    Gamemasters,
    /// Level 3 - admins.
    Admins,
    /// Level 4 - owners.
    Owners,
}

impl PermissionLevel {
    /// Every level in ascending order.
    pub const ALL_LEVELS: [Self; 5] = [
        Self::All,
        Self::Moderators,
        Self::Gamemasters,
        Self::Admins,
        Self::Owners,
    ];

    /// Vanilla level id (`0`-`4`).
    #[must_use]
    pub const fn id(self) -> u8 {
        match self {
            Self::All => 0,
            Self::Moderators => 1,
            Self::Gamemasters => 2,
            Self::Admins => 3,
            Self::Owners => 4,
        }
    }

    /// Vanilla serialized name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Moderators => "moderators",
            Self::Gamemasters => "gamemasters",
            Self::Admins => "admins",
            Self::Owners => "owners",
        }
    }

    /// Returns the level for a vanilla id, clamping out-of-range values as vanilla does.
    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            i32::MIN..=0 => Self::All,
            1 => Self::Moderators,
            2 => Self::Gamemasters,
            3 => Self::Admins,
            _ => Self::Owners,
        }
    }

    /// Parses a vanilla serialized level name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL_LEVELS
            .into_iter()
            .find(|level| level.name() == name)
    }

    /// Returns whether this level grants at least `required`.
    #[must_use]
    pub const fn is_equal_or_higher_than(self, required: Self) -> bool {
        self.id() >= required.id()
    }

    /// Returns the vanilla client entity-event id announcing this level.
    #[must_use]
    pub const fn client_entity_status(self) -> EntityStatus {
        match self {
            Self::All => EntityStatus::PermissionLevelAll,
            Self::Moderators => EntityStatus::PermissionLevelModerators,
            Self::Gamemasters => EntityStatus::PermissionLevelGamemasters,
            Self::Admins => EntityStatus::PermissionLevelAdmins,
            Self::Owners => EntityStatus::PermissionLevelOwners,
        }
    }
}

impl fmt::Display for PermissionLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Vanilla `LevelBasedPermissionSet`: a permission set defined only by its level.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LevelBasedPermissionSet {
    level: PermissionLevel,
}

impl LevelBasedPermissionSet {
    /// The level-0 set.
    pub const ALL: Self = Self::for_level(PermissionLevel::All);
    /// The level-1 set.
    pub const MODERATOR: Self = Self::for_level(PermissionLevel::Moderators);
    /// The level-2 set.
    pub const GAMEMASTER: Self = Self::for_level(PermissionLevel::Gamemasters);
    /// The level-3 set.
    pub const ADMIN: Self = Self::for_level(PermissionLevel::Admins);
    /// The level-4 set.
    pub const OWNER: Self = Self::for_level(PermissionLevel::Owners);

    /// Wraps one level.
    #[must_use]
    pub const fn for_level(level: PermissionLevel) -> Self {
        Self { level }
    }

    /// Returns the wrapped level.
    #[must_use]
    pub const fn level(self) -> PermissionLevel {
        self.level
    }

    /// Returns whether this set satisfies `required`.
    #[must_use]
    pub const fn has_command_level(self, required: PermissionLevel) -> bool {
        self.level.is_equal_or_higher_than(required)
    }

    /// Returns whether this set may use advanced entity selectors.
    ///
    /// Vanilla grants `commands/entity_selectors` at `GAMEMASTERS`.
    #[must_use]
    pub const fn allows_entity_selectors(self) -> bool {
        self.has_command_level(PermissionLevel::Gamemasters)
    }
}

impl Serialize for PermissionLevel {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(self.id())
    }
}

impl<'de> Deserialize<'de> for PermissionLevel {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct LevelVisitor;

        impl Visitor<'_> for LevelVisitor {
            type Value = PermissionLevel;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a vanilla op level id (0-4) or level name")
            }

            fn visit_i64<E: DeError>(self, value: i64) -> Result<Self::Value, E> {
                Ok(PermissionLevel::from_id(
                    i32::try_from(value).unwrap_or(i32::MAX),
                ))
            }

            fn visit_u64<E: DeError>(self, value: u64) -> Result<Self::Value, E> {
                Ok(PermissionLevel::from_id(
                    i32::try_from(value).unwrap_or(i32::MAX),
                ))
            }

            fn visit_str<E: DeError>(self, value: &str) -> Result<Self::Value, E> {
                PermissionLevel::from_name(value)
                    .ok_or_else(|| E::custom(format!("unknown permission level '{value}'")))
            }
        }

        deserializer.deserialize_any(LevelVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::{LevelBasedPermissionSet, PermissionLevel};

    #[test]
    fn level_ids_and_order_match_vanilla() {
        assert_eq!(PermissionLevel::All.id(), 0);
        assert_eq!(PermissionLevel::Owners.id(), 4);
        assert!(PermissionLevel::Owners.is_equal_or_higher_than(PermissionLevel::Admins));
        assert!(!PermissionLevel::Gamemasters.is_equal_or_higher_than(PermissionLevel::Admins));
        assert!(PermissionLevel::Admins.is_equal_or_higher_than(PermissionLevel::Admins));
    }

    #[test]
    fn ids_clamp_like_vanillas_continuous_map() {
        assert_eq!(PermissionLevel::from_id(-5), PermissionLevel::All);
        assert_eq!(PermissionLevel::from_id(2), PermissionLevel::Gamemasters);
        assert_eq!(PermissionLevel::from_id(99), PermissionLevel::Owners);
    }

    #[test]
    fn names_round_trip() {
        for level in PermissionLevel::ALL_LEVELS {
            assert_eq!(PermissionLevel::from_name(level.name()), Some(level));
        }
        assert_eq!(PermissionLevel::from_name("serverop"), None);
    }

    #[test]
    fn level_sets_gate_command_levels_and_entity_selectors() {
        assert!(LevelBasedPermissionSet::OWNER.has_command_level(PermissionLevel::Admins));
        assert!(!LevelBasedPermissionSet::GAMEMASTER.has_command_level(PermissionLevel::Admins));
        assert!(LevelBasedPermissionSet::GAMEMASTER.allows_entity_selectors());
        assert!(!LevelBasedPermissionSet::ALL.allows_entity_selectors());
    }

    #[test]
    fn serialization_uses_the_vanilla_level_id() {
        let serialized = toml::to_string(&LevelField {
            level: PermissionLevel::Admins,
        });
        assert!(serialized.is_ok_and(|value| value.contains("level = 3")));
    }

    #[derive(serde::Serialize)]
    struct LevelField {
        level: PermissionLevel,
    }
}
