//! Vanilla `PiglinBruteSpecificSensor`.

use steel_registry::vanilla_entities;

use crate::entity::SharedEntity;
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::MemoryStore;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::memory::nearest_visible::NearestVisibleLivingEntities;
use crate::entity::ai::brain::sensor::{BrainSensor, SensorType};
use crate::entity::ai::sensing::nearest_visible_living::get_visible_entities;
use crate::entity::ai::sensing::piglin_specific::find_nearby_adult_piglins;
use crate::world::World;

/// Vanilla `SensorType.PIGLIN_BRUTE_SPECIFIC_SENSOR`.
pub(crate) static PIGLIN_BRUTE_SPECIFIC_SENSOR: SensorType =
    SensorType::new(create, "piglin_brute_specific_sensor");

/// Vanilla `SensorType.PIGLIN_BRUTE_SPECIFIC_SENSOR` factory.
pub(crate) fn create() -> Box<dyn BrainSensor> {
    Box::new(PiglinBruteSpecificSensor)
}

/// Vanilla `PiglinBruteSpecificSensor`: the nearest visible wither skeleton or
/// wither, plus the nearby adult piglins.
pub(crate) struct PiglinBruteSpecificSensor;

impl BrainSensor for PiglinBruteSpecificSensor {
    fn requires(&self) -> &'static [MemoryModuleTypeRef] {
        static REQUIRES: &[MemoryModuleTypeRef] = &[
            &mem::NEAREST_VISIBLE_LIVING_ENTITIES,
            &mem::NEAREST_VISIBLE_NEMESIS,
            &mem::NEARBY_ADULT_PIGLINS,
        ];
        REQUIRES
    }

    fn sense(&mut self, _level: &World, _body: &SharedEntity, memories: &mut MemoryStore) {
        let visible =
            get_visible_entities(memories).unwrap_or_else(NearestVisibleLivingEntities::empty);

        // Vanilla `visibleLivingEntities.findClosest(entity -> entity instanceof
        // WitherSkeleton || entity instanceof WitherBoss)`.
        let nemesis = visible.find_closest(is_wither_family);

        // Vanilla `PiglinAi.findNearbyAdultPiglins(brain)`.
        let adult_piglins = find_nearby_adult_piglins(memories);

        // Vanilla `brain.setMemory(type, Optional)`: an empty optional erases the memory.
        match nemesis {
            Some(nemesis) => memories.set(&mem::NEAREST_VISIBLE_NEMESIS, nemesis),
            None => memories.erase(&mem::NEAREST_VISIBLE_NEMESIS),
        }
        memories.set(&mem::NEARBY_ADULT_PIGLINS, adult_piglins);
    }
}

/// Vanilla `entity instanceof WitherSkeleton || entity instanceof WitherBoss`.
fn is_wither_family(entity: &SharedEntity) -> bool {
    entity.entity_type().key == vanilla_entities::WITHER_SKELETON.key
        || entity.entity_type().key == vanilla_entities::WITHER.key
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Weak};

    use glam::DVec3;
    use steel_registry::init_vanilla_registry;

    use super::*;
    use crate::entity::entities::PigEntity;

    #[test]
    fn is_wither_family_matches_only_wither_mobs() {
        init_vanilla_registry();
        let wither: SharedEntity = Arc::new(PigEntity::new(
            &vanilla_entities::WITHER,
            2,
            DVec3::ZERO,
            Weak::new(),
        ));
        let pig: SharedEntity = Arc::new(PigEntity::new(
            &vanilla_entities::PIG,
            3,
            DVec3::ZERO,
            Weak::new(),
        ));

        assert!(is_wither_family(&wither));
        assert!(!is_wither_family(&pig));
    }

    #[test]
    fn requires_matches_vanilla() {
        let sensor = PiglinBruteSpecificSensor;
        let keys: Vec<_> = sensor
            .requires()
            .iter()
            .map(|memory| memory.key().clone())
            .collect();

        assert_eq!(
            keys,
            vec![
                mem::NEAREST_VISIBLE_LIVING_ENTITIES.key().clone(),
                mem::NEAREST_VISIBLE_NEMESIS.key().clone(),
                mem::NEARBY_ADULT_PIGLINS.key().clone(),
            ]
        );
    }
}
