//! Vanilla `IsInWaterSensor`.

use crate::entity::SharedEntity;
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::MemoryStore;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::sensor::{BrainSensor, SensorType};
use crate::world::World;

/// Vanilla `SensorType.IS_IN_WATER`.
pub(crate) static IS_IN_WATER: SensorType = SensorType::new(create, "is_in_water");

/// Vanilla `SensorType.IS_IN_WATER` factory.
pub(crate) fn create() -> Box<dyn BrainSensor> {
    Box::new(IsInWaterSensor)
}

/// Vanilla `IsInWaterSensor`: mirrors `LivingEntity.isInWater()` into the
/// `IS_IN_WATER` unit memory, erasing it when the body leaves the water.
pub(crate) struct IsInWaterSensor;

impl BrainSensor for IsInWaterSensor {
    fn requires(&self) -> &'static [MemoryModuleTypeRef] {
        static REQUIRES: &[MemoryModuleTypeRef] = &[&mem::IS_IN_WATER];
        REQUIRES
    }

    fn sense(&mut self, _level: &World, entity: &SharedEntity, memories: &mut MemoryStore) {
        if entity.is_in_water() {
            memories.set(&mem::IS_IN_WATER, ());
        } else {
            memories.erase(&mem::IS_IN_WATER);
        }
    }
}
