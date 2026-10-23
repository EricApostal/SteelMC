//! Vanilla `VillagerHostilesSensor` and its 11-entry hostile table.

use steel_registry::entity_type::EntityTypeRef;
use steel_registry::vanilla_entities;

use crate::entity::SharedEntity;
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::MemoryStore;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::memory::nearest_visible::NearestVisibleLivingEntities;
use crate::entity::ai::brain::sensor::BrainSensor;
use crate::world::World;

/// Vanilla `SensorType.VILLAGER_HOSTILES` factory.
pub(crate) fn create() -> Box<dyn BrainSensor> {
    Box::new(VillagerHostilesSensor)
}

/// Vanilla `VillagerHostilesSensor`.
pub(crate) struct VillagerHostilesSensor;

impl BrainSensor for VillagerHostilesSensor {
    fn requires(&self) -> &'static [MemoryModuleTypeRef] {
        static REQUIRES: &[MemoryModuleTypeRef] =
            &[&mem::NEAREST_HOSTILE, &mem::NEAREST_VISIBLE_LIVING_ENTITIES];
        REQUIRES
    }

    fn sense(&mut self, _level: &World, entity: &SharedEntity, memories: &mut MemoryStore) {
        match nearest_hostile(entity, memories) {
            Some(hostile) => memories.set(&mem::NEAREST_HOSTILE, hostile),
            None => memories.erase(&mem::NEAREST_HOSTILE),
        }
    }
}

/// Vanilla `VillagerHostilesSensor.ACCEPTABLE_DISTANCE_FROM_HOSTILES`: entity type
/// to detection distance in blocks, tested with `distSqr <= distance * distance`.
static ACCEPTABLE_DISTANCE_FROM_HOSTILES: &[(EntityTypeRef, f64)] = &[
    (&vanilla_entities::DROWNED, 8.0),
    (&vanilla_entities::EVOKER, 12.0),
    (&vanilla_entities::HUSK, 8.0),
    (&vanilla_entities::ILLUSIONER, 12.0),
    (&vanilla_entities::PILLAGER, 15.0),
    (&vanilla_entities::RAVAGER, 12.0),
    (&vanilla_entities::VEX, 8.0),
    (&vanilla_entities::VINDICATOR, 10.0),
    (&vanilla_entities::ZOGLIN, 10.0),
    (&vanilla_entities::ZOMBIE, 8.0),
    (&vanilla_entities::ZOMBIE_VILLAGER, 8.0),
];

/// Vanilla `ACCEPTABLE_DISTANCE_FROM_HOSTILES.get(type)`: `None` is `isHostile` false.
#[must_use]
pub fn hostile_distance(entity_type: EntityTypeRef) -> Option<f64> {
    ACCEPTABLE_DISTANCE_FROM_HOSTILES
        .iter()
        .find(|(hostile_type, _)| hostile_type.key == entity_type.key)
        .map(|(_, distance)| *distance)
}

/// Vanilla `NearestVisibleLivingEntities.findClosest(mob -> isMatchingEntity(...))`.
///
/// `NEAREST_VISIBLE_LIVING_ENTITIES` is ordered nearest-first, so the first
/// matching entry is the nearest hostile in range.
fn nearest_hostile(body: &SharedEntity, memories: &MemoryStore) -> Option<SharedEntity> {
    let visible: NearestVisibleLivingEntities =
        memories.get(&mem::NEAREST_VISIBLE_LIVING_ENTITIES)?;

    visible.find_closest(|mob| {
        hostile_distance(mob.entity_type())
            .is_some_and(|distance| body.distance_to_sqr(mob.position()) <= distance * distance)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_table_matches_vanilla_distances() {
        let cases: &[(EntityTypeRef, f64)] = &[
            (&vanilla_entities::DROWNED, 8.0),
            (&vanilla_entities::EVOKER, 12.0),
            (&vanilla_entities::HUSK, 8.0),
            (&vanilla_entities::ILLUSIONER, 12.0),
            (&vanilla_entities::PILLAGER, 15.0),
            (&vanilla_entities::RAVAGER, 12.0),
            (&vanilla_entities::VEX, 8.0),
            (&vanilla_entities::VINDICATOR, 10.0),
            (&vanilla_entities::ZOGLIN, 10.0),
            (&vanilla_entities::ZOMBIE, 8.0),
            (&vanilla_entities::ZOMBIE_VILLAGER, 8.0),
        ];

        assert_eq!(cases.len(), 11);
        assert_eq!(ACCEPTABLE_DISTANCE_FROM_HOSTILES.len(), cases.len());

        for (entity_type, distance) in cases {
            assert_eq!(
                hostile_distance(entity_type),
                Some(*distance),
                "hostile distance for {}",
                entity_type.key
            );
        }

        assert_eq!(hostile_distance(&vanilla_entities::VILLAGER), None);
        assert_eq!(hostile_distance(&vanilla_entities::IRON_GOLEM), None);
    }
}
