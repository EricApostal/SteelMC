//! Vanilla `WardenEntitySensor`.

use std::sync::Arc;

use steel_registry::vanilla_entities;

use crate::entity::Entity as _;
use crate::entity::LivingEntity;
use crate::entity::SharedEntity;
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::MemoryStore;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::sensor::{BrainSensor, SensorType};
use crate::entity::ai::sensing::villager::scan_nearest_living_entities;
use crate::world::World;

/// Vanilla `SensorType.WARDEN_ENTITY_SENSOR`.
pub(crate) static WARDEN_ENTITY_SENSOR: SensorType =
    SensorType::new(create, "warden_entity_sensor");

/// Vanilla `SensorType.WARDEN_ENTITY_SENSOR` factory.
pub(crate) fn create() -> Box<dyn BrainSensor> {
    Box::new(WardenEntitySensor)
}

/// Vanilla `WardenEntitySensor`.
pub(crate) struct WardenEntitySensor;

impl BrainSensor for WardenEntitySensor {
    fn requires(&self) -> &'static [MemoryModuleTypeRef] {
        static REQUIRES: &[MemoryModuleTypeRef] = &[
            &mem::NEAREST_LIVING_ENTITIES,
            &mem::NEAREST_VISIBLE_LIVING_ENTITIES,
            &mem::NEAREST_ATTACKABLE,
        ];
        REQUIRES
    }

    fn sense(&mut self, level: &World, entity: &SharedEntity, memories: &mut MemoryStore) {
        // Vanilla `WardenEntitySensor.doTick`: `super.doTick` refreshes the living
        // caches first, then players are preferred over everything else.
        scan_nearest_living_entities(level, entity, memories);

        let attackable = closest_targetable(level, entity, memories, true)
            .or_else(|| closest_targetable(level, entity, memories, false));
        match attackable {
            Some(target) => memories.set(&mem::NEAREST_ATTACKABLE, target),
            None => memories.erase(&mem::NEAREST_ATTACKABLE),
        }
    }
}

/// Vanilla `WardenEntitySensor.getClosest(body, test)`: the nearest (the
/// `NEAREST_LIVING_ENTITIES` list is ordered nearest-first) targetable entity,
/// restricted to players (`players_only`) or non-players.
fn closest_targetable(
    level: &World,
    body: &SharedEntity,
    memories: &MemoryStore,
    players_only: bool,
) -> Option<SharedEntity> {
    memories
        .get(&mem::NEAREST_LIVING_ENTITIES)?
        .into_iter()
        .find(|entity: &SharedEntity| {
            (entity.entity_type().key == vanilla_entities::PLAYER.key) == players_only
                && warden_can_target_entity(level, body, entity)
        })
}

/// Vanilla `Warden.canTargetEntity`.
///
/// Local port: `entity/ai/behavior/warden/mod.rs::can_target_entity` needs a
/// `BehaviorContext`, which sensors do not have, so the same predicate is
/// spelled here against the sensor's `level`/`body` pair.
#[must_use]
fn warden_can_target_entity(level: &World, body: &SharedEntity, target: &SharedEntity) -> bool {
    let Some(living) = target.as_living_entity() else {
        return false;
    };

    let same_level = match (body.level(), target.level()) {
        (Some(body_level), Some(target_level)) => Arc::ptr_eq(&body_level, &target_level),
        _ => false,
    };

    same_level
        && !target
            .as_player()
            .is_some_and(|player| player.has_infinite_materials() || player.is_spectator())
        && !body.is_allied_to(target.as_ref())
        && target.entity_type().key != vanilla_entities::ARMOR_STAND.key
        && target.entity_type().key != vanilla_entities::WARDEN.key
        && !target.is_invulnerable()
        && !LivingEntity::is_dead_or_dying(living)
        && level
            .world_border_snapshot()
            .is_within_bounds(target.bounding_box())
}
