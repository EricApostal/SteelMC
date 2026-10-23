//! Vanilla `BehaviorUtils` plus the small memory-access helpers every behavior
//! package needs.
//!
//! Vanilla behaviors reach the brain through `body.getBrain()`; Steel passes the
//! memory store in [`BehaviorContext`], so the same operations are expressed as
//! context-first helpers.

use std::sync::Arc;

use glam::DVec3;
use steel_registry::item_stack::ItemStack;
use steel_utils::BlockPos;

use super::super::brain::behavior::BehaviorContext;
use super::super::brain::memory::module_type as mem;
use super::super::brain::tracker::{BlockPosTracker, EntityTracker, PositionTracker, WalkTarget};
use super::super::path::Path;
use crate::entity::{PathfinderMob, SharedEntity};

/// Vanilla `BehaviorUtils.lookAtEntity`.
pub fn look_at_entity(ctx: &mut BehaviorContext<'_>, target: &SharedEntity) {
    set_look_target(ctx, Arc::new(EntityTracker::new(target.clone(), true)));
}

/// Vanilla `Brain.setMemory(LOOK_TARGET, tracker)`.
pub fn set_look_target(ctx: &mut BehaviorContext<'_>, target: Arc<dyn PositionTracker>) {
    ctx.set_memory(&mem::LOOK_TARGET, target);
}

/// Vanilla `Brain.setMemory(WALK_TARGET, new WalkTarget(tracker, speed, closeEnough))`.
pub fn set_walk_target(
    ctx: &mut BehaviorContext<'_>,
    target: Arc<dyn PositionTracker>,
    speed_modifier: f32,
    close_enough_dist: i32,
) {
    ctx.set_memory(
        &mem::WALK_TARGET,
        WalkTarget::new(target, speed_modifier, close_enough_dist),
    );
}

/// Vanilla `BehaviorUtils.setWalkAndLookTargetMemories(walker, target, speed, closeEnough)`.
pub fn set_walk_and_look_target(
    ctx: &mut BehaviorContext<'_>,
    target: Arc<dyn PositionTracker>,
    speed_modifier: f32,
    close_enough_dist: i32,
) {
    ctx.set_memory(&mem::LOOK_TARGET, target.clone());
    set_walk_target(ctx, target, speed_modifier, close_enough_dist);
}

/// Vanilla `BehaviorUtils.setWalkAndLookTargetMemories(walker, BlockPos, ...)`.
pub fn set_walk_and_look_target_to_pos(
    ctx: &mut BehaviorContext<'_>,
    pos: BlockPos,
    speed_modifier: f32,
    close_enough_dist: i32,
) {
    set_walk_and_look_target(
        ctx,
        Arc::new(BlockPosTracker::from_block_pos(pos)),
        speed_modifier,
        close_enough_dist,
    );
}

/// Vanilla `BehaviorUtils.setWalkAndLookTargetMemories(walker, Entity, ...)`.
pub fn set_walk_and_look_target_to_entity(
    ctx: &mut BehaviorContext<'_>,
    entity: &SharedEntity,
    speed_modifier: f32,
    close_enough_dist: i32,
) {
    set_walk_and_look_target(
        ctx,
        Arc::new(EntityTracker::new(entity.clone(), true)),
        speed_modifier,
        close_enough_dist,
    );
}

/// Vanilla `BehaviorUtils.setWalkAndLookTargetMemoriesToEachOther`.
pub fn lock_gaze_and_walk_to_each_other(
    ctx: &mut BehaviorContext<'_>,
    other: &SharedEntity,
    speed_modifier: f32,
    close_enough_dist: i32,
) {
    set_walk_and_look_target_to_entity(ctx, other, speed_modifier, close_enough_dist);
}

/// Vanilla `Brain.eraseMemory(WALK_TARGET)` plus the navigation reset vanilla
/// performs when a walk target is abandoned.
pub fn clear_walk_target(ctx: &mut BehaviorContext<'_>) {
    ctx.erase_memory(&mem::WALK_TARGET);
    ctx.erase_memory(&mem::PATH);
    if let Some(mob) = ctx.body.as_pathfinder_mob() {
        mob.mob_base().navigation().lock().stop();
    }
}

/// Vanilla `Brain.eraseMemory(LOOK_TARGET)`.
pub fn clear_look_target(ctx: &mut BehaviorContext<'_>) {
    ctx.erase_memory(&mem::LOOK_TARGET);
}

/// Vanilla `BehaviorUtils.entityIsVisible`: the body's
/// `NEAREST_VISIBLE_LIVING_ENTITIES` memory already applies the line-of-sight test.
#[must_use]
pub fn entity_is_visible(ctx: &BehaviorContext<'_>, target: &SharedEntity) -> bool {
    ctx.memories()
        .get(&mem::NEAREST_VISIBLE_LIVING_ENTITIES)
        .is_some_and(|visible| visible.contains(target))
}

/// Vanilla `Brain.getMemory(NEAREST_VISIBLE_LIVING_ENTITIES).findClosest(predicate)`.
#[must_use]
pub fn find_visible_entity(
    ctx: &BehaviorContext<'_>,
    filter: impl Fn(&SharedEntity) -> bool,
) -> Option<SharedEntity> {
    ctx.memories()
        .get(&mem::NEAREST_VISIBLE_LIVING_ENTITIES)
        .and_then(|visible| visible.find_closest(filter))
}

/// Vanilla `PathNavigation.createPath(target)`: `None` when no reachable path exists.
#[must_use]
pub fn can_reach(ctx: &BehaviorContext<'_>, pos: BlockPos, reach_range: i32) -> Option<Path> {
    let mob = ctx.body.as_pathfinder_mob()?;
    mob.create_path_to(pos, reach_range)
}

/// Manhattan distance between two block positions, vanilla `BlockPos.distManhattan`.
#[must_use]
pub fn manhattan_distance(a: BlockPos, b: BlockPos) -> i32 {
    (a.0.x - b.0.x).abs() + (a.0.y - b.0.y).abs() + (a.0.z - b.0.z).abs()
}

/// Vanilla `Entity.closerThan(other, distance)`.
#[must_use]
pub fn closer_than(entity: &SharedEntity, other: &SharedEntity, distance: f64) -> bool {
    entity.position().distance_squared(other.position()) <= distance * distance
}

/// Vanilla `Entity.distanceToSqr(BlockPos)` for the block's center.
#[must_use]
pub fn distance_to_block_sqr(entity: &SharedEntity, pos: BlockPos) -> f64 {
    entity.distance_to_sqr(block_center(pos))
}

/// The center of a block, `Vec3.atCenterOf`.
#[must_use]
pub fn block_center(pos: BlockPos) -> DVec3 {
    let (x, y, z) = pos.get_center();
    DVec3::new(x, y, z)
}

/// The bottom center of a block, `Vec3.atBottomCenterOf`.
#[must_use]
pub fn block_bottom_center(pos: BlockPos) -> DVec3 {
    let (x, y, z) = pos.get_bottom_center();
    DVec3::new(x, y, z)
}

/// Vanilla `Mob.isWithinMeleeAttackRange(target)`.
#[must_use]
pub fn is_within_melee_attack_range(body: &SharedEntity, target: &SharedEntity) -> bool {
    let Some(mob) = body.as_mob() else {
        return false;
    };
    let Some(target) = target.as_living_entity() else {
        return false;
    };

    mob.is_within_melee_attack_range(target)
}

/// Vanilla `Villager.canSee`-style checks in this package are expressed with
/// [`entity_is_visible`]; this alias keeps vanilla call sites readable.
#[must_use]
pub fn can_see(ctx: &BehaviorContext<'_>, target: &SharedEntity) -> bool {
    entity_is_visible(ctx, target)
}

/// The body as a pathfinding mob, if it has navigation.
#[must_use]
pub fn pathfinder_mob<'a>(ctx: &'a BehaviorContext<'_>) -> Option<&'a dyn PathfinderMob> {
    ctx.body.as_pathfinder_mob()
}

/// Vanilla `BehaviorUtils.throwItem`: drops `item` from the thrower's hand toward
/// `target_pos`, with the default throw velocity and pickup delay.
pub fn throw_item(thrower: &SharedEntity, item: ItemStack, target_pos: DVec3) {
    /// Vanilla `BehaviorUtils.throwItem`'s `throwVelocity` (`0.3F` on every axis).
    const THROW_VELOCITY: f64 = 0.3;
    /// Vanilla `BehaviorUtils.throwItem`'s `handYDistanceFromEye`.
    const HAND_Y_DISTANCE_FROM_EYE: f64 = 0.3;

    if item.is_empty() {
        return;
    }

    let Some(level) = thrower.level() else {
        return;
    };

    let mut velocity = target_pos - thrower.position();
    // Vanilla `Vec3.normalize` returns the zero vector for a degenerate input.
    velocity = if velocity.length_squared() < 1.0e-5 {
        DVec3::ZERO
    } else {
        velocity.normalize() * THROW_VELOCITY
    };

    let position = DVec3::new(
        thrower.position().x,
        thrower.get_eye_y() - HAND_Y_DISTANCE_FROM_EYE,
        thrower.position().z,
    );

    let Some(item_entity) = level.spawn_item_with_velocity(position, item, velocity) else {
        return;
    };

    item_entity.set_thrower(thrower.uuid());
    item_entity.set_default_pickup_delay();
}
