//! Vanilla `StrollAroundPoi`.

use steel_utils::GlobalPos;

use super::random_pos::land_random_pos;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::memory::{MemoryModuleType, MemoryStatus};
use crate::entity::ai::brain::tracker::WalkTarget;

/// Vanilla `StrollAroundPoi.MIN_TIME_BETWEEN_STROLLS`.
const MIN_TIME_BETWEEN_STROLLS: i64 = 180;
/// Vanilla `StrollAroundPoi.STROLL_MAX_XZ_DIST`.
const STROLL_MAX_XZ_DIST: i32 = 8;
/// Vanilla `StrollAroundPoi.STROLL_MAX_Y_DIST`.
const STROLL_MAX_Y_DIST: i32 = 6;

/// Vanilla `StrollAroundPoi`: picks a nearby landing spot while near a POI memory.
pub struct StrollAroundPoi {
    memory: &'static MemoryModuleType<GlobalPos>,
    speed_modifier: f32,
    max_distance_from_poi: i32,
    next_ok_start_time: i64,
}

impl StrollAroundPoi {
    /// Vanilla `StrollAroundPoi.create(memoryType, speedModifier, maxDistanceFromPoi)`.
    #[must_use]
    pub fn create(
        memory: &'static MemoryModuleType<GlobalPos>,
        speed_modifier: f32,
        max_distance_from_poi: i32,
    ) -> OneShot<Self> {
        OneShot::new(Self {
            memory,
            speed_modifier,
            max_distance_from_poi,
            next_ok_start_time: 0,
        })
        .with_required_memories(vec![&mem::WALK_TARGET, memory])
    }
}

impl Trigger for StrollAroundPoi {
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

        let speed = self.speed_modifier;
        let land_pos = ctx
            .body
            .as_pathfinder_mob()
            .and_then(|mob| land_random_pos(mob, STROLL_MAX_XZ_DIST, STROLL_MAX_Y_DIST));
        ctx.set_memory_or_erase(
            &mem::WALK_TARGET,
            land_pos.map(|pos| WalkTarget::from_vec(pos, speed, 1)),
        );
        self.next_ok_start_time = ctx.timestamp + MIN_TIME_BETWEEN_STROLLS;
        true
    }
}
