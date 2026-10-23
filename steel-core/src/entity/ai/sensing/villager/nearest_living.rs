//! Vanilla `NearestLivingEntitySensor` and its `NearestVisibleLivingEntitySensor`
//! base semantics.

use crate::entity::SharedEntity;
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::MemoryStore;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::sensor::BrainSensor;
use crate::entity::ai::sensing::villager::scan_nearest_living_entities;
use crate::world::World;

/// Vanilla `SensorType.NEAREST_LIVING_ENTITIES` factory.
pub(crate) fn create() -> Box<dyn BrainSensor> {
    Box::new(NearestLivingEntitySensor)
}

/// Vanilla `NearestLivingEntitySensor`.
pub(crate) struct NearestLivingEntitySensor;

impl BrainSensor for NearestLivingEntitySensor {
    fn requires(&self) -> &'static [MemoryModuleTypeRef] {
        static REQUIRES: &[MemoryModuleTypeRef] = &[
            &mem::NEAREST_LIVING_ENTITIES,
            &mem::NEAREST_VISIBLE_LIVING_ENTITIES,
        ];
        REQUIRES
    }

    fn sense(&mut self, level: &World, entity: &SharedEntity, memories: &mut MemoryStore) {
        scan_nearest_living_entities(level, entity, memories);
    }
}
