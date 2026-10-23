//! Vanilla `AdultSensor`.

use crate::entity::SharedEntity;
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::MemoryStore;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::sensor::{BrainSensor, SensorType};
use crate::world::World;

/// Vanilla `SensorType.NEAREST_ADULT`.
pub(crate) static NEAREST_ADULT: SensorType = SensorType::new(create, "nearest_adult");

/// Vanilla `SensorType.NEAREST_ADULT` factory.
pub(crate) fn create() -> Box<dyn BrainSensor> {
    Box::new(AdultSensor)
}

/// Vanilla `AdultSensor`: the nearest visible adult of the body's own entity type.
pub(crate) struct AdultSensor;

impl BrainSensor for AdultSensor {
    fn requires(&self) -> &'static [MemoryModuleTypeRef] {
        static REQUIRES: &[MemoryModuleTypeRef] = &[
            &mem::NEAREST_VISIBLE_ADULT,
            &mem::NEAREST_VISIBLE_LIVING_ENTITIES,
        ];
        REQUIRES
    }

    fn sense(&mut self, _level: &World, entity: &SharedEntity, memories: &mut MemoryStore) {
        let Some(visible) = memories.get(&mem::NEAREST_VISIBLE_LIVING_ENTITIES) else {
            // Vanilla `getMemory(...).ifPresent(...)`: without the visible-entity
            // cache the sensor leaves `NEAREST_VISIBLE_ADULT` untouched.
            return;
        };

        // Vanilla `setMemory(type, Optional)`: an empty result erases the memory.
        match visible.find_closest(same_type_adult(entity)) {
            Some(adult) => memories.set(&mem::NEAREST_VISIBLE_ADULT, adult),
            None => memories.erase(&mem::NEAREST_VISIBLE_ADULT),
        }
    }
}

/// Vanilla `entity.getType() == body.getType() && !entity.isBaby()`.
///
/// `FOLLOWABLE_FRIENDLY_MOBS` membership is not consulted here — that is
/// [`super::adult_any_type`]'s override; this predicate replicates vanilla exactly.
fn same_type_adult(body: &SharedEntity) -> impl Fn(&SharedEntity) -> bool {
    // Vanilla compares `getType()` identity; Steel compares the registry keys,
    // which are unique per entity type (the tree-wide idiom).
    let body_type_key = body.entity_type().key.clone();

    move |entity: &SharedEntity| {
        entity.entity_type().key == body_type_key
            && entity
                .as_living_entity()
                .is_some_and(|living| !living.is_baby())
    }
}
