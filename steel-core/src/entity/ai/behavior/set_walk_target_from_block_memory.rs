//! Vanilla `SetWalkTargetFromBlockMemory`.

use std::f64::consts::FRAC_PI_2;

use steel_utils::{BlockPos, GlobalPos};

use super::poi::{self, PoiMemory, PoiPredicate};
use super::random_pos::default_random_pos_towards;
use super::utils;
use super::villager;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::memory::{MemoryModuleType, MemoryStatus};
use crate::entity::ai::brain::tracker::WalkTarget;

/// Vanilla `SetWalkTargetFromBlockMemory` partial-step fallback distance.
const STEP_TOWARDS_XZ_DIST: i32 = 15;
/// Vanilla `SetWalkTargetFromBlockMemory` partial-step fallback vertical distance.
const STEP_TOWARDS_Y_DIST: i32 = 7;
/// Vanilla `SetWalkTargetFromBlockMemory.MAX_TRIES`.
const MAX_TRIES: i32 = 1000;

/// Vanilla `SetWalkTargetFromBlockMemory`: walks to a block memory, stepping toward
/// out-of-range targets and releasing the POI when the target is unreachable.
pub struct SetWalkTargetFromBlockMemory {
    memory: &'static MemoryModuleType<GlobalPos>,
    speed_modifier: f32,
    close_enough_dist: i32,
    too_far_distance: i32,
    too_long_unreachable_duration: i64,
}

impl SetWalkTargetFromBlockMemory {
    /// Vanilla `SetWalkTargetFromBlockMemory.create(memoryType, speedModifier,
    /// closeEnoughDist, tooFarDistance, tooLongUnreachableDuration)`.
    #[must_use]
    pub fn create(
        memory: &'static MemoryModuleType<GlobalPos>,
        speed_modifier: f32,
        close_enough_dist: i32,
        too_far_distance: i32,
        too_long_unreachable_duration: i32,
    ) -> OneShot<Self> {
        OneShot::new(Self {
            memory,
            speed_modifier,
            close_enough_dist,
            too_far_distance,
            too_long_unreachable_duration: i64::from(too_long_unreachable_duration),
        })
        .with_required_memories(vec![
            &mem::CANT_REACH_WALK_TARGET_SINCE,
            &mem::WALK_TARGET,
            memory,
        ])
    }
}

impl Trigger for SetWalkTargetFromBlockMemory {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if !ctx.check_memory(&mem::WALK_TARGET, MemoryStatus::ValueAbsent) {
            return false;
        }
        let Some(target_pos) = ctx.get_memory(self.memory) else {
            return false;
        };
        let cant_reach_since = ctx.get_memory(&mem::CANT_REACH_WALK_TARGET_SINCE);

        let reachable_window = target_pos.dimension == ctx.level.key
            && cant_reach_since.is_none_or(|since| {
                ctx.level.game_time() - since <= self.too_long_unreachable_duration
            });

        if reachable_window {
            let body_pos = ctx.body.block_position();
            let target_dist = utils::manhattan_distance(target_pos.pos, body_pos);

            if target_dist > self.too_far_distance {
                let (bx, by, bz) = target_pos.pos.get_bottom_center();
                let towards = glam::DVec3::new(bx, by, bz);
                let mut tries = 0;

                loop {
                    let step = ctx.body.as_pathfinder_mob().and_then(|mob| {
                        default_random_pos_towards(
                            mob,
                            STEP_TOWARDS_XZ_DIST,
                            STEP_TOWARDS_Y_DIST,
                            towards,
                            FRAC_PI_2,
                        )
                    });

                    let in_range = step.is_some_and(|pos| {
                        utils::manhattan_distance(BlockPos::from(pos), body_pos)
                            <= self.too_far_distance
                    });
                    if let (Some(pos), true) = (step, in_range) {
                        ctx.set_memory(
                            &mem::WALK_TARGET,
                            WalkTarget::from_vec(pos, self.speed_modifier, self.close_enough_dist),
                        );
                        break;
                    }

                    tries += 1;
                    if tries == MAX_TRIES {
                        release_poi(ctx, self.memory, target_pos.pos);
                        ctx.erase_memory(self.memory);
                        ctx.set_memory(&mem::CANT_REACH_WALK_TARGET_SINCE, ctx.timestamp);
                        return true;
                    }
                }
            } else if target_dist > self.close_enough_dist {
                ctx.set_memory(
                    &mem::WALK_TARGET,
                    WalkTarget::from_block_pos(
                        target_pos.pos,
                        self.speed_modifier,
                        self.close_enough_dist,
                    ),
                );
            }
        } else {
            release_poi(ctx, self.memory, target_pos.pos);
            ctx.erase_memory(self.memory);
            ctx.set_memory(&mem::CANT_REACH_WALK_TARGET_SINCE, ctx.timestamp);
        }

        true
    }
}

/// Vanilla `Villager.releasePoi`: release only while the POI still matches the memory's
/// `POI_MEMORIES` predicate.
fn release_poi(ctx: &BehaviorContext<'_>, memory: &MemoryModuleType<GlobalPos>, pos: BlockPos) {
    let predicate = match poi_memory_of(memory) {
        Some(poi_memory) => villager::profession_of(ctx.body)
            .map_or(PoiPredicate::Any, |profession| {
                poi_memory.predicate(profession)
            }),
        None => PoiPredicate::Any,
    };

    if poi::poi_type_at(ctx.level, pos).is_some_and(|poi_type| predicate.matches(poi_type)) {
        poi::release(ctx.level, pos);
    }
}

/// Vanilla `Villager.POI_MEMORIES` keys.
fn poi_memory_of(memory: &MemoryModuleType<GlobalPos>) -> Option<PoiMemory> {
    match memory.key() {
        key if key == mem::HOME.key() => Some(PoiMemory::Home),
        key if key == mem::JOB_SITE.key() => Some(PoiMemory::JobSite),
        key if key == mem::POTENTIAL_JOB_SITE.key() => Some(PoiMemory::PotentialJobSite),
        key if key == mem::MEETING_POINT.key() => Some(PoiMemory::MeetingPoint),
        _ => None,
    }
}
