//! Vanilla `PrepareRamNearestTarget`: pick a target, walk to a ramming start
//! position, wait, then set `RAM_TARGET`.

use std::cmp::Ordering;
use std::sync::Arc;

use glam::DVec3;
use steel_utils::BlockPos;
use steel_utils::direction::Direction;
use steel_utils::entity_events::EntityStatus;

use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStatus, module_type as mem};
use crate::entity::ai::brain::tracker::{EntityTracker, WalkTarget};
use crate::entity::ai::targeting::TargetingConditions;
use crate::entity::{PathfinderMob, SharedEntity};

/// Vanilla `PrepareRamNearestTarget.TIME_OUT_DURATION`.
pub const TIME_OUT_DURATION: i32 = 160;

/// Vanilla `PrepareRamNearestTarget.RamCandidate`.
pub struct RamCandidate {
    start_position: BlockPos,
    target_position: BlockPos,
    target: SharedEntity,
}

impl RamCandidate {
    #[must_use]
    pub const fn start_position(&self) -> BlockPos {
        self.start_position
    }

    #[must_use]
    pub const fn target_position(&self) -> BlockPos {
        self.target_position
    }

    #[must_use]
    pub fn target(&self) -> &SharedEntity {
        &self.target
    }
}

/// Vanilla `PrepareRamNearestTarget<E extends PathfinderMob>`.
pub struct PrepareRamNearestTarget {
    state: BehaviorState,
    cooldown_on_fail: i32,
    min_ram_distance: i32,
    max_ram_distance: i32,
    walk_speed: f32,
    ram_targeting: TargetingConditions,
    ram_prepare_time: i32,
    ram_candidate: Option<RamCandidate>,
    reached_ram_position_timestamp: Option<i64>,
}

impl PrepareRamNearestTarget {
    /// Vanilla `PrepareRamNearestTarget(getCooldownOnFail, minRamDistance,
    /// maxRamDistance, walkSpeed, ramTargeting, ramPrepareTime, getPrepareRamSound)`.
    ///
    /// Steel carries no sound handle on the behavior layer, so `getPrepareRamSound`
    /// is not taken here; target selection and the walk-up are complete.
    #[must_use]
    pub fn new(
        cooldown_on_fail: i32,
        min_ram_distance: i32,
        max_ram_distance: i32,
        walk_speed: f32,
        ram_targeting: TargetingConditions,
        ram_prepare_time: i32,
    ) -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (
                        &mem::LOOK_TARGET as MemoryModuleTypeRef,
                        MemoryStatus::Registered,
                    ),
                    (
                        &mem::RAM_COOLDOWN_TICKS as MemoryModuleTypeRef,
                        MemoryStatus::ValueAbsent,
                    ),
                    (
                        &mem::NEAREST_VISIBLE_LIVING_ENTITIES as MemoryModuleTypeRef,
                        MemoryStatus::ValuePresent,
                    ),
                    (
                        &mem::RAM_TARGET as MemoryModuleTypeRef,
                        MemoryStatus::ValueAbsent,
                    ),
                ],
                TIME_OUT_DURATION,
                TIME_OUT_DURATION,
            ),
            cooldown_on_fail,
            min_ram_distance,
            max_ram_distance,
            walk_speed,
            ram_targeting,
            ram_prepare_time,
            ram_candidate: None,
            reached_ram_position_timestamp: None,
        }
    }

    /// Vanilla `getEdgeOfBlock(startRamPos, targetPos)`: the target's bottom center
    /// nudged half a block toward the rammer.
    #[must_use]
    fn edge_of_block(start_ram_pos: BlockPos, target_pos: BlockPos) -> DVec3 {
        let x_offset = 0.5 * f64::from((target_pos.x() - start_ram_pos.x()).signum());
        let z_offset = 0.5 * f64::from((target_pos.z() - start_ram_pos.z()).signum());
        let (bottom_x, bottom_y, bottom_z) = target_pos.get_bottom_center();
        DVec3::new(bottom_x + x_offset, bottom_y, bottom_z + z_offset)
    }

    /// Vanilla `calculateRammingStartPosition`.
    fn calculate_ramming_start_position(
        &self,
        ctx: &BehaviorContext<'_>,
        target: &SharedEntity,
    ) -> Option<BlockPos> {
        let mob = ctx.body.as_pathfinder_mob()?;
        let target_pos = target.block_position();
        if !is_walkable_block(mob, target_pos) {
            return None;
        }

        let mut possible_ram_positions = Vec::new();
        for direction in Direction::HORIZONTAL {
            let mut walkable = target_pos;
            for _ in 0..self.max_ram_distance {
                walkable = walkable.relative(direction);
                if !is_walkable_block(mob, walkable) {
                    walkable = walkable.relative(direction.opposite());
                    break;
                }
            }

            if manhattan_distance(walkable, target_pos) >= self.min_ram_distance {
                possible_ram_positions.push(walkable);
            }
        }

        let body_pos = ctx.body.block_position();
        possible_ram_positions.sort_by(|a, b| {
            dist_sqr(*a, body_pos)
                .partial_cmp(&dist_sqr(*b, body_pos))
                .unwrap_or(Ordering::Equal)
        });

        possible_ram_positions.into_iter().find(|pos| {
            mob.create_path_to(*pos, 0)
                .is_some_and(|path| path.can_reach())
        })
    }

    /// Vanilla `chooseRamPosition`.
    fn choose_ram_position(&mut self, ctx: &mut BehaviorContext<'_>, target: SharedEntity) {
        self.reached_ram_position_timestamp = None;
        let target_position = target.block_position();
        self.ram_candidate =
            self.calculate_ramming_start_position(ctx, &target)
                .map(|start_position| RamCandidate {
                    start_position,
                    target_position,
                    target,
                });
    }
}

/// Vanilla `isWalkableBlock`: stable destination with zero pathfinding malus.
fn is_walkable_block(mob: &dyn PathfinderMob, target_pos: BlockPos) -> bool {
    use crate::entity::ai::path::PathfindingContext;
    use crate::entity::mob::Mob as _;

    let Some(world) = mob.level() else {
        return false;
    };
    if !mob.is_stable_destination(target_pos) {
        return false;
    }

    // Vanilla `WalkNodeEvaluator.getPathTypeStatic(body, targetPos)`.
    let mut context = PathfindingContext::new(world.as_ref(), mob.block_position());
    let path_type =
        context.get_path_type_from_state(target_pos.x(), target_pos.y(), target_pos.z());
    mob.get_pathfinding_malus(path_type) == 0.0
}

/// Vanilla `BlockPos.distManhattan`.
fn manhattan_distance(a: BlockPos, b: BlockPos) -> i32 {
    (a.x() - b.x()).abs() + (a.y() - b.y()).abs() + (a.z() - b.z()).abs()
}

/// Vanilla `BlockPos.distSqr`.
fn dist_sqr(a: BlockPos, b: BlockPos) -> f64 {
    let dx = f64::from(a.x() - b.x());
    let dy = f64::from(a.y() - b.y());
    let dz = f64::from(a.z() - b.z());
    dx * dx + dy * dy + dz * dz
}

impl Behavior for PrepareRamNearestTarget {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        let body = ctx.body;
        let target = ctx
            .memories()
            .get(&mem::NEAREST_VISIBLE_LIVING_ENTITIES)
            .and_then(|visible| {
                visible.find_closest(|entity| {
                    entity.as_living_entity().is_some_and(|living| {
                        self.ram_targeting
                            .test(ctx.level, body.as_living_entity(), living)
                    })
                })
            });
        if let Some(target) = target {
            self.choose_ram_position(ctx, target);
        }
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        if !ctx.has_memory_value(&mem::RAM_TARGET) {
            ctx.body.broadcast_entity_event(EntityStatus::EndRam);
            ctx.set_memory(&mem::RAM_COOLDOWN_TICKS, self.cooldown_on_fail);
        }
        self.ram_candidate = None;
        self.reached_ram_position_timestamp = None;
    }

    fn can_still_use(&self, _ctx: &mut BehaviorContext<'_>) -> bool {
        self.ram_candidate
            .as_ref()
            .is_some_and(|candidate| candidate.target().is_alive())
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        let Some(candidate) = &self.ram_candidate else {
            return;
        };

        ctx.set_memory(
            &mem::WALK_TARGET,
            WalkTarget::from_block_pos(candidate.start_position(), self.walk_speed, 0),
        );
        ctx.set_memory(
            &mem::LOOK_TARGET,
            Arc::new(EntityTracker::new(candidate.target().clone(), true)),
        );

        let did_target_move = candidate.target().block_position() != candidate.target_position();
        if did_target_move {
            let target = candidate.target().clone();
            ctx.body.broadcast_entity_event(EntityStatus::EndRam);
            if let Some(mob) = ctx.body.as_pathfinder_mob() {
                mob.mob_base().navigation().lock().stop();
            }
            self.choose_ram_position(ctx, target);
            return;
        }

        let start_ram_pos = ctx.body.block_position();
        if start_ram_pos != candidate.start_position() {
            return;
        }

        ctx.body.broadcast_entity_event(EntityStatus::StartRam);
        if self.reached_ram_position_timestamp.is_none() {
            self.reached_ram_position_timestamp = Some(ctx.timestamp);
        }

        let reached_at = self.reached_ram_position_timestamp;
        if reached_at
            .is_some_and(|reached| ctx.timestamp - reached >= i64::from(self.ram_prepare_time))
        {
            let target_position = candidate.target_position();
            ctx.set_memory(
                &mem::RAM_TARGET,
                Self::edge_of_block(start_ram_pos, target_position),
            );
            self.ram_candidate = None;
        }
    }
}
