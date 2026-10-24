//! Vanilla `VillagerBabiesSensor`.

use steel_registry::vanilla_entities;

use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::MemoryStore;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::sensor::BrainSensor;
use crate::entity::{LivingEntity, SharedEntity};
use crate::world::World;

/// Vanilla `SensorType.VILLAGER_BABIES` factory.
pub(crate) fn create() -> Box<dyn BrainSensor> {
    Box::new(VillagerBabiesSensor)
}

/// Vanilla `VillagerBabiesSensor`.
pub(crate) struct VillagerBabiesSensor;

impl BrainSensor for VillagerBabiesSensor {
    fn requires(&self) -> &'static [MemoryModuleTypeRef] {
        static REQUIRES: &[MemoryModuleTypeRef] = &[&mem::VISIBLE_VILLAGER_BABIES];
        REQUIRES
    }

    fn sense(&mut self, _level: &World, _entity: &SharedEntity, memories: &mut MemoryStore) {
        let baby_villagers: Vec<SharedEntity> =
            match memories.get(&mem::NEAREST_VISIBLE_LIVING_ENTITIES) {
                Some(visible) => visible.find(is_villager_baby).cloned().collect(),
                None => Vec::new(),
            };

        // Vanilla writes the (possibly empty) list; an empty collection erases the
        // memory on write, like vanilla `Brain.isEmptyCollection`.
        memories.set(&mem::VISIBLE_VILLAGER_BABIES, baby_villagers);
    }
}

/// Vanilla `isVillagerBaby`: `entity.is(EntityTypes.VILLAGER) && entity.isBaby()`.
fn is_villager_baby(entity: &SharedEntity) -> bool {
    entity.entity_type().key == vanilla_entities::VILLAGER.key
        && entity.as_living_entity().is_some_and(LivingEntity::is_baby)
}
