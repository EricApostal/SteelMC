//! Vanilla `MobSensor<T>`: the generic "set a boolean memory for a while when a
//! matching mob is nearby" sensor, plus the `ARMADILLO_SCARE_DETECTED` instance.
//!
//! Vanilla parameterizes this sensor with a `BiPredicate<T, LivingEntity>` mob
//! test and a `Predicate<T>` readiness test. Steel's sensors all operate on
//! [`SharedEntity`], so the same two predicates are stored as boxed closures over
//! `SharedEntity` pairs.

use steel_registry::vanilla_entity_type_tags::EntityTypeTag;
use steel_registry::{REGISTRY, TaggedRegistryExt as _};

use crate::entity::ai::brain::memory::MemoryModuleType;
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::MemoryStore;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::sensor::{BrainSensor, SensorType};
use crate::entity::leash::Leashable;
use crate::entity::{Entity, LivingEntity, PathfinderMob, SharedEntity};
use crate::world::World;

/// Vanilla `Armadillo.SCARE_DISTANCE_HORIZONTAL`: the scare box's x/z inflation.
const SCARE_DISTANCE_HORIZONTAL: f64 = 7.0;
/// Vanilla `Armadillo.SCARE_DISTANCE_VERTICAL`: the scare box's y inflation.
const SCARE_DISTANCE_VERTICAL: f64 = 2.0;
/// Vanilla `new MobSensor<>(5, ...)` scan rate for the armadillo scare sensor.
const ARMADILLO_SCARE_SCAN_RATE: i64 = 5;
/// Vanilla `new MobSensor<>(..., DANGER_DETECTED_RECENTLY, 80)` time-to-live.
const ARMADILLO_SCARE_MEMORY_TIME_TO_LIVE: i64 = 80;

/// Vanilla `SensorType.ARMADILLO_SCARE_DETECTED`.
pub(crate) static ARMADILLO_SCARE_DETECTED: SensorType =
    SensorType::new(create_armadillo_scare, "armadillo_scare_detected");

/// Vanilla `SensorType.ARMADILLO_SCARE_DETECTED` factory:
/// `new MobSensor<>(5, Armadillo::isScaredBy, Armadillo::canStayRolledUp,
/// DANGER_DETECTED_RECENTLY, 80)`.
pub(crate) fn create_armadillo_scare() -> Box<dyn BrainSensor> {
    Box::new(MobSensor::new(
        ARMADILLO_SCARE_SCAN_RATE,
        Box::new(is_scared_by),
        Box::new(can_stay_rolled_up),
        &mem::DANGER_DETECTED_RECENTLY,
        ARMADILLO_SCARE_MEMORY_TIME_TO_LIVE,
    ))
}

/// Vanilla `MobSensor<T>`: whenever `ready_test` accepts the body, remember that a
/// mob matching `mob_test` was nearby by setting `to_set` for `memory_time_to_live`
/// ticks; otherwise erase `to_set`.
pub(crate) struct MobSensor {
    /// Vanilla `Sensor(scanRate)`.
    scan_rate: i64,
    /// Vanilla `MobSensor.mobTest`: `BiPredicate<T, LivingEntity>`.
    mob_test: Box<dyn Fn(&SharedEntity, &SharedEntity) -> bool + Send + Sync>,
    /// Vanilla `MobSensor.readyTest`: `Predicate<T>`.
    ready_test: Box<dyn Fn(&SharedEntity) -> bool + Send + Sync>,
    /// Vanilla `MobSensor.toSet`.
    to_set: &'static MemoryModuleType<bool>,
    /// Vanilla `MobSensor.memoryTimeToLive`.
    memory_time_to_live: i64,
}

impl MobSensor {
    /// Vanilla `MobSensor(int scanRate, BiPredicate<T, LivingEntity> mobTest,
    /// Predicate<T> readyTest, MemoryModuleType<Boolean> toSet, int memoryTimeToLive)`.
    pub(crate) fn new(
        scan_rate: i64,
        mob_test: Box<dyn Fn(&SharedEntity, &SharedEntity) -> bool + Send + Sync>,
        ready_test: Box<dyn Fn(&SharedEntity) -> bool + Send + Sync>,
        to_set: &'static MemoryModuleType<bool>,
        memory_time_to_live: i64,
    ) -> Self {
        Self {
            scan_rate,
            mob_test,
            ready_test,
            to_set,
            memory_time_to_live,
        }
    }

    /// Vanilla `MobSensor.checkForMobsNearby`: if the `NEAREST_LIVING_ENTITIES`
    /// memory is present and any entry passes `mob_test`, call [`Self::mob_detected`].
    pub(crate) fn check_for_mobs_nearby(&self, body: &SharedEntity, memories: &mut MemoryStore) {
        let Some(living_entities) = memories.get(&mem::NEAREST_LIVING_ENTITIES) else {
            // Vanilla `getMemory(...)` returning an empty `Optional` skips the scan.
            return;
        };

        if living_entities
            .iter()
            .any(|entity| (self.mob_test)(body, entity))
        {
            self.mob_detected(memories);
        }
    }

    /// Vanilla `MobSensor.mobDetected`: `setMemoryWithExpiry(toSet, true, memoryTimeToLive)`.
    pub(crate) fn mob_detected(&self, memories: &mut MemoryStore) {
        memories.set_with_expiry(self.to_set, true, self.memory_time_to_live);
    }

    /// Vanilla `MobSensor.clearMemory`: `eraseMemory(toSet)`.
    pub(crate) fn clear_memory(&self, memories: &mut MemoryStore) {
        memories.erase(self.to_set);
    }
}

impl BrainSensor for MobSensor {
    /// Vanilla `MobSensor.requires`: `Set.of(NEAREST_LIVING_ENTITIES)`.
    fn requires(&self) -> &'static [MemoryModuleTypeRef] {
        static REQUIRES: &[MemoryModuleTypeRef] = &[&mem::NEAREST_LIVING_ENTITIES];
        REQUIRES
    }

    /// Vanilla `MobSensor.doTick`: erase the memory unless `readyTest` passes, then
    /// scan the nearby living entities.
    fn sense(&mut self, _level: &World, entity: &SharedEntity, memories: &mut MemoryStore) {
        if !(self.ready_test)(entity) {
            self.clear_memory(memories);
        } else {
            self.check_for_mobs_nearby(entity, memories);
        }
    }

    /// Vanilla `Sensor(scanRate)`.
    fn scan_rate(&self) -> i64 {
        self.scan_rate
    }
}

/// Vanilla `Armadillo.isScaredBy`: an entity inside the armadillo's inflated
/// `7 x 2 x 7` scare box that is undead, the armadillo's last hurter, or a
/// non-spectator player that is sprinting or riding another entity.
///
/// `MobSensor`'s mob test receives [`SharedEntity`] pairs, so the predicate is a
/// free function here; the armadillo entity's own `is_scared_by` carries the same
/// expression for the concrete entity.
fn is_scared_by(body: &SharedEntity, living: &SharedEntity) -> bool {
    let bounds = body.bounding_box().inflate_xyz(
        SCARE_DISTANCE_HORIZONTAL,
        SCARE_DISTANCE_VERTICAL,
        SCARE_DISTANCE_HORIZONTAL,
    );
    if !bounds.intersects(living.bounding_box()) {
        return false;
    }

    if REGISTRY
        .entity_types
        .is_in_tag(living.entity_type(), &EntityTypeTag::UNDEAD)
    {
        return true;
    }

    // Vanilla `this.getLastHurtByMob() == livingEntity` is reference identity;
    // entity ids are the per-level identity used across Steel's memories.
    if body
        .as_living_entity()
        .and_then(|living_entity| living_entity.last_hurt_by_mob())
        .is_some_and(|hurter| hurter.id() == living.id())
    {
        return true;
    }

    if let Some(player) = living.as_player() {
        return !Entity::is_spectator(player)
            && (LivingEntity::is_sprinting(player) || player.is_passenger());
    }

    false
}

/// Vanilla `Armadillo.canStayRolledUp`: not panicking, not in liquid, not leashed,
/// not riding, and not carrying passengers.
///
/// `PathfinderMob::is_panicking` covers vanilla's brain-memory check plus the
/// running-`PanicGoal` fallback; `Entity.isInLiquid` is `is_in_water() || is_in_lava()`.
fn can_stay_rolled_up(body: &SharedEntity) -> bool {
    let panicking = body
        .as_pathfinder_mob()
        .is_some_and(|pathfinder| PathfinderMob::is_panicking(pathfinder));
    let in_liquid = body.is_in_water() || body.is_in_lava();
    let leashed = body
        .as_leashable()
        .is_some_and(|leashable| Leashable::is_leashed(leashable));

    !panicking && !in_liquid && !leashed && !body.is_passenger() && !body.is_vehicle()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn armadillo_scare_requires_only_nearby_living_entities() {
        let sensor = create_armadillo_scare();

        assert_eq!(sensor.requires().len(), 1);
        assert_eq!(
            sensor.requires()[0].key(),
            mem::NEAREST_LIVING_ENTITIES.key()
        );
        assert_eq!(sensor.scan_rate(), ARMADILLO_SCARE_SCAN_RATE);
    }

    #[test]
    fn mob_detected_sets_the_expiring_memory() {
        let mut memories = MemoryStore::new();
        memories.register(&mem::DANGER_DETECTED_RECENTLY);

        let sensor = MobSensor::new(
            ARMADILLO_SCARE_SCAN_RATE,
            Box::new(|_: &SharedEntity, _: &SharedEntity| false),
            Box::new(|_: &SharedEntity| true),
            &mem::DANGER_DETECTED_RECENTLY,
            ARMADILLO_SCARE_MEMORY_TIME_TO_LIVE,
        );

        sensor.mob_detected(&mut memories);

        assert_eq!(memories.get(&mem::DANGER_DETECTED_RECENTLY), Some(true));
        assert_eq!(
            memories.time_to_live(&mem::DANGER_DETECTED_RECENTLY),
            ARMADILLO_SCARE_MEMORY_TIME_TO_LIVE
        );
    }

    #[test]
    fn clear_memory_erases_the_memory() {
        let mut memories = MemoryStore::new();
        memories.register(&mem::DANGER_DETECTED_RECENTLY);

        let sensor = MobSensor::new(
            ARMADILLO_SCARE_SCAN_RATE,
            Box::new(|_: &SharedEntity, _: &SharedEntity| false),
            Box::new(|_: &SharedEntity| true),
            &mem::DANGER_DETECTED_RECENTLY,
            ARMADILLO_SCARE_MEMORY_TIME_TO_LIVE,
        );

        memories.set(&mem::DANGER_DETECTED_RECENTLY, true);
        sensor.clear_memory(&mut memories);

        assert!(!memories.has_value(&mem::DANGER_DETECTED_RECENTLY));
    }
}
