//! Vanilla `SecondaryPoiSensor`.

use std::ptr;

use steel_registry::REGISTRY;
use steel_utils::GlobalPos;

use crate::entity::SharedEntity;
use crate::entity::ai::behavior::villager::villager;
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::MemoryStore;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::sensor::BrainSensor;
use crate::world::World;

/// Vanilla `SecondaryPoiSensor.SCAN_RATE`.
const SCAN_RATE: i64 = 40;
/// Vanilla `SecondaryPoiSensor.doTick`: `horizontalSearch = 4`.
const HORIZONTAL_SEARCH: i32 = 4;
/// Vanilla `SecondaryPoiSensor.doTick`: `verticalSearch = 2`.
const VERTICAL_SEARCH: i32 = 2;

/// Vanilla `SensorType.SECONDARY_POIS` factory.
pub(crate) fn create() -> Box<dyn BrainSensor> {
    Box::new(SecondaryPoiSensor)
}

/// Vanilla `SecondaryPoiSensor`.
pub(crate) struct SecondaryPoiSensor;

impl BrainSensor for SecondaryPoiSensor {
    fn requires(&self) -> &'static [MemoryModuleTypeRef] {
        static REQUIRES: &[MemoryModuleTypeRef] = &[&mem::SECONDARY_JOB_SITE];
        REQUIRES
    }

    fn scan_rate(&self) -> i64 {
        SCAN_RATE
    }

    fn sense(&mut self, level: &World, entity: &SharedEntity, memories: &mut MemoryStore) {
        let Some(villager) = villager(level, entity) else {
            return;
        };

        let profession = villager.profession();
        let dimension = level.key.clone();
        let center = entity.block_position();
        let mut job_sites: Vec<GlobalPos> = Vec::new();

        for x in -HORIZONTAL_SEARCH..=HORIZONTAL_SEARCH {
            for y in -VERTICAL_SEARCH..=VERTICAL_SEARCH {
                for z in -HORIZONTAL_SEARCH..=HORIZONTAL_SEARCH {
                    let test_pos = center.offset(x, y, z);
                    let state = level.get_block_state(test_pos);
                    let Some(block) = REGISTRY.blocks.by_state_id(state) else {
                        continue;
                    };

                    if profession
                        .secondary_poi
                        .iter()
                        .any(|secondary| ptr::eq(*secondary, block))
                    {
                        job_sites.push(GlobalPos::new(dimension.clone(), test_pos));
                    }
                }
            }
        }

        // Vanilla erases the memory when no secondary POI is in range.
        memories.set(&mem::SECONDARY_JOB_SITE, job_sites);
    }
}
