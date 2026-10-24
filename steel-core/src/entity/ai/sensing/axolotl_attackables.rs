//! Vanilla `AxolotlAttackablesSensor`.

use steel_registry::vanilla_entity_type_tags::EntityTypeTag;
use steel_registry::{REGISTRY, TaggedRegistryExt as _};

use crate::entity::SharedEntity;
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::MemoryStore;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::sensor::{BrainSensor, SensorType};
use crate::entity::ai::sensing::nearest_visible_living::set_nearest_visible_entity;
use crate::entity::ai::sensing::villager::{follow_range, is_entity_attackable};
use crate::world::World;

/// Vanilla `SensorType.AXOLOTL_ATTACKABLES`.
pub(crate) static AXOLOTL_ATTACKABLES: SensorType = SensorType::new(create, "axolotl_attackables");

/// Vanilla `SensorType.AXOLOTL_ATTACKABLES` factory.
pub(crate) fn create() -> Box<dyn BrainSensor> {
    Box::new(AxolotlAttackablesSensor)
}

/// Vanilla `AxolotlAttackablesSensor.TARGET_DETECTION_DISTANCE`.
///
/// Vanilla's `isClose` compares against a hardcoded `64.0`, which is exactly this
/// radius squared; the comparison below keeps the product so the constant documents
/// the radius without altering the arithmetic.
pub(crate) const TARGET_DETECTION_DISTANCE: f64 = 8.0;

/// Vanilla `AxolotlAttackablesSensor`: writes the nearest visible entity the axolotl
/// may attack to `NEAREST_ATTACKABLE`.
pub(crate) struct AxolotlAttackablesSensor;

impl BrainSensor for AxolotlAttackablesSensor {
    /// Vanilla `AxolotlAttackablesSensor.requires`:
    /// `Sets.union(super.requires(), Set.of(HAS_HUNTING_COOLDOWN))`, where
    /// `super.requires()` is `{getMemoryToSet(), NEAREST_VISIBLE_LIVING_ENTITIES}`.
    fn requires(&self) -> &'static [MemoryModuleTypeRef] {
        static REQUIRES: &[MemoryModuleTypeRef] = &[
            &mem::NEAREST_ATTACKABLE,
            &mem::NEAREST_VISIBLE_LIVING_ENTITIES,
            &mem::HAS_HUNTING_COOLDOWN,
        ];
        REQUIRES
    }

    /// Vanilla `AxolotlAttackablesSensor.doTick` through
    /// `NearestVisibleLivingEntitySensor.doTick`.
    fn sense(&mut self, level: &World, entity: &SharedEntity, memories: &mut MemoryStore) {
        let Some(follow_range) = follow_range(entity) else {
            // Without a follow range no target can pass `isEntityAttackable`, so the
            // memory is erased (vanilla's `getAttributeValue` would throw instead).
            memories.erase(&mem::NEAREST_ATTACKABLE);
            return;
        };

        // Vanilla reads both memories from the body's brain inside the predicate.
        let attack_target = memories.get(&mem::ATTACK_TARGET);
        let has_hunting_cooldown = memories.has_value(&mem::HAS_HUNTING_COOLDOWN);

        set_nearest_visible_entity(
            level,
            entity,
            memories,
            &mem::NEAREST_ATTACKABLE,
            move |level, body, mob| {
                is_matching_entity(
                    level,
                    body,
                    mob,
                    follow_range,
                    attack_target.as_ref(),
                    has_hunting_cooldown,
                )
            },
        );
    }
}

/// Vanilla `AxolotlAttackablesSensor.isMatchingEntity`.
#[must_use]
fn is_matching_entity(
    level: &World,
    body: &SharedEntity,
    mob: &SharedEntity,
    follow_range: f64,
    attack_target: Option<&SharedEntity>,
    has_hunting_cooldown: bool,
) -> bool {
    is_close(body, mob)
        && mob.is_in_water()
        && (is_hostile_target(mob) || is_hunt_target(mob, has_hunting_cooldown))
        && is_entity_attackable(level, body, mob, follow_range, attack_target)
}

/// Vanilla `AxolotlAttackablesSensor.isClose`: `mob.distanceToSqr(body) <= 64.0`.
#[must_use]
fn is_close(body: &SharedEntity, mob: &SharedEntity) -> bool {
    mob.distance_to_sqr(body.position()) <= TARGET_DETECTION_DISTANCE * TARGET_DETECTION_DISTANCE
}

/// Vanilla `AxolotlAttackablesSensor.isHostileTarget`:
/// `mob.is(EntityTypeTags.AXOLOTL_ALWAYS_HOSTILES)`.
#[must_use]
fn is_hostile_target(mob: &SharedEntity) -> bool {
    REGISTRY
        .entity_types
        .is_in_tag(mob.entity_type(), &EntityTypeTag::AXOLOTL_ALWAYS_HOSTILES)
}

/// Vanilla `AxolotlAttackablesSensor.isHuntTarget`:
/// `!body.getBrain().hasMemoryValue(HAS_HUNTING_COOLDOWN) && mob.is(AXOLOTL_HUNT_TARGETS)`.
///
/// `has_hunting_cooldown` is the body's `HAS_HUNTING_COOLDOWN` memory, read once per
/// scan by `AxolotlAttackablesSensor::sense` as vanilla reads it per predicate call.
#[must_use]
fn is_hunt_target(mob: &SharedEntity, has_hunting_cooldown: bool) -> bool {
    !has_hunting_cooldown
        && REGISTRY
            .entity_types
            .is_in_tag(mob.entity_type(), &EntityTypeTag::AXOLOTL_HUNT_TARGETS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::ai::brain::sensor::DEFAULT_SCAN_RATE;
    use steel_utils::Identifier;

    #[test]
    fn requires_matches_vanilla_union() {
        let sensor = create();
        let keys: Vec<&Identifier> = sensor.requires().iter().map(|ty| ty.key()).collect();

        assert_eq!(keys.len(), 3);
        assert_eq!(keys[0], &Identifier::vanilla_static("nearest_attackable"));
        assert_eq!(keys[1], &Identifier::vanilla_static("visible_mobs"));
        assert_eq!(keys[2], &Identifier::vanilla_static("has_hunting_cooldown"));
    }

    #[test]
    fn scan_rate_is_the_vanilla_default() {
        // Vanilla `AxolotlAttackablesSensor()` uses the no-arg `Sensor()` constructor.
        assert_eq!(create().scan_rate(), DEFAULT_SCAN_RATE);
    }

    #[test]
    fn target_detection_distance_squares_to_vanillas_hardcoded_64() {
        // Vanilla `isClose` hardcodes `64.0`.
        assert_eq!(TARGET_DETECTION_DISTANCE * TARGET_DETECTION_DISTANCE, 64.0);
    }
}
