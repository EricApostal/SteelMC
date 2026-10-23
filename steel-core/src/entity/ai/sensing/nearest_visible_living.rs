//! Vanilla `NearestVisibleLivingEntitySensor`: the shared base of the sensors that
//! pick one entity out of the `NEAREST_VISIBLE_LIVING_ENTITIES` cache.
//!
//! Vanilla subclasses implement `isMatchingEntity(level, body, mob)` and
//! `getMemoryToSet()`; [`set_nearest_visible_entity`] is that template in function
//! form. This module exposes no `SensorType` — the concrete sensors do.

use crate::entity::SharedEntity;
use crate::entity::ai::brain::memory::MemoryModuleType;
use crate::entity::ai::brain::memory::MemoryStore;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::memory::nearest_visible::NearestVisibleLivingEntities;
use crate::world::World;

/// Vanilla `NearestVisibleLivingEntitySensor.getVisibleEntities(body)`.
#[must_use]
pub(crate) fn get_visible_entities(memories: &MemoryStore) -> Option<NearestVisibleLivingEntities> {
    memories.get(&mem::NEAREST_VISIBLE_LIVING_ENTITIES)
}

/// Vanilla `NearestVisibleLivingEntitySensor.getNearestEntity`:
/// `getVisibleEntities(body).flatMap(entities -> entities.findClosest(isMatchingEntity))`.
#[must_use]
pub(crate) fn nearest_visible_entity(
    level: &World,
    body: &SharedEntity,
    memories: &MemoryStore,
    is_matching: impl Fn(&World, &SharedEntity, &SharedEntity) -> bool,
) -> Option<SharedEntity> {
    get_visible_entities(memories)?.find_closest(|mob| is_matching(level, body, mob))
}

/// Vanilla `NearestVisibleLivingEntitySensor.doTick`:
/// `brain.setMemory(getMemoryToSet(), getNearestEntity(...))`, where an empty
/// `Optional` erases the memory.
pub(crate) fn set_nearest_visible_entity(
    level: &World,
    body: &SharedEntity,
    memories: &mut MemoryStore,
    memory: &MemoryModuleType<SharedEntity>,
    is_matching: impl Fn(&World, &SharedEntity, &SharedEntity) -> bool,
) {
    match nearest_visible_entity(level, body, memories, is_matching) {
        Some(entity) => memories.set(memory, entity),
        None => memories.erase(memory),
    }
}
