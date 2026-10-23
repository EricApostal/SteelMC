//! Vanilla `GolemSensor`.

use steel_registry::vanilla_entities;

use crate::entity::SharedEntity;
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::MemoryStore;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::sensor::BrainSensor;
use crate::world::World;

/// Vanilla `GolemSensor.GOLEM_SCAN_RATE`.
const GOLEM_SCAN_RATE: i64 = 200;
/// Vanilla `GolemSensor.MEMORY_TIME_TO_LIVE`.
pub(crate) const MEMORY_TIME_TO_LIVE: i64 = 599;

/// Vanilla `SensorType.GOLEM_DETECTED` factory.
pub(crate) fn create() -> Box<dyn BrainSensor> {
    Box::new(GolemSensor)
}

/// Vanilla `GolemSensor`.
pub(crate) struct GolemSensor;

impl BrainSensor for GolemSensor {
    fn requires(&self) -> &'static [MemoryModuleTypeRef] {
        static REQUIRES: &[MemoryModuleTypeRef] =
            &[&mem::NEAREST_LIVING_ENTITIES, &mem::GOLEM_DETECTED_RECENTLY];
        REQUIRES
    }

    fn scan_rate(&self) -> i64 {
        GOLEM_SCAN_RATE
    }

    fn sense(&mut self, _level: &World, _entity: &SharedEntity, memories: &mut MemoryStore) {
        check_for_nearby_golem(memories);
    }
}

/// Vanilla `GolemSensor.checkForNearbyGolem`.
pub(crate) fn check_for_nearby_golem(memories: &mut MemoryStore) {
    let Some(living_entities) = memories.get(&mem::NEAREST_LIVING_ENTITIES) else {
        return;
    };

    if living_entities.iter().any(is_iron_golem) {
        golem_detected(memories);
    }
}

/// Vanilla `GolemSensor.golemDetected`.
pub(crate) fn golem_detected(memories: &mut MemoryStore) {
    memories.set_with_expiry(&mem::GOLEM_DETECTED_RECENTLY, true, MEMORY_TIME_TO_LIVE);
}

/// Vanilla `entity.is(EntityTypes.IRON_GOLEM)`.
fn is_iron_golem(entity: &SharedEntity) -> bool {
    entity.entity_type().key == vanilla_entities::IRON_GOLEM.key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golem_detected_memory_lasts_599_ticks() {
        let mut memories = MemoryStore::new();
        memories.register(&mem::GOLEM_DETECTED_RECENTLY);

        golem_detected(&mut memories);

        assert_eq!(memories.get(&mem::GOLEM_DETECTED_RECENTLY), Some(true));
        assert_eq!(
            memories.time_to_live(&mem::GOLEM_DETECTED_RECENTLY),
            MEMORY_TIME_TO_LIVE
        );
    }

    #[test]
    fn check_for_nearby_golem_without_the_memory_does_nothing() {
        let mut memories = MemoryStore::new();
        memories.register(&mem::NEAREST_LIVING_ENTITIES);
        memories.register(&mem::GOLEM_DETECTED_RECENTLY);

        check_for_nearby_golem(&mut memories);

        assert!(!memories.has_value(&mem::GOLEM_DETECTED_RECENTLY));
    }
}
