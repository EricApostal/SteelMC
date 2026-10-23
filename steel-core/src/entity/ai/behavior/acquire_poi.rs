//! Vanilla `AcquirePoi`.

use std::cell::RefCell;
use std::mem;

use rustc_hash::FxHashMap;
use steel_registry::poi::PoiTypeRef;
use steel_utils::BlockPos;
use steel_utils::entity_events::EntityStatus;

use super::poi::{self, Occupancy, PoiPredicate};
use crate::entity::LivingEntity;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::{MemoryModuleType, MemoryModuleTypeRef, MemoryStatus};
use crate::world::World;

/// Vanilla `AcquirePoi.SCAN_RANGE`.
const SCAN_RANGE: i32 = 48;
/// Vanilla `AcquirePoi.create` local `batchSize`.
const BATCH_SIZE: usize = 5;
/// Vanilla `AcquirePoi.create` local `rate`.
const RATE: i64 = 20;
/// Vanilla `AcquirePoi.JitteredLinearRetry.MIN_INTERVAL_INCREASE`.
const MIN_INTERVAL_INCREASE: i32 = 40;
/// Vanilla `AcquirePoi.JitteredLinearRetry.MAX_INTERVAL_INCREASE`.
const MAX_INTERVAL_INCREASE: i32 = 80;
/// Vanilla `AcquirePoi.JitteredLinearRetry.MAX_RETRY_PATHFINDING_INTERVAL`.
const MAX_RETRY_PATHFINDING_INTERVAL: i32 = 400;

/// Vanilla `AcquirePoi.JitteredLinearRetry`: backs off one POI's re-pathfind attempts.
struct JitteredLinearRetry {
    previous_attempt_timestamp: i64,
    next_scheduled_attempt_timestamp: i64,
    current_delay: i32,
}

impl JitteredLinearRetry {
    fn new(first_attempt_timestamp: i64) -> Self {
        let mut retry = Self {
            previous_attempt_timestamp: first_attempt_timestamp,
            next_scheduled_attempt_timestamp: first_attempt_timestamp,
            current_delay: 0,
        };
        retry.mark_attempt(first_attempt_timestamp);
        retry
    }

    fn mark_attempt(&mut self, timestamp: i64) {
        self.previous_attempt_timestamp = timestamp;
        let suggested_delay =
            self.current_delay + rand::random_range(MIN_INTERVAL_INCREASE..MAX_INTERVAL_INCREASE);
        self.current_delay = suggested_delay.min(MAX_RETRY_PATHFINDING_INTERVAL);
        self.next_scheduled_attempt_timestamp = timestamp + i64::from(self.current_delay);
    }

    fn is_still_valid(&self, timestamp: i64) -> bool {
        timestamp - self.previous_attempt_timestamp < 400
    }

    fn should_retry(&self, timestamp: i64) -> bool {
        timestamp >= self.next_scheduled_attempt_timestamp
    }
}

/// Vanilla `AcquirePoi`: finds a nearby free POI, paths to it and claims a ticket.
///
/// Steel's `OneShot` only registers the memories its trigger reads; the vanilla
/// `absent(memoryToAcquire)`/`absent(memoryToValidate)` entry conditions are
/// re-checked at the top of [`Trigger::trigger`].
pub struct AcquirePoi {
    poi_type: PoiPredicate,
    memory_to_validate: &'static MemoryModuleType<steel_utils::GlobalPos>,
    memory_to_acquire: &'static MemoryModuleType<steel_utils::GlobalPos>,
    only_if_adult: bool,
    event: Option<u8>,
    valid_poi: fn(&World, BlockPos) -> bool,
    next_scheduled_start: i64,
    batch_cache: FxHashMap<BlockPos, JitteredLinearRetry>,
}

impl AcquirePoi {
    /// Vanilla `AcquirePoi.create(poiType, memoryToValidate, memoryToAcquire,
    /// onlyIfAdult, onPoiAcquisitionEvent, validPoi)`.
    ///
    /// `memory_to_acquire == None` mirrors vanilla's single-memory overloads, which
    /// acquire and validate the same memory.
    #[must_use]
    pub fn create(
        poi_type: PoiPredicate,
        memory_to_validate: &'static MemoryModuleType<steel_utils::GlobalPos>,
        memory_to_acquire: Option<&'static MemoryModuleType<steel_utils::GlobalPos>>,
        only_if_adult: bool,
        event: Option<u8>,
        valid_poi: fn(&World, BlockPos) -> bool,
    ) -> OneShot<Self> {
        let memory_to_acquire = memory_to_acquire.unwrap_or(memory_to_validate);
        let required_memories: Vec<MemoryModuleTypeRef> =
            vec![memory_to_validate, memory_to_acquire];
        OneShot::new(Self {
            poi_type,
            memory_to_validate,
            memory_to_acquire,
            only_if_adult,
            event,
            valid_poi,
            next_scheduled_start: 0,
            batch_cache: FxHashMap::default(),
        })
        .with_required_memories(required_memories)
    }
}

impl Trigger for AcquirePoi {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if !ctx.check_memory(self.memory_to_acquire, MemoryStatus::ValueAbsent) {
            return false;
        }
        if self.memory_to_validate.key() != self.memory_to_acquire.key()
            && !ctx.check_memory(self.memory_to_validate, MemoryStatus::ValueAbsent)
        {
            return false;
        }

        if self.only_if_adult
            && ctx
                .body
                .as_living_entity()
                .is_some_and(LivingEntity::is_baby)
        {
            return false;
        }

        let game_time = ctx.level.game_time();
        if self.next_scheduled_start == 0 {
            self.next_scheduled_start = game_time + i64::from(rand::random_range(0..RATE as i32));
            return false;
        }
        if game_time < self.next_scheduled_start {
            return false;
        }
        self.next_scheduled_start =
            ctx.timestamp + RATE + i64::from(rand::random_range(0..RATE as i32));

        let timestamp = ctx.timestamp;
        let mut cache = mem::take(&mut self.batch_cache);
        cache.retain(|_, retry| retry.is_still_valid(timestamp));

        let center = ctx.body.block_position();
        let poi_type = self.poi_type;
        let valid_poi = self.valid_poi;
        let level = ctx.level;
        let candidates: Vec<(BlockPos, PoiTypeRef)> = {
            let cache_cell = RefCell::new(cache);
            let found = poi::find_all_closest_first_with_type(
                level,
                poi_type,
                |pos| {
                    let mut cache = cache_cell.borrow_mut();
                    match cache.get_mut(&pos) {
                        None => true,
                        Some(retry) => {
                            if !retry.should_retry(timestamp) {
                                return false;
                            }
                            retry.mark_attempt(timestamp);
                            true
                        }
                    }
                },
                center,
                SCAN_RANGE,
                Occupancy::Free,
            );
            cache = cache_cell.into_inner();
            found
                .into_iter()
                .take(BATCH_SIZE)
                .filter(|(pos, _)| valid_poi(level, *pos))
                .collect()
        };

        let path = ctx
            .body
            .as_pathfinder_mob()
            .and_then(|mob| poi::find_path_to_pois(mob, &candidates));

        match path {
            Some(path) if path.can_reach() => {
                let target_pos = path.target();
                if poi::poi_type_at(level, target_pos).is_some() {
                    let _ = poi::take(level, poi_type, |pos| pos == target_pos, target_pos, 1);
                    ctx.set_memory(self.memory_to_acquire, poi::global_pos(level, target_pos));
                    if let Some(status) = self.event.and_then(acquisition_event_status) {
                        ctx.body.broadcast_entity_event(status);
                    }
                    cache.clear();
                }
            }
            _ => {
                for (pos, _) in &candidates {
                    cache
                        .entry(*pos)
                        .or_insert_with(|| JitteredLinearRetry::new(timestamp));
                }
            }
        }

        self.batch_cache = cache;
        true
    }
}

/// Vanilla broadcasts the raw acquisition byte through `ServerLevel.broadcastEntityEvent`;
/// Steel's entity-event API is typed, so the two events villagers use are mapped.
fn acquisition_event_status(event: u8) -> Option<EntityStatus> {
    match event {
        13 => Some(EntityStatus::VillagerAngry),
        14 => Some(EntityStatus::VillagerHappy),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_backoff_is_capped_at_the_vanilla_maximum() {
        let mut retry = JitteredLinearRetry::new(0);
        for timestamp in 0..100 {
            retry.mark_attempt(timestamp);
        }

        assert!(retry.current_delay <= MAX_RETRY_PATHFINDING_INTERVAL);
        assert!(retry.current_delay >= MIN_INTERVAL_INCREASE);
    }

    /// The acquisition event byte maps to the villager happy status; other bytes are
    /// ignored because Steel's entity events are typed.
    #[test]
    fn acquisition_event_byte_maps_to_villager_events() {
        assert_eq!(
            acquisition_event_status(14),
            Some(EntityStatus::VillagerHappy)
        );
        assert_eq!(
            acquisition_event_status(13),
            Some(EntityStatus::VillagerAngry)
        );
        assert_eq!(acquisition_event_status(0), None);
    }
}
