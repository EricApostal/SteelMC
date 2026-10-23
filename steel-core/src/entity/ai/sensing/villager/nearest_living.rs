//! Vanilla `NearestLivingEntitySensor` and its `NearestVisibleLivingEntitySensor`
//! base semantics.

use crate::entity::SharedEntity;
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::MemoryStore;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::memory::nearest_visible::NearestVisibleLivingEntities;
use crate::entity::ai::brain::sensor::BrainSensor;
use crate::entity::ai::sensing::villager::{follow_range, is_entity_targetable};
use crate::world::World;

/// Vanilla `SensorType.NEAREST_LIVING_ENTITIES` factory.
pub(crate) fn create() -> Box<dyn BrainSensor> {
    Box::new(NearestLivingEntitySensor)
}

/// Vanilla `NearestLivingEntitySensor`.
pub(crate) struct NearestLivingEntitySensor;

impl BrainSensor for NearestLivingEntitySensor {
    fn requires(&self) -> &'static [MemoryModuleTypeRef] {
        static REQUIRES: &[MemoryModuleTypeRef] = &[
            &mem::NEAREST_LIVING_ENTITIES,
            &mem::NEAREST_VISIBLE_LIVING_ENTITIES,
        ];
        REQUIRES
    }

    fn sense(&mut self, level: &World, entity: &SharedEntity, memories: &mut MemoryStore) {
        let Some(follow_range) = follow_range(entity) else {
            return;
        };

        let aabb = entity
            .bounding_box()
            .inflate_xyz(follow_range, follow_range, follow_range);
        let body_id = entity.id();
        let mut living_entities = level.get_entities_in_aabb_matching(&aabb, |mob| {
            mob.id() != body_id && mob.is_alive() && mob.as_living_entity().is_some()
        });
        living_entities.sort_by(|first, second| {
            entity
                .distance_to_sqr(first.position())
                .total_cmp(&entity.distance_to_sqr(second.position()))
        });

        // Vanilla `Sensor.isEntityTargetable` is evaluated lazily against the live
        // brain; the nearest-visible cache cannot reach the memory store, so the
        // `ATTACK_TARGET` snapshot taken here stands in for it.
        let attack_target = memories.get(&mem::ATTACK_TARGET);
        let visible_entities = NearestVisibleLivingEntities::new(
            living_entities.clone(),
            visible_test(entity, follow_range, attack_target),
        );

        memories.set(&mem::NEAREST_LIVING_ENTITIES, living_entities);
        memories.set(&mem::NEAREST_VISIBLE_LIVING_ENTITIES, visible_entities);
    }
}

/// Vanilla `NearestVisibleLivingEntities` line-of-sight test (non-combat targeting).
fn visible_test(
    body: &SharedEntity,
    follow_range: f64,
    attack_target: Option<SharedEntity>,
) -> impl Fn(&SharedEntity) -> bool + Send + Sync + 'static {
    let world = body.level();
    let body = body.clone();

    move |target: &SharedEntity| {
        let Some(world) = world.as_deref() else {
            return false;
        };

        is_entity_targetable(world, &body, target, follow_range, attack_target.as_ref())
    }
}
