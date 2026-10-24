//! Vanilla `FrogAttackablesSensor`.

use steel_registry::vanilla_entity_type_tags::EntityTypeTag;
use steel_registry::{REGISTRY, TaggedRegistryExt as _};
use uuid::Uuid;

use crate::entity::SharedEntity;
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::MemoryStore;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::sensor::{BrainSensor, SensorType};
use crate::entity::ai::sensing::nearest_visible_living::set_nearest_visible_entity;
use crate::entity::ai::sensing::villager::{closer_than, follow_range, is_entity_attackable};
use crate::world::World;

/// Vanilla `SensorType.FROG_ATTACKABLES`.
pub(crate) static FROG_ATTACKABLES: SensorType = SensorType::new(create, "frog_attackables");

/// Vanilla `SensorType.FROG_ATTACKABLES` factory.
pub(crate) fn create() -> Box<dyn BrainSensor> {
    Box::new(FrogAttackablesSensor)
}

/// Vanilla `FrogAttackablesSensor.TARGET_DETECTION_DISTANCE`, used verbatim by
/// `isMatchingEntity` as `mob.closerThan(body, 10.0)`.
pub(crate) const TARGET_DETECTION_DISTANCE: f64 = 10.0;

/// Vanilla `FrogAttackablesSensor`: writes the nearest visible entity the frog may
/// eat to `NEAREST_ATTACKABLE`.
pub(crate) struct FrogAttackablesSensor;

impl BrainSensor for FrogAttackablesSensor {
    /// Vanilla `FrogAttackablesSensor.requires`:
    /// `Sets.union(super.requires(), Set.of(UNREACHABLE_TONGUE_TARGETS))`, where
    /// `super.requires()` is `{getMemoryToSet(), NEAREST_VISIBLE_LIVING_ENTITIES}`.
    fn requires(&self) -> &'static [MemoryModuleTypeRef] {
        static REQUIRES: &[MemoryModuleTypeRef] = &[
            &mem::NEAREST_ATTACKABLE,
            &mem::NEAREST_VISIBLE_LIVING_ENTITIES,
            &mem::UNREACHABLE_TONGUE_TARGETS,
        ];
        REQUIRES
    }

    /// Vanilla `FrogAttackablesSensor.doTick` through
    /// `NearestVisibleLivingEntitySensor.doTick`.
    fn sense(&mut self, level: &World, entity: &SharedEntity, memories: &mut MemoryStore) {
        let Some(follow_range) = follow_range(entity) else {
            // Without a follow range no target can pass `isEntityAttackable`, so the
            // memory is erased (vanilla's `getAttributeValue` would throw instead).
            memories.erase(&mem::NEAREST_ATTACKABLE);
            return;
        };

        // Vanilla reads these from the body's brain inside the predicate; neither is
        // written during the scan, so they are hoisted out of the closure.
        let attack_target = memories.get(&mem::ATTACK_TARGET);
        let unreachable_targets = memories
            .get(&mem::UNREACHABLE_TONGUE_TARGETS)
            .unwrap_or_default();

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
                    &unreachable_targets,
                )
            },
        );
    }
}

/// Vanilla `FrogAttackablesSensor.isMatchingEntity`:
/// `isEntityAttackable(...) && Frog.canEat(mob) && !isUnreachableAttackTarget(...)`
/// and only then `mob.closerThan(body, 10.0)`.
#[must_use]
fn is_matching_entity(
    level: &World,
    body: &SharedEntity,
    mob: &SharedEntity,
    follow_range: f64,
    attack_target: Option<&SharedEntity>,
    unreachable_targets: &[Uuid],
) -> bool {
    is_entity_attackable(level, body, mob, follow_range, attack_target)
        && can_eat(mob)
        && !unreachable_targets.contains(&mob.uuid())
        && closer_than(mob, body, TARGET_DETECTION_DISTANCE)
}

/// Vanilla `Frog.canEat(LivingEntity)`.
///
/// Vanilla rejects an `AbstractCubeMob` whose `getSize() != 1` before the tag test.
/// Steel's slime and magma-cube entities do not expose a cube size yet, so that
/// guard is the one documented difference: every `minecraft:frog_food` member is
/// treated as edible regardless of its size.
#[must_use]
fn can_eat(mob: &SharedEntity) -> bool {
    REGISTRY
        .entity_types
        .is_in_tag(mob.entity_type(), &EntityTypeTag::FROG_FOOD)
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
        assert_eq!(
            keys[2],
            &Identifier::vanilla_static("unreachable_tongue_targets")
        );
    }

    #[test]
    fn scan_rate_is_the_vanilla_default() {
        // Vanilla `FrogAttackablesSensor()` uses the no-arg `Sensor()` constructor.
        assert_eq!(create().scan_rate(), DEFAULT_SCAN_RATE);
    }

    #[test]
    fn target_detection_distance_is_ten_blocks() {
        // Vanilla `isMatchingEntity` hardcodes `mob.closerThan(body, 10.0)`.
        assert_eq!(TARGET_DETECTION_DISTANCE, 10.0);
    }
}
