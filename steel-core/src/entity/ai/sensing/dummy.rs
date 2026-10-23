//! Vanilla `DummySensor`.

use crate::entity::SharedEntity;
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::MemoryStore;
use crate::entity::ai::brain::sensor::{BrainSensor, SensorType};
use crate::world::World;

/// Vanilla `SensorType.DUMMY`.
pub(crate) static DUMMY: SensorType = SensorType::new(create, "dummy");

/// Vanilla `SensorType.DUMMY` factory.
pub(crate) fn create() -> Box<dyn BrainSensor> {
    Box::new(DummySensor)
}

/// Vanilla `DummySensor`: a `requires()`-free no-op, used as a placeholder where a
/// brain provider wants a sensor slot that does nothing this version.
pub(crate) struct DummySensor;

impl BrainSensor for DummySensor {
    fn requires(&self) -> &'static [MemoryModuleTypeRef] {
        static REQUIRES: &[MemoryModuleTypeRef] = &[];
        REQUIRES
    }

    fn sense(&mut self, _level: &World, _entity: &SharedEntity, _memories: &mut MemoryStore) {}
}
