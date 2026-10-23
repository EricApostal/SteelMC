//! Vanilla `HurtBySensor`.

use crate::entity::SharedEntity;
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::MemoryStore;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::sensor::BrainSensor;
use crate::entity::damage::{DamageSource, RecentDamageSource};
use crate::world::World;

/// Vanilla `SensorType.HURT_BY` factory.
pub(crate) fn create() -> Box<dyn BrainSensor> {
    Box::new(HurtBySensor)
}

/// Vanilla `HurtBySensor`.
pub(crate) struct HurtBySensor;

impl BrainSensor for HurtBySensor {
    fn requires(&self) -> &'static [MemoryModuleTypeRef] {
        static REQUIRES: &[MemoryModuleTypeRef] = &[&mem::HURT_BY, &mem::HURT_BY_ENTITY];
        REQUIRES
    }

    fn sense(&mut self, level: &World, entity: &SharedEntity, memories: &mut MemoryStore) {
        let Some(living) = entity.as_living_entity() else {
            return;
        };

        match living.last_damage_source() {
            Some(recent) => {
                memories.set(&mem::HURT_BY, to_damage_source(&recent));

                if let Some(causing) = recent.causing_entity()
                    && causing.as_living_entity().is_some()
                {
                    memories.set(&mem::HURT_BY_ENTITY, causing.clone());
                }
            }
            None => memories.erase(&mem::HURT_BY),
        }

        if let Some(hurt_by_entity) = memories.get(&mem::HURT_BY_ENTITY)
            && (!hurt_by_entity.is_alive()
                || !hurt_by_entity
                    .level()
                    .is_some_and(|world| world.key == level.key))
        {
            memories.erase(&mem::HURT_BY_ENTITY);
        }
    }
}

/// Vanilla `DamageSource` carries the damage type and its (still retained)
/// entities; `RecentDamageSource` is the history read, so rebuild the source the
/// behaviors expect in `HURT_BY`.
fn to_damage_source(recent: &RecentDamageSource) -> DamageSource {
    let mut source = DamageSource::environment(recent.damage_type);
    if let Some(causing) = recent.causing_entity() {
        source = source.with_causing_entity(causing.clone());
    }
    if let Some(direct) = recent.direct_entity() {
        source = source.with_direct_entity(direct.clone());
    }
    if let Some(position) = recent.source_position_raw() {
        source = source.with_source_position(position);
    }
    source
}
