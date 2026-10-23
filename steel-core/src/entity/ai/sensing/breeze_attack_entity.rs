//! Vanilla `BreezeAttackEntitySensor`.

use crate::entity::Entity as _;
use crate::entity::SharedEntity;
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::MemoryStore;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::sensor::{BrainSensor, SensorType};
use crate::entity::ai::sensing::villager::{
    follow_range, is_entity_attackable, scan_nearest_living_entities,
};
use crate::world::World;

/// Vanilla `SensorType.BREEZE_ATTACK_ENTITY_SENSOR`.
pub(crate) static BREEZE_ATTACK_ENTITY_SENSOR: SensorType =
    SensorType::new(create, "breeze_attack_entity_sensor");

/// Vanilla `SensorType.BREEZE_ATTACK_ENTITY_SENSOR` factory.
pub(crate) fn create() -> Box<dyn BrainSensor> {
    Box::new(BreezeAttackEntitySensor)
}

/// Vanilla `BreezeAttackEntitySensor`.
pub(crate) struct BreezeAttackEntitySensor;

impl BrainSensor for BreezeAttackEntitySensor {
    fn requires(&self) -> &'static [MemoryModuleTypeRef] {
        static REQUIRES: &[MemoryModuleTypeRef] = &[
            &mem::NEAREST_LIVING_ENTITIES,
            &mem::NEAREST_VISIBLE_LIVING_ENTITIES,
            &mem::NEAREST_ATTACKABLE,
        ];
        REQUIRES
    }

    fn sense(&mut self, level: &World, entity: &SharedEntity, memories: &mut MemoryStore) {
        // Vanilla `BreezeAttackEntitySensor.doTick`: `super.doTick` refreshes the
        // living caches first.
        scan_nearest_living_entities(level, entity, memories);

        let Some(follow_range) = follow_range(entity) else {
            // Without a follow range no entity can pass the targeting test, so
            // the memory is erased (vanilla's `getAttributeValue` would throw).
            memories.erase(&mem::NEAREST_ATTACKABLE);
            return;
        };
        let attack_target = memories.get(&mem::ATTACK_TARGET);

        // Vanilla filters `NEAREST_LIVING_ENTITIES` (ordered nearest-first) by
        // `NO_CREATIVE_OR_SPECTATOR` then `isEntityAttackable`, taking the first
        // match and erasing the memory when nothing matches.
        let attackable =
            memories
                .get(&mem::NEAREST_LIVING_ENTITIES)
                .and_then(|entities: Vec<SharedEntity>| {
                    entities.into_iter().find(|target| {
                        no_creative_or_spectator(target)
                            && is_entity_attackable(
                                level,
                                entity,
                                target,
                                follow_range,
                                attack_target.as_ref(),
                            )
                    })
                });
        match attackable {
            Some(target) => memories.set(&mem::NEAREST_ATTACKABLE, target),
            None => memories.erase(&mem::NEAREST_ATTACKABLE),
        }
    }
}

/// Vanilla `EntitySelector.NO_CREATIVE_OR_SPECTATOR`: players in creative or
/// spectator mode are excluded; non-players always pass.
#[must_use]
fn no_creative_or_spectator(target: &SharedEntity) -> bool {
    !target
        .as_player()
        .is_some_and(|player| player.has_infinite_materials() || player.is_spectator())
}
