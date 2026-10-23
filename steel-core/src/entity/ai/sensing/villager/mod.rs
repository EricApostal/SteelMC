//! Vanilla villager sensors (`Villager.BRAIN_PROVIDER`).
//!
//! One submodule per vanilla `net.minecraft.world.entity.ai.sensing` class. The
//! [`SensorType`] statics below mirror vanilla `SensorType` constants; the villager
//! brain builder owns their registration order.

pub(crate) mod golem;
pub(crate) mod hurt_by;
pub(crate) mod nearest_bed;
pub(crate) mod nearest_item;
pub(crate) mod nearest_living;
pub(crate) mod player;
pub(crate) mod secondary_pois;
pub(crate) mod villager_babies;
pub(crate) mod villager_hostiles;

use steel_registry::vanilla_attributes;

use crate::entity::SharedEntity;
use crate::entity::ai::brain::sensor::SensorType;
use crate::entity::ai::targeting::TargetingConditions;
use crate::world::World;

/// Vanilla `SensorType.NEAREST_LIVING_ENTITIES`.
pub(crate) static NEAREST_LIVING_ENTITIES: SensorType =
    SensorType::new(nearest_living::create, "nearest_living_entities");

/// Vanilla `SensorType.NEAREST_PLAYERS`.
pub(crate) static NEAREST_PLAYERS: SensorType = SensorType::new(player::create, "nearest_players");

/// Vanilla `SensorType.NEAREST_ITEMS`.
pub(crate) static NEAREST_ITEMS: SensorType =
    SensorType::new(nearest_item::create, "nearest_items");

/// Vanilla `SensorType.NEAREST_BED`.
pub(crate) static NEAREST_BED: SensorType = SensorType::new(nearest_bed::create, "nearest_bed");

/// Vanilla `SensorType.HURT_BY`.
pub(crate) static HURT_BY: SensorType = SensorType::new(hurt_by::create, "hurt_by");

/// Vanilla `SensorType.VILLAGER_HOSTILES`.
pub(crate) static VILLAGER_HOSTILES: SensorType =
    SensorType::new(villager_hostiles::create, "villager_hostiles");

/// Vanilla `SensorType.VILLAGER_BABIES`.
pub(crate) static VILLAGER_BABIES: SensorType =
    SensorType::new(villager_babies::create, "villager_babies");

/// Vanilla `SensorType.SECONDARY_POIS`.
pub(crate) static SECONDARY_POIS: SensorType =
    SensorType::new(secondary_pois::create, "secondary_pois");

/// Vanilla `SensorType.GOLEM_DETECTED`.
pub(crate) static GOLEM_DETECTED: SensorType = SensorType::new(golem::create, "golem_detected");

/// Vanilla `LivingEntity.getAttributeValue(Attributes.FOLLOW_RANGE)`.
///
/// `None` only for bodies without the attribute (non-living or test doubles); the
/// sensors then skip the scan, where vanilla's `getAttributeValue` throws.
#[must_use]
pub(crate) fn follow_range(body: &SharedEntity) -> Option<f64> {
    body.as_living_entity().and_then(|living| {
        living
            .attributes()
            .lock()
            .get_value(vanilla_attributes::FOLLOW_RANGE)
    })
}

/// Vanilla `Entity.closerThan(other, distance)`: strictly closer than `distance`.
#[must_use]
pub(crate) fn closer_than(entity: &SharedEntity, other: &SharedEntity, distance: f64) -> bool {
    entity.position().distance_squared(other.position()) < distance * distance
}

/// Vanilla `Sensor.isEntityTargetable` (`TargetingConditions.forNonCombat()`).
#[must_use]
pub(crate) fn is_entity_targetable(
    level: &World,
    body: &SharedEntity,
    target: &SharedEntity,
    follow_range: f64,
    attack_target: Option<&SharedEntity>,
) -> bool {
    let (Some(body), Some(target)) = (body.as_living_entity(), target.as_living_entity()) else {
        return false;
    };

    if is_memory_target(target, attack_target) {
        TargetingConditions::for_non_combat()
            .range(follow_range)
            .ignore_invisibility_testing()
            .test(level, Some(body), target)
    } else {
        TargetingConditions::for_non_combat()
            .range(follow_range)
            .test(level, Some(body), target)
    }
}

/// Vanilla `Sensor.isEntityAttackable` (`TargetingConditions.forCombat()`).
#[must_use]
pub(crate) fn is_entity_attackable(
    level: &World,
    body: &SharedEntity,
    target: &SharedEntity,
    follow_range: f64,
    attack_target: Option<&SharedEntity>,
) -> bool {
    let (Some(body), Some(target)) = (body.as_living_entity(), target.as_living_entity()) else {
        return false;
    };

    if is_memory_target(target, attack_target) {
        TargetingConditions::for_combat()
            .range(follow_range)
            .ignore_invisibility_testing()
            .test(level, Some(body), target)
    } else {
        TargetingConditions::for_combat()
            .range(follow_range)
            .test(level, Some(body), target)
    }
}

/// Vanilla `body.getBrain().isMemoryValue(ATTACK_TARGET, entity)`.
fn is_memory_target(
    target: &dyn crate::entity::LivingEntity,
    attack_target: Option<&SharedEntity>,
) -> bool {
    attack_target.is_some_and(|attack_target| attack_target.id() == target.id())
}

#[cfg(test)]
mod tests {
    use super::*;
    use steel_utils::Identifier;

    /// Vanilla sensor → `requires()` memory keys, from each sensor class.
    static EXPECTED_REQUIRES: &[(&SensorType, &[&str])] = &[
        (&NEAREST_LIVING_ENTITIES, &["mobs", "visible_mobs"]),
        (
            &NEAREST_PLAYERS,
            &[
                "nearest_players",
                "nearest_visible_player",
                "nearest_visible_targetable_player",
                "nearest_visible_targetable_players",
            ],
        ),
        (&NEAREST_ITEMS, &["nearest_visible_wanted_item"]),
        (&NEAREST_BED, &["nearest_bed"]),
        (&HURT_BY, &["hurt_by", "hurt_by_entity"]),
        (&VILLAGER_HOSTILES, &["nearest_hostile", "visible_mobs"]),
        (&VILLAGER_BABIES, &["visible_villager_babies"]),
        (&SECONDARY_POIS, &["secondary_job_site"]),
        (&GOLEM_DETECTED, &["mobs", "golem_detected_recently"]),
    ];

    #[test]
    fn sensor_requires_match_vanilla() {
        assert_eq!(EXPECTED_REQUIRES.len(), 9);

        for (sensor_type, expected_keys) in EXPECTED_REQUIRES {
            let sensor = sensor_type.create();
            let keys: Vec<&Identifier> = sensor.requires().iter().map(|ty| ty.key()).collect();

            assert_eq!(
                keys.len(),
                expected_keys.len(),
                "sensor {} requires() count mismatch",
                sensor_type.debug_name()
            );
            for &expected_key in *expected_keys {
                let expected = Identifier::vanilla_static(expected_key);
                assert!(
                    keys.iter().any(|key| **key == expected),
                    "sensor {} must require {expected}",
                    sensor_type.debug_name()
                );
            }
        }
    }
}
