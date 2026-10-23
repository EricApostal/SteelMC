//! Vanilla `AdultSensorAnyType`.

use steel_registry::REGISTRY;
use steel_registry::TaggedRegistryExt as _;
use steel_registry::vanilla_entity_type_tags::EntityTypeTag;

use crate::entity::SharedEntity;
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::MemoryStore;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::sensor::{BrainSensor, SensorType};
use crate::world::World;

/// Vanilla `SensorType.NEAREST_ADULT_ANY_TYPE`.
pub(crate) static NEAREST_ADULT_ANY_TYPE: SensorType =
    SensorType::new(create, "nearest_adult_any_type");

/// Vanilla `SensorType.NEAREST_ADULT_ANY_TYPE` factory.
pub(crate) fn create() -> Box<dyn BrainSensor> {
    Box::new(AdultSensorAnyType)
}

/// Vanilla `AdultSensorAnyType` (extends `AdultSensor`): the nearest visible adult
/// among any `EntityTypeTags.FOLLOWABLE_FRIENDLY_MOBS` type rather than the body's
/// own type. `requires()` is inherited unchanged from `AdultSensor`.
pub(crate) struct AdultSensorAnyType;

impl BrainSensor for AdultSensorAnyType {
    fn requires(&self) -> &'static [MemoryModuleTypeRef] {
        static REQUIRES: &[MemoryModuleTypeRef] = &[
            &mem::NEAREST_VISIBLE_ADULT,
            &mem::NEAREST_VISIBLE_LIVING_ENTITIES,
        ];
        REQUIRES
    }

    fn sense(&mut self, _level: &World, _entity: &SharedEntity, memories: &mut MemoryStore) {
        let Some(visible) = memories.get(&mem::NEAREST_VISIBLE_LIVING_ENTITIES) else {
            // Vanilla `getMemory(...).ifPresent(...)`: without the visible-entity
            // cache the sensor leaves `NEAREST_VISIBLE_ADULT` untouched.
            return;
        };

        // Vanilla `setMemory(type, Optional)`: an empty result erases the memory.
        match visible.find_closest(is_followable_friendly_adult) {
            Some(adult) => memories.set(&mem::NEAREST_VISIBLE_ADULT, adult),
            None => memories.erase(&mem::NEAREST_VISIBLE_ADULT),
        }
    }
}

/// Vanilla `entity.is(EntityTypeTags.FOLLOWABLE_FRIENDLY_MOBS) && !entity.isBaby()`.
fn is_followable_friendly_adult(entity: &SharedEntity) -> bool {
    REGISTRY.entity_types.is_in_tag(
        entity.entity_type(),
        &EntityTypeTag::FOLLOWABLE_FRIENDLY_MOBS,
    ) && entity
        .as_living_entity()
        .is_some_and(|living| !living.is_baby())
}
