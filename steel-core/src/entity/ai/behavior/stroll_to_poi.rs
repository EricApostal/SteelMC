//! Vanilla `StrollToPoi`.

use steel_utils::GlobalPos;

use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::memory::{MemoryModuleType, MemoryStatus};
use crate::entity::ai::brain::tracker::WalkTarget;

/// Vanilla `StrollToPoi` re-path cadence.
const MIN_STROLL_INTERVAL: i64 = 80;

/// Vanilla `StrollToPoi`: walks to the POI memory while nearby.
pub struct StrollToPoi {
    memory: &'static MemoryModuleType<GlobalPos>,
    speed_modifier: f32,
    close_enough_dist: i32,
    max_distance_from_poi: i32,
    next_ok_start_time: i64,
}

impl StrollToPoi {
    /// Vanilla `StrollToPoi.create(memoryType, speedModifier, closeEnoughDist, maxDistanceFromPoi)`.
    #[must_use]
    pub fn create(
        memory: &'static MemoryModuleType<GlobalPos>,
        speed_modifier: f32,
        close_enough_dist: i32,
        max_distance_from_poi: i32,
    ) -> OneShot<Self> {
        OneShot::new(Self {
            memory,
            speed_modifier,
            close_enough_dist,
            max_distance_from_poi,
            next_ok_start_time: 0,
        })
        .with_required_memories(vec![&mem::WALK_TARGET, memory])
    }
}

impl Trigger for StrollToPoi {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if !ctx.check_memory(&mem::WALK_TARGET, MemoryStatus::Registered) {
            return false;
        }
        let Some(pos) = ctx.get_memory(self.memory) else {
            return false;
        };
        if pos.dimension != ctx.level.key
            || !pos
                .pos
                .closer_to_center_than(ctx.body.position(), f64::from(self.max_distance_from_poi))
        {
            return false;
        }

        if ctx.timestamp <= self.next_ok_start_time {
            return true;
        }

        ctx.set_memory(
            &mem::WALK_TARGET,
            WalkTarget::from_block_pos(pos.pos, self.speed_modifier, self.close_enough_dist),
        );
        self.next_ok_start_time = ctx.timestamp + MIN_STROLL_INTERVAL;
        true
    }
}
