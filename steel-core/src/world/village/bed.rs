//! Bed helpers shared with villagers (vanilla `AbstractBedBlock`).

use steel_registry::vanilla_entities;
use steel_utils::{BlockPos, WorldAabb};

use crate::world::World;

/// Wakes the villager sleeping in the bed at `pos`, returning whether one was woken
/// (vanilla `AbstractBedBlock.kickVillagerOutOfBed`).
///
/// Vanilla queries `Villager` instances whose `isSleeping` is set inside the bed's own
/// bounding box and stops the first one found; zombie villagers and other mobs are not
/// affected.
pub fn kick_villager_out_of_bed(world: &World, pos: BlockPos) -> bool {
    let bounds = WorldAabb::new(
        f64::from(pos.x()),
        f64::from(pos.y()),
        f64::from(pos.z()),
        f64::from(pos.x() + 1),
        f64::from(pos.y() + 1),
        f64::from(pos.z() + 1),
    );

    for entity in world.get_entities_in_aabb(&bounds) {
        if entity.entity_type() != &vanilla_entities::VILLAGER {
            continue;
        }
        let Some(living) = entity.as_living_entity() else {
            continue;
        };
        if !living.is_sleeping() {
            continue;
        }

        living.stop_sleeping();
        return true;
    }

    false
}
