//! Vanilla `MoveToTargetSink`: follow the remembered walk target.

use std::f64::consts::FRAC_PI_2;

use steel_utils::BlockPos;
use steel_utils::locks::SyncMutex;

use crate::entity::SharedEntity;
use crate::entity::ai::behavior::random_pos::default_random_pos_towards;
use crate::entity::ai::behavior::utils::{block_bottom_center, block_center, manhattan_distance};
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStatus, module_type as mem};
use crate::entity::ai::brain::tracker::WalkTarget;
use crate::entity::ai::path::Path;

/// Vanilla `MoveToTargetSink.MAX_COOLDOWN_BEFORE_RETRYING`.
const MAX_COOLDOWN_BEFORE_RETRYING: i32 = 40;
/// Vanilla `DefaultRandomPos.getPosTowards` bounds for the partial-step fallback.
const PARTIAL_STEP_HORIZONTAL_DIST: i32 = 10;
const PARTIAL_STEP_VERTICAL_DIST: i32 = 7;

#[derive(Debug, Clone, Copy, PartialEq)]
struct MoveToTargetSinkProgress {
    remaining_cooldown: i32,
    last_target_pos: Option<BlockPos>,
    speed_modifier: f32,
}

/// Vanilla `MoveToTargetSink`.
///
/// The progress lives behind a lock because vanilla mutates it from
/// `checkExtraStartConditions`, which Steel's [`Behavior`] trait exposes as `&self`.
pub struct MoveToTargetSink {
    state: BehaviorState,
    path: SyncMutex<Option<Path>>,
    progress: SyncMutex<MoveToTargetSinkProgress>,
}

impl MoveToTargetSink {
    /// Vanilla `new MoveToTargetSink()`.
    #[must_use]
    pub fn new() -> Self {
        Self::with_range(150, 250)
    }

    /// Vanilla `new MoveToTargetSink(minTimeout, maxTimeout)`.
    #[must_use]
    pub fn with_range(min_timeout: i32, max_timeout: i32) -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (
                        &mem::CANT_REACH_WALK_TARGET_SINCE as MemoryModuleTypeRef,
                        MemoryStatus::Registered,
                    ),
                    (&mem::PATH as MemoryModuleTypeRef, MemoryStatus::ValueAbsent),
                    (
                        &mem::WALK_TARGET as MemoryModuleTypeRef,
                        MemoryStatus::ValuePresent,
                    ),
                ],
                min_timeout,
                max_timeout,
            ),
            path: SyncMutex::new(None),
            progress: SyncMutex::new(MoveToTargetSinkProgress {
                remaining_cooldown: 0,
                last_target_pos: None,
                speed_modifier: 0.0,
            }),
        }
    }

    fn reached_target(body: &SharedEntity, walk_target: &WalkTarget) -> bool {
        manhattan_distance(
            walk_target.target().current_block_position(),
            body.block_position(),
        ) <= walk_target.close_enough_dist()
    }

    /// Vanilla `isWalkTargetSpectator`: the target only matters for spectator
    /// entities, which villagers never track.
    fn is_walk_target_spectator(_walk_target: &WalkTarget) -> bool {
        false
    }

    fn try_compute_path(
        ctx: &mut BehaviorContext<'_>,
        path: &mut Option<Path>,
        progress: &mut MoveToTargetSinkProgress,
        walk_target: &WalkTarget,
    ) -> bool {
        let target_pos = walk_target.target().current_block_position();
        let Some(mob) = ctx.body.as_pathfinder_mob() else {
            return false;
        };

        *path = mob.create_path_to(target_pos, 0);
        progress.speed_modifier = walk_target.speed_modifier();

        if Self::reached_target(ctx.body, walk_target) {
            ctx.erase_memory(&mem::CANT_REACH_WALK_TARGET_SINCE);
        } else {
            let can_reach = path.as_ref().is_some_and(Path::can_reach);
            if can_reach {
                ctx.erase_memory(&mem::CANT_REACH_WALK_TARGET_SINCE);
            } else if !ctx.has_memory_value(&mem::CANT_REACH_WALK_TARGET_SINCE) {
                ctx.set_memory(&mem::CANT_REACH_WALK_TARGET_SINCE, ctx.timestamp);
            }

            if path.is_some() {
                return true;
            }

            let Some(partial_step) = default_random_pos_towards(
                mob,
                PARTIAL_STEP_HORIZONTAL_DIST,
                PARTIAL_STEP_VERTICAL_DIST,
                block_bottom_center(target_pos),
                FRAC_PI_2,
            ) else {
                return false;
            };

            *path = mob.create_path_to(
                BlockPos::containing(partial_step.x, partial_step.y, partial_step.z),
                0,
            );
            return path.is_some();
        }

        path.is_some()
    }
}

impl Default for MoveToTargetSink {
    fn default() -> Self {
        Self::new()
    }
}

impl Behavior for MoveToTargetSink {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        let mut progress = self.progress.lock();
        if progress.remaining_cooldown > 0 {
            progress.remaining_cooldown -= 1;
            return false;
        }

        let Some(walk_target) = ctx.get_memory(&mem::WALK_TARGET) else {
            return false;
        };
        let reached_target = Self::reached_target(ctx.body, &walk_target);
        let mut path = self.path.lock();
        if !reached_target && Self::try_compute_path(ctx, &mut path, &mut progress, &walk_target) {
            progress.last_target_pos = Some(walk_target.target().current_block_position());
            return true;
        }

        ctx.erase_memory(&mem::WALK_TARGET);
        if reached_target {
            ctx.erase_memory(&mem::CANT_REACH_WALK_TARGET_SINCE);
        }

        false
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        let progress = self.progress.lock();
        if self.path.lock().is_none() || progress.last_target_pos.is_none() {
            return false;
        }

        let Some(walk_target) = ctx.get_memory(&mem::WALK_TARGET) else {
            return false;
        };
        let Some(mob) = ctx.body.as_pathfinder_mob() else {
            return false;
        };

        !mob.mob_base().navigation().lock().is_done()
            && !Self::reached_target(ctx.body, &walk_target)
            && !Self::is_walk_target_spectator(&walk_target)
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        let path = self.path.lock().clone();
        let speed_modifier = self.progress.lock().speed_modifier;
        let Some(path) = path else {
            return;
        };

        ctx.set_memory(&mem::PATH, path.clone());
        if let Some(mob) = ctx.body.as_pathfinder_mob() {
            mob.move_to_path(Some(path), f64::from(speed_modifier));
        }
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        let Some(mob) = ctx.body.as_pathfinder_mob() else {
            return;
        };
        let new_path = mob.mob_base().navigation().lock().path().cloned();

        {
            let mut progress = self.progress.lock();
            let mut path = self.path.lock();
            if *path != new_path {
                *path = new_path.clone();
                match &new_path {
                    Some(new_path) => ctx.set_memory(&mem::PATH, new_path.clone()),
                    None => ctx.erase_memory(&mem::PATH),
                }
            }

            if new_path.is_none() {
                return;
            }
            let Some(last_target_pos) = progress.last_target_pos else {
                return;
            };
            let Some(walk_target) = ctx.get_memory(&mem::WALK_TARGET) else {
                return;
            };

            let target_pos = walk_target.target().current_block_position();
            if block_center(target_pos).distance_squared(block_center(last_target_pos)) <= 4.0
                || !Self::try_compute_path(ctx, &mut path, &mut progress, &walk_target)
            {
                return;
            }

            progress.last_target_pos = Some(target_pos);
        }

        self.start(ctx);
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        let mut progress = self.progress.lock();
        let navigation_stuck = ctx
            .body
            .as_pathfinder_mob()
            .is_some_and(|mob| mob.mob_base().navigation().lock().is_stuck());

        if let Some(walk_target) = ctx.get_memory(&mem::WALK_TARGET)
            && !Self::reached_target(ctx.body, &walk_target)
            && navigation_stuck
        {
            progress.remaining_cooldown = rand::random_range(0..MAX_COOLDOWN_BEFORE_RETRYING);
        }

        if let Some(mob) = ctx.body.as_pathfinder_mob() {
            mob.mob_base().navigation().lock().stop();
        }
        ctx.erase_memory(&mem::WALK_TARGET);
        ctx.erase_memory(&mem::PATH);
        *self.path.lock() = None;
    }
}
