//! Vanilla `HoglinSpecificSensor`.

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_entities;
use steel_utils::BlockPos;

use crate::entity::SharedEntity;
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::MemoryStore;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::memory::nearest_visible::NearestVisibleLivingEntities;
use crate::entity::ai::brain::sensor::{BrainSensor, SensorType};
use crate::entity::ai::sensing::nearest_visible_living::get_visible_entities;
use crate::entity::ai::sensing::villager::{find_nearest_block_by_manhattan, is_adult};
use crate::world::World;

/// Vanilla `HoglinSpecificSensor` repellent search: `8` blocks horizontally.
const REPELLENT_SEARCH_RANGE_XZ: i32 = 8;
/// Vanilla `HoglinSpecificSensor` repellent search: `4` blocks vertically.
const REPELLENT_SEARCH_RANGE_Y: i32 = 4;

/// Vanilla `SensorType.HOGLIN_SPECIFIC_SENSOR`.
pub(crate) static HOGLIN_SPECIFIC_SENSOR: SensorType =
    SensorType::new(create, "hoglin_specific_sensor");

/// Vanilla `SensorType.HOGLIN_SPECIFIC_SENSOR` factory.
pub(crate) fn create() -> Box<dyn BrainSensor> {
    Box::new(HoglinSpecificSensor)
}

/// Vanilla `HoglinSpecificSensor`: caches the nearest `HOGLIN_REPELLENTS` block and
/// the visible adult piglins/hoglins plus their counts.
pub(crate) struct HoglinSpecificSensor;

impl BrainSensor for HoglinSpecificSensor {
    fn requires(&self) -> &'static [MemoryModuleTypeRef] {
        static REQUIRES: &[MemoryModuleTypeRef] = &[
            &mem::NEAREST_VISIBLE_LIVING_ENTITIES,
            &mem::NEAREST_REPELLENT,
            &mem::NEAREST_VISIBLE_ADULT_PIGLIN,
            &mem::NEAREST_VISIBLE_ADULT_HOGLINS,
            &mem::VISIBLE_ADULT_PIGLIN_COUNT,
            &mem::VISIBLE_ADULT_HOGLIN_COUNT,
        ];
        REQUIRES
    }

    fn sense(&mut self, level: &World, entity: &SharedEntity, memories: &mut MemoryStore) {
        // Vanilla `brain.setMemory(NEAREST_REPELLENT, findNearestRepellent(level, body))`:
        // an empty `Optional` erases the memory.
        match find_nearest_repellent(level, entity) {
            Some(pos) => memories.set(&mem::NEAREST_REPELLENT, pos),
            None => memories.erase(&mem::NEAREST_REPELLENT),
        }

        // Vanilla `getMemory(NEAREST_VISIBLE_LIVING_ENTITIES).orElse(empty())`.
        let visible_entities =
            get_visible_entities(memories).unwrap_or_else(NearestVisibleLivingEntities::empty);

        // Vanilla `visibleLivingEntities.findAll(entity -> !entity.isBaby()
        // && (entity instanceof Piglin || entity instanceof Hoglin))`. An entity
        // cannot be both a piglin and a hoglin, so the two vanilla `if`s are
        // mutually exclusive.
        let mut adult_piglin: Option<SharedEntity> = None;
        let mut adult_piglin_count = 0i32;
        let mut adult_hoglins: Vec<SharedEntity> = Vec::new();
        for candidate in visible_entities
            .find(|candidate| is_adult(candidate) && (is_piglin(candidate) || is_hoglin(candidate)))
        {
            if is_piglin(candidate) {
                adult_piglin_count += 1;
                if adult_piglin.is_none() {
                    adult_piglin = Some(candidate.clone());
                }
            } else if is_hoglin(candidate) {
                adult_hoglins.push(candidate.clone());
            }
        }

        // Vanilla `brain.setMemory(NEAREST_VISIBLE_ADULT_PIGLIN, adultPiglin)`.
        match adult_piglin {
            Some(piglin) => memories.set(&mem::NEAREST_VISIBLE_ADULT_PIGLIN, piglin),
            None => memories.erase(&mem::NEAREST_VISIBLE_ADULT_PIGLIN),
        }
        // Vanilla `brain.setMemory(NEAREST_VISIBLE_ADULT_HOGLINS, adultHoglins)`;
        // Steel's `set` erases empty collections like vanilla `isEmptyCollection`.
        let adult_hoglin_count = adult_hoglins.len() as i32;
        memories.set(&mem::NEAREST_VISIBLE_ADULT_HOGLINS, adult_hoglins);
        memories.set(&mem::VISIBLE_ADULT_PIGLIN_COUNT, adult_piglin_count);
        memories.set(&mem::VISIBLE_ADULT_HOGLIN_COUNT, adult_hoglin_count);
    }
}

/// Vanilla `HoglinSpecificSensor.findNearestRepellent`: the first block within `8`
/// blocks horizontally and `4` vertically carrying the `HOGLIN_REPELLENTS` tag.
fn find_nearest_repellent(level: &World, body: &SharedEntity) -> Option<BlockPos> {
    find_nearest_block_by_manhattan(
        level,
        body.block_position(),
        REPELLENT_SEARCH_RANGE_XZ,
        REPELLENT_SEARCH_RANGE_Y,
        |state| state.get_block().has_tag(&BlockTag::HOGLIN_REPELLENTS),
    )
}

/// Vanilla `entity instanceof Piglin`; Steel compares the registered entity type.
fn is_piglin(entity: &SharedEntity) -> bool {
    entity.entity_type().key == vanilla_entities::PIGLIN.key
}

/// Vanilla `entity instanceof Hoglin`; Steel compares the registered entity type.
fn is_hoglin(entity: &SharedEntity) -> bool {
    entity.entity_type().key == vanilla_entities::HOGLIN.key
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::ai::brain::sensor::DEFAULT_SCAN_RATE;

    #[test]
    fn requires_lists_all_six_vanilla_memories() {
        let sensor = create();
        let requires = sensor.requires();

        assert_eq!(requires.len(), 6);
        for expected in [
            &mem::NEAREST_VISIBLE_LIVING_ENTITIES as MemoryModuleTypeRef,
            &mem::NEAREST_REPELLENT as MemoryModuleTypeRef,
            &mem::NEAREST_VISIBLE_ADULT_PIGLIN as MemoryModuleTypeRef,
            &mem::NEAREST_VISIBLE_ADULT_HOGLINS as MemoryModuleTypeRef,
            &mem::VISIBLE_ADULT_PIGLIN_COUNT as MemoryModuleTypeRef,
            &mem::VISIBLE_ADULT_HOGLIN_COUNT as MemoryModuleTypeRef,
        ] {
            assert!(
                requires.iter().any(|actual| actual.key() == expected.key()),
                "requires should list {}",
                expected.key()
            );
        }
    }

    #[test]
    fn uses_the_default_scan_rate() {
        assert_eq!(create().scan_rate(), DEFAULT_SCAN_RATE);
    }
}
