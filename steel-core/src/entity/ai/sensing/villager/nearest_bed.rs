//! Vanilla `NearestBedSensor`.

use rustc_hash::FxHashMap;
use steel_registry::vanilla_poi_types;
use steel_utils::BlockPos;
use steel_utils::locks::SyncMutex;

use crate::entity::SharedEntity;
use crate::entity::ai::behavior::poi::{self, Occupancy, PoiPredicate};
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::MemoryStore;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::sensor::BrainSensor;
use crate::world::World;

/// Vanilla `NearestBedSensor.CACHE_TIMEOUT`.
const CACHE_TIMEOUT: i64 = 40;
/// Vanilla `NearestBedSensor.BATCH_SIZE`.
const BATCH_SIZE: i32 = 5;
/// Vanilla `NearestBedSensor.RATE`: also the scan rate (`super(20)`).
const RATE: i64 = 20;
/// Vanilla `findAllWithType(..., body.blockPosition(), 48, ANY)`.
const SEARCH_RADIUS: i32 = 48;

/// Vanilla `SensorType.NEAREST_BED` factory.
pub(crate) fn create() -> Box<dyn BrainSensor> {
    Box::new(NearestBedSensor::default())
}

/// Vanilla `NearestBedSensor`: babies look for a home POI worth pathing to, with a
/// retry cache that stops re-trying the same unreachable beds for a while.
pub(crate) struct NearestBedSensor {
    batch_cache: BatchCache,
}

impl Default for NearestBedSensor {
    fn default() -> Self {
        Self {
            batch_cache: BatchCache::default(),
        }
    }
}

impl BrainSensor for NearestBedSensor {
    fn requires(&self) -> &'static [MemoryModuleTypeRef] {
        static REQUIRES: &[MemoryModuleTypeRef] = &[&mem::NEAREST_BED];
        REQUIRES
    }

    fn scan_rate(&self) -> i64 {
        RATE
    }

    fn sense(&mut self, level: &World, entity: &SharedEntity, memories: &mut MemoryStore) {
        if !entity
            .as_living_entity()
            .is_some_and(|living| living.is_baby())
        {
            return;
        }

        // Vanilla seeds this jitter from the level random; it only shifts when the
        // batch cache entries expire, so a local source is enough.
        let last_update = level.game_time() + rand::random_range(0..RATE);
        self.batch_cache.begin_batch(last_update);

        let pois = poi::find_all_closest_first_with_type(
            level,
            PoiPredicate::Type(&vanilla_poi_types::HOME),
            |pos| self.batch_cache.test(pos),
            entity.block_position(),
            SEARCH_RADIUS,
            Occupancy::Any,
        );

        let path = entity
            .as_pathfinder_mob()
            .and_then(|mob| poi::find_path_to_pois(mob, &pois));

        if let Some(path) = path.filter(|path| path.can_reach()) {
            let target_pos = path.target();
            if poi::poi_type_at(level, target_pos).is_some() {
                memories.set(&mem::NEAREST_BED, target_pos);
            }
        } else if self.batch_cache.should_retry() {
            self.batch_cache.remove_expired();
        }
    }
}

/// Vanilla `NearestBedSensor.batchCache`/`triedCount`/`lastUpdate`.
#[derive(Default)]
struct BatchCache {
    state: SyncMutex<BatchCacheState>,
}

#[derive(Default)]
struct BatchCacheState {
    /// Block position to the game time the entry expires at.
    entries: FxHashMap<BlockPos, i64>,
    tried_count: i32,
    last_update: i64,
}

impl BatchCache {
    /// Vanilla `this.triedCount = 0; this.lastUpdate = gameTime + random.nextInt(20)`.
    fn begin_batch(&self, last_update: i64) {
        let mut state = self.state.lock();
        state.tried_count = 0;
        state.last_update = last_update;
    }

    /// Vanilla `cacheTest`: at most [`BATCH_SIZE`] candidates per batch, once each.
    ///
    /// Vanilla increments before comparing (`++this.triedCount >= 5`), so the call
    /// that reaches the batch size is rejected and only `BATCH_SIZE - 1` candidates
    /// are actually tried per batch.
    fn test(&self, pos: BlockPos) -> bool {
        let mut state = self.state.lock();
        if state.entries.contains_key(&pos) {
            return false;
        }

        state.tried_count += 1;
        if state.tried_count >= BATCH_SIZE {
            return false;
        }

        let expires_at = state.last_update + CACHE_TIMEOUT;
        state.entries.insert(pos, expires_at);
        true
    }

    /// Vanilla `this.triedCount < 5`.
    fn should_retry(&self) -> bool {
        self.state.lock().tried_count < BATCH_SIZE
    }

    /// Vanilla `batchCache.long2LongEntrySet().removeIf(e -> e.getLongValue() < lastUpdate)`.
    fn remove_expired(&self) {
        let mut state = self.state.lock();
        let last_update = state.last_update;
        state
            .entries
            .retain(|_pos, expires_at| *expires_at >= last_update);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(x: i32) -> BlockPos {
        BlockPos::new(x, 64, 0)
    }

    #[test]
    fn batch_cache_tries_each_candidate_once_up_to_the_batch_size() {
        let cache = BatchCache::default();
        cache.begin_batch(100);

        // Vanilla `++triedCount >= 5` lets four candidates through per batch: the
        // fifth call only consumes the count.
        for x in 0..BATCH_SIZE - 1 {
            assert!(cache.test(pos(x)), "candidate {x} should be tried");
        }
        assert!(
            !cache.test(pos(BATCH_SIZE - 1)),
            "the call that reaches the batch size is rejected"
        );
        assert!(!cache.test(pos(BATCH_SIZE)), "batch is exhausted");
        assert!(!cache.test(pos(0)), "candidates are only tried once");
    }

    #[test]
    fn batch_cache_expires_entries_before_last_update() {
        let cache = BatchCache::default();
        cache.begin_batch(100);
        assert!(cache.test(pos(0)));

        cache.begin_batch(150);
        cache.remove_expired();
        assert!(cache.test(pos(0)), "entry expired before the retry update");

        cache.begin_batch(200);
        assert!(!cache.test(pos(0)), "entry is still cached");
    }

    #[test]
    fn next_batch_stops_retrying_once_the_batch_size_is_hit() {
        let cache = BatchCache::default();
        cache.begin_batch(100);
        for x in 0..BATCH_SIZE - 1 {
            assert!(cache.test(pos(x)));
        }
        assert!(cache.should_retry(), "four tries leave the batch open");
        assert!(!cache.test(pos(BATCH_SIZE - 1)));
        assert!(!cache.should_retry());

        cache.begin_batch(200);
        assert!(cache.should_retry());
    }
}
