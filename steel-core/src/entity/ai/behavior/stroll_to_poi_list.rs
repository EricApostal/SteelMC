//! Vanilla `StrollToPoiList`.

use steel_utils::GlobalPos;

use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::memory::{MemoryModuleType, MemoryStatus};
use crate::entity::ai::brain::tracker::WalkTarget;

/// Vanilla `StrollToPoiList` re-path cadence.
const MIN_STROLL_INTERVAL: i64 = 100;

/// Vanilla `StrollToPoiList`: strolls to a random POI from a list while staying near a
/// main POI memory.
pub struct StrollToPoiList {
    stroll_to_memory: &'static MemoryModuleType<Vec<GlobalPos>>,
    speed_modifier: f32,
    close_enough_dist: i32,
    max_distance_from_poi: i32,
    must_be_close_to_memory: &'static MemoryModuleType<GlobalPos>,
    next_ok_start_time: i64,
}

impl StrollToPoiList {
    /// Vanilla `StrollToPoiList.create(strollToMemoryType, speedModifier, closeEnoughDist,
    /// maxDistanceFromPoi, mustBeCloseToMemoryType)`.
    #[must_use]
    pub fn create(
        stroll_to_memory: &'static MemoryModuleType<Vec<GlobalPos>>,
        speed_modifier: f32,
        close_enough_dist: i32,
        max_distance_from_poi: i32,
        must_be_close_to_memory: &'static MemoryModuleType<GlobalPos>,
    ) -> OneShot<Self> {
        OneShot::new(Self {
            stroll_to_memory,
            speed_modifier,
            close_enough_dist,
            max_distance_from_poi,
            must_be_close_to_memory,
            next_ok_start_time: 0,
        })
        .with_required_memories(vec![
            &mem::WALK_TARGET,
            stroll_to_memory,
            must_be_close_to_memory,
        ])
    }
}

impl Trigger for StrollToPoiList {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if !ctx.check_memory(&mem::WALK_TARGET, MemoryStatus::Registered)
            || !ctx.check_memory(self.stroll_to_memory, MemoryStatus::ValuePresent)
            || !ctx.check_memory(self.must_be_close_to_memory, MemoryStatus::ValuePresent)
        {
            return false;
        }

        let stroll_to: Vec<GlobalPos> = ctx.get_memory(self.stroll_to_memory).unwrap_or_default();
        if stroll_to.is_empty() {
            return false;
        }
        let Some(stay_close_to) = ctx.get_memory(self.must_be_close_to_memory) else {
            return false;
        };

        let index = rand::random_range(0..stroll_to.len());
        let target_pos = stroll_to[index].clone();
        if target_pos.dimension != ctx.level.key
            || !stay_close_to
                .pos
                .closer_to_center_than(ctx.body.position(), f64::from(self.max_distance_from_poi))
        {
            return false;
        }

        if ctx.timestamp > self.next_ok_start_time {
            ctx.set_memory(
                &mem::WALK_TARGET,
                WalkTarget::from_block_pos(
                    target_pos.pos,
                    self.speed_modifier,
                    self.close_enough_dist,
                ),
            );
            self.next_ok_start_time = ctx.timestamp + MIN_STROLL_INTERVAL;
        }

        true
    }
}
