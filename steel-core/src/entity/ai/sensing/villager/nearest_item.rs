//! Vanilla `NearestItemSensor`.

use steel_utils::Downcast as _;

use crate::entity::SharedEntity;
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::MemoryStore;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::sensor::BrainSensor;
use crate::entity::ai::sensing::villager::closer_than;
use crate::entity::entities::ItemEntity;
use crate::world::World;

/// Vanilla `NearestItemSensor.XZ_RANGE`.
const XZ_RANGE: f64 = 32.0;
/// Vanilla `NearestItemSensor.Y_RANGE`.
const Y_RANGE: f64 = 16.0;
/// Vanilla `NearestItemSensor.MAX_DISTANCE_TO_WANTED_ITEM`.
pub(crate) const MAX_DISTANCE_TO_WANTED_ITEM: f64 = 32.0;

/// Vanilla `SensorType.NEAREST_ITEMS` factory.
pub(crate) fn create() -> Box<dyn BrainSensor> {
    Box::new(NearestItemSensor)
}

/// Vanilla `NearestItemSensor`.
pub(crate) struct NearestItemSensor;

impl BrainSensor for NearestItemSensor {
    fn requires(&self) -> &'static [MemoryModuleTypeRef] {
        static REQUIRES: &[MemoryModuleTypeRef] = &[&mem::NEAREST_VISIBLE_WANTED_ITEM];
        REQUIRES
    }

    fn sense(&mut self, level: &World, entity: &SharedEntity, memories: &mut MemoryStore) {
        let Some(mob) = entity.as_mob() else {
            return;
        };

        let aabb = entity
            .bounding_box()
            .inflate_xyz(XZ_RANGE, Y_RANGE, XZ_RANGE);
        let mut items = level.get_entities_in_aabb_matching(&aabb, |item| {
            item.downcast_ref::<ItemEntity>().is_some()
        });
        items.sort_by(|first, second| {
            entity
                .distance_to_sqr(first.position())
                .total_cmp(&entity.distance_to_sqr(second.position()))
        });

        let wanted_item = items
            .iter()
            .find(|candidate| {
                let item: &SharedEntity = candidate;
                let Some(item_entity) = item.downcast_ref::<ItemEntity>() else {
                    return false;
                };

                mob.wants_to_pick_up(&item_entity.get_item())
                    && closer_than(item, entity, MAX_DISTANCE_TO_WANTED_ITEM)
                    && entity
                        .as_living_entity()
                        .is_some_and(|living| living.has_line_of_sight(item.as_ref()))
            })
            .cloned();

        match wanted_item {
            Some(item) => memories.set(&mem::NEAREST_VISIBLE_WANTED_ITEM, item),
            None => memories.erase(&mem::NEAREST_VISIBLE_WANTED_ITEM),
        }
    }
}
