use glam::DVec3;
use std::f32::consts as f32_consts;
use std::f64::consts as f64_consts;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_utils::BlockPos;
use steel_utils::random::{Random, legacy_random::LegacyRandom};

use crate::entity::PathfinderMob;
use crate::entity::ai::path::PathfindingContext;
use crate::entity::ai::walk::WalkPathEvaluator;
use crate::fluid::FluidStateExt as _;

const RANDOM_POS_ATTEMPTS: usize = 10;

/// Vanilla's callers pass `(float) (Math.PI / 2)`; the `float` narrowing is part of the math.
pub(crate) const HALF_PI: f64 = f32_consts::FRAC_PI_2 as f64;

/// Vanilla `Raider.ObtainLeaderBannerGoal` passes `(float) (Math.PI / 10)`.
pub(crate) const TENTH_PI: f64 = (f64_consts::PI / 10.0) as f32 as f64;

pub(crate) fn default_random_pos(
    mob: &dyn PathfinderMob,
    horizontal_dist: i32,
    vertical_dist: i32,
) -> Option<DVec3> {
    let restrict = mob_restricted(mob, f64::from(horizontal_dist));
    generate_random_pos(mob, || {
        let direction = generate_random_direction(mob, horizontal_dist, vertical_dist);
        default_random_pos_toward_direction(mob, f64::from(horizontal_dist), restrict, direction)
    })
}

pub(crate) fn default_random_pos_towards(
    mob: &dyn PathfinderMob,
    horizontal_dist: i32,
    vertical_dist: i32,
    towards_pos: DVec3,
    max_xz_radians_from_dir: f64,
) -> Option<DVec3> {
    let dir = towards_pos - mob.position();
    let restrict = mob_restricted(mob, f64::from(horizontal_dist));
    generate_random_pos(mob, || {
        let direction = {
            let mut random = LegacyRandom::from_seed(rand::random());
            generate_random_direction_within_radians(
                &mut random,
                0.0,
                f64::from(horizontal_dist),
                vertical_dist,
                0,
                dir.x,
                dir.z,
                max_xz_radians_from_dir,
            )
        }?;
        default_random_pos_toward_direction(mob, f64::from(horizontal_dist), restrict, direction)
    })
}

pub(crate) fn default_random_pos_away(
    mob: &dyn PathfinderMob,
    horizontal_dist: i32,
    vertical_dist: i32,
    avoid_pos: DVec3,
) -> Option<DVec3> {
    let dir_away = mob.position() - avoid_pos;
    let restrict = mob_restricted(mob, f64::from(horizontal_dist));
    generate_random_pos(mob, || {
        let direction = {
            let mut random = LegacyRandom::from_seed(rand::random());
            generate_random_direction_within_radians(
                &mut random,
                0.0,
                f64::from(horizontal_dist),
                vertical_dist,
                0,
                dir_away.x,
                dir_away.z,
                HALF_PI,
            )
        }?;
        default_random_pos_toward_direction(mob, f64::from(horizontal_dist), restrict, direction)
    })
}

pub(crate) fn land_random_pos(
    mob: &dyn PathfinderMob,
    horizontal_dist: i32,
    vertical_dist: i32,
) -> Option<DVec3> {
    let restrict = mob_restricted(mob, f64::from(horizontal_dist));
    generate_random_pos(mob, || {
        let direction = generate_random_direction(mob, horizontal_dist, vertical_dist);
        let pos =
            land_random_pos_toward_direction(mob, f64::from(horizontal_dist), restrict, direction)?;
        land_move_pos_up_out_of_solid(mob, pos)
    })
}

/// Vanilla `LandRandomPos.getPosTowards(mob, xz, y, towardsPos)`: the land search steered
/// toward `towardsPos`, scored by the walk-target value.
pub(crate) fn land_random_pos_towards(
    mob: &dyn PathfinderMob,
    horizontal_dist: i32,
    vertical_dist: i32,
    towards_pos: DVec3,
) -> Option<DVec3> {
    let dir = towards_pos - mob.position();
    let restrict = mob_restricted(mob, f64::from(horizontal_dist));
    generate_random_pos(mob, || {
        let direction = {
            let mut random = LegacyRandom::from_seed(rand::random());
            generate_random_direction_within_radians(
                &mut random,
                0.0,
                f64::from(horizontal_dist),
                vertical_dist,
                0,
                dir.x,
                dir.z,
                HALF_PI,
            )
        }?;
        let pos =
            land_random_pos_toward_direction(mob, f64::from(horizontal_dist), restrict, direction)?;
        land_move_pos_up_out_of_solid(mob, pos)
    })
}

fn generate_random_pos(
    mob: &dyn PathfinderMob,
    pos_supplier: impl FnMut() -> Option<BlockPos>,
) -> Option<DVec3> {
    generate_random_pos_with_weight(pos_supplier, |pos| {
        f64::from(mob.get_walk_target_value(pos))
    })
}

/// Vanilla `HoverRandomPos.getPos(...)`: a land-biased direction search whose result is
/// lifted above solid ground so the mob hovers.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors the vanilla helper's parameter list"
)]
pub(crate) fn hover_random_pos(
    mob: &dyn PathfinderMob,
    horizontal_dist: i32,
    vertical_dist: i32,
    x_dir: f64,
    z_dir: f64,
    max_xz_radians_from_dir: f64,
    hover_max_height: i32,
    hover_min_height: i32,
) -> Option<DVec3> {
    let restrict = mob_restricted(mob, f64::from(horizontal_dist));
    generate_random_pos(mob, || {
        let direction = {
            let mut random = LegacyRandom::from_seed(rand::random());
            generate_random_direction_within_radians(
                &mut random,
                0.0,
                f64::from(horizontal_dist),
                vertical_dist,
                0,
                x_dir,
                z_dir,
                max_xz_radians_from_dir,
            )
        }?;
        let pos =
            land_random_pos_toward_direction(mob, f64::from(horizontal_dist), restrict, direction)?;
        let above_solid = rand::random_range(hover_min_height..=hover_max_height);
        move_up_to_above_solid(mob, pos, above_solid)
    })
}

/// Vanilla `AirAndWaterRandomPos.getPos(...)`: a land-biased direction search lifted out of
/// solid blocks, rejecting positions with a pathfinding malus.
pub(crate) fn air_and_water_random_pos(
    mob: &dyn PathfinderMob,
    horizontal_dist: i32,
    vertical_dist: i32,
    flying_height: i32,
    x_dir: f64,
    z_dir: f64,
    max_xz_radians_from_dir: f64,
) -> Option<DVec3> {
    let restrict = mob_restricted(mob, f64::from(horizontal_dist));
    generate_random_pos(mob, || {
        let direction = {
            let mut random = LegacyRandom::from_seed(rand::random());
            generate_random_direction_within_radians(
                &mut random,
                0.0,
                f64::from(horizontal_dist),
                vertical_dist,
                flying_height,
                x_dir,
                z_dir,
                max_xz_radians_from_dir,
            )
        }?;
        let pos = generate_random_pos_toward_direction(mob, f64::from(horizontal_dist), direction);
        if is_outside_limits(mob, pos) || is_restricted(restrict, mob, pos) {
            return None;
        }
        let pos = move_up_out_of_solid(mob, pos)?;
        if has_malus(mob, pos) {
            return None;
        }
        Some(pos)
    })
}

/// Vanilla `AirRandomPos.getPosTowards(mob, xz, y, flyingHeight, towardsPos, maxXZRadians)`:
/// the [`air_and_water_random_pos`] search steered toward `towardsPos`, rejecting water.
pub(crate) fn air_random_pos_towards(
    mob: &dyn PathfinderMob,
    horizontal_dist: i32,
    vertical_dist: i32,
    flying_height: i32,
    towards_pos: DVec3,
    max_xz_radians_from_dir: f64,
) -> Option<DVec3> {
    let dir = towards_pos - mob.position();
    let restrict = mob_restricted(mob, f64::from(horizontal_dist));
    generate_random_pos(mob, || {
        let direction = {
            let mut random = LegacyRandom::from_seed(rand::random());
            generate_random_direction_within_radians(
                &mut random,
                0.0,
                f64::from(horizontal_dist),
                vertical_dist,
                flying_height,
                dir.x,
                dir.z,
                max_xz_radians_from_dir,
            )
        }?;
        let pos = generate_random_pos_toward_direction(mob, f64::from(horizontal_dist), direction);
        if is_outside_limits(mob, pos) || is_restricted(restrict, mob, pos) {
            return None;
        }
        let pos = move_up_out_of_solid(mob, pos)?;
        if has_malus(mob, pos) || is_water(mob, pos) {
            return None;
        }
        Some(pos)
    })
}

/// Vanilla `RandomPos.moveUpToAboveSolid`.
fn move_up_to_above_solid(
    mob: &dyn PathfinderMob,
    pos: BlockPos,
    above_solid: i32,
) -> Option<BlockPos> {
    if !is_solid(mob, pos) {
        return Some(pos);
    }

    let world = mob.level()?;
    let mut pos = pos.above();
    while pos.y() <= world.get_max_y() && is_solid(mob, pos) {
        pos = pos.above();
    }
    let first_non_solid_y = pos.y();
    while pos.y() <= world.get_max_y() && pos.y() - first_non_solid_y < above_solid {
        pos = pos.above();
        if is_solid(mob, pos) {
            pos = pos.below();
            break;
        }
    }
    if is_water(mob, pos) || has_malus(mob, pos) {
        return None;
    }
    Some(pos)
}

/// Vanilla `LandRandomPos.getPos(mob, xz, y, weight)`: the same 10-attempt
/// search as [`generate_random_pos`], scored by a caller-supplied weight.
pub(crate) fn land_random_pos_with_weight(
    mob: &dyn PathfinderMob,
    horizontal_dist: i32,
    vertical_dist: i32,
    weight: impl Fn(BlockPos) -> f64,
) -> Option<DVec3> {
    let restrict = mob_restricted(mob, f64::from(horizontal_dist));
    generate_random_pos_with_weight(
        || {
            let direction = generate_random_direction(mob, horizontal_dist, vertical_dist);
            let pos = land_random_pos_toward_direction(
                mob,
                f64::from(horizontal_dist),
                restrict,
                direction,
            )?;
            land_move_pos_up_out_of_solid(mob, pos)
        },
        weight,
    )
}

fn generate_random_pos_with_weight(
    mut pos_supplier: impl FnMut() -> Option<BlockPos>,
    weight: impl Fn(BlockPos) -> f64,
) -> Option<DVec3> {
    let mut best_weight = f64::NEG_INFINITY;
    let mut best_pos = None;

    for _ in 0..RANDOM_POS_ATTEMPTS {
        let Some(pos) = pos_supplier() else {
            continue;
        };
        let value = weight(pos);
        if value > best_weight {
            best_weight = value;
            best_pos = Some(pos);
        }
    }

    best_pos.map(block_bottom_center)
}

fn generate_random_direction(
    _mob: &dyn PathfinderMob,
    horizontal_dist: i32,
    vertical_dist: i32,
) -> BlockPos {
    BlockPos::new(
        rand::random_range(0..(2 * horizontal_dist + 1)) - horizontal_dist,
        rand::random_range(0..(2 * vertical_dist + 1)) - vertical_dist,
        rand::random_range(0..(2 * horizontal_dist + 1)) - horizontal_dist,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "vanilla random direction helper takes these values independently"
)]
fn generate_random_direction_within_radians(
    random: &mut impl Random,
    min_horizontal_dist: f64,
    max_horizontal_dist: f64,
    vertical_dist: i32,
    flying_height: i32,
    x_dir: f64,
    z_dir: f64,
    max_xz_radians_from_dir: f64,
) -> Option<BlockPos> {
    let y_radians_center = z_dir.atan2(x_dir) - f64::from(f32_consts::FRAC_PI_2);
    // Vanilla computes the jitter in `float` arithmetic: `2.0F * nextFloat() - 1.0F`.
    let jitter = f64::from(2.0_f32 * random.next_f32() - 1.0);
    let y_radians = y_radians_center + jitter * max_xz_radians_from_dir;
    // Vanilla `Mth.SQRT_OF_TWO` is the `float` square root, widened to `double`.
    let dist = lerp(
        random.next_f64().sqrt(),
        min_horizontal_dist,
        max_horizontal_dist,
    ) * f64::from(f32_consts::SQRT_2);
    let xt = -dist * y_radians.sin();
    let zt = dist * y_radians.cos();
    if xt.abs() > max_horizontal_dist || zt.abs() > max_horizontal_dist {
        return None;
    }

    let yt = random.next_i32_bounded(2 * vertical_dist + 1) - vertical_dist + flying_height;
    Some(BlockPos::containing(xt, f64::from(yt), zt))
}

fn default_random_pos_toward_direction(
    mob: &dyn PathfinderMob,
    horizontal_dist: f64,
    restrict: bool,
    direction: BlockPos,
) -> Option<BlockPos> {
    let pos = generate_random_pos_toward_direction(mob, horizontal_dist, direction);
    if !is_outside_limits(mob, pos)
        && !is_restricted(restrict, mob, pos)
        && mob.is_stable_destination(pos)
        && !has_malus(mob, pos)
    {
        Some(pos)
    } else {
        None
    }
}

fn land_random_pos_toward_direction(
    mob: &dyn PathfinderMob,
    horizontal_dist: f64,
    restrict: bool,
    direction: BlockPos,
) -> Option<BlockPos> {
    let pos = generate_random_pos_toward_direction(mob, horizontal_dist, direction);
    if !is_outside_limits(mob, pos)
        && !is_restricted(restrict, mob, pos)
        && mob.is_stable_destination(pos)
    {
        Some(pos)
    } else {
        None
    }
}

fn generate_random_pos_toward_direction(
    mob: &dyn PathfinderMob,
    horizontal_dist: f64,
    direction: BlockPos,
) -> BlockPos {
    let mut xt = f64::from(direction.x());
    let mut zt = f64::from(direction.z());
    let position = mob.position();
    if mob.has_home() && horizontal_dist > 1.0 {
        let center = mob.home_position();
        if position.x > f64::from(center.x()) {
            xt -= rand::random_range(0.0..horizontal_dist / 2.0);
        } else {
            xt += rand::random_range(0.0..horizontal_dist / 2.0);
        }

        if position.z > f64::from(center.z()) {
            zt -= rand::random_range(0.0..horizontal_dist / 2.0);
        } else {
            zt += rand::random_range(0.0..horizontal_dist / 2.0);
        }
    }

    BlockPos::containing(
        xt + position.x,
        f64::from(direction.y()) + position.y,
        zt + position.z,
    )
}

fn land_move_pos_up_out_of_solid(mob: &dyn PathfinderMob, pos: BlockPos) -> Option<BlockPos> {
    let pos = move_up_out_of_solid(mob, pos)?;
    if !is_water(mob, pos) && !has_malus(mob, pos) {
        Some(pos)
    } else {
        None
    }
}

fn move_up_out_of_solid(mob: &dyn PathfinderMob, pos: BlockPos) -> Option<BlockPos> {
    if !is_solid(mob, pos) {
        return Some(pos);
    }

    let world = mob.level()?;
    let mut pos = pos.above();
    while pos.y() <= world.get_max_y() && is_solid(mob, pos) {
        pos = pos.above();
    }
    Some(pos)
}

fn mob_restricted(mob: &dyn PathfinderMob, horizontal_dist: f64) -> bool {
    mob.has_home()
        && block_center_distance_sqr(mob.home_position(), mob.position())
            < (f64::from(mob.home_radius()) + horizontal_dist + 1.0).powi(2)
}

fn is_outside_limits(mob: &dyn PathfinderMob, pos: BlockPos) -> bool {
    mob.level()
        .is_none_or(|world| world.is_outside_build_height(pos.y()))
}

fn is_restricted(restrict: bool, mob: &dyn PathfinderMob, pos: BlockPos) -> bool {
    restrict && !mob.is_within_home_pos(pos)
}

fn is_water(mob: &dyn PathfinderMob, pos: BlockPos) -> bool {
    mob.level()
        .is_none_or(|world| world.get_block_state(pos).get_fluid_state().is_water())
}

fn has_malus(mob: &dyn PathfinderMob, pos: BlockPos) -> bool {
    let Some(world) = mob.level() else {
        return true;
    };
    let mut context = PathfindingContext::new(world.as_ref(), mob.block_position());
    let path_type = WalkPathEvaluator::path_type_static(&mut context, pos);
    mob.get_pathfinding_malus(path_type) != 0.0
}

fn is_solid(mob: &dyn PathfinderMob, pos: BlockPos) -> bool {
    mob.level()
        .is_some_and(|world| world.get_block_state(pos).is_solid())
}

fn block_bottom_center(pos: BlockPos) -> DVec3 {
    let (x, y, z) = pos.get_bottom_center();
    DVec3::new(x, y, z)
}

fn block_center_distance_sqr(pos: BlockPos, target: DVec3) -> f64 {
    let (x, y, z) = pos.get_center();
    DVec3::new(x, y, z).distance_squared(target)
}

fn lerp(delta: f64, start: f64, end: f64) -> f64 {
    start + delta * (end - start)
}

#[cfg(test)]
mod tests {
    use steel_utils::random::legacy_random::LegacyRandom;

    use super::*;

    #[test]
    fn random_direction_within_radians_matches_vanilla_seed() {
        let mut random = LegacyRandom::from_seed(0);

        let direction = generate_random_direction_within_radians(
            &mut random,
            0.0,
            16.0,
            7,
            0,
            1.0,
            0.0,
            HALF_PI,
        );

        assert_eq!(direction, Some(BlockPos::new(15, -5, 13)));
    }

    #[test]
    fn random_direction_within_radians_points_away_from_avoid_pos() {
        let mut random = LegacyRandom::from_seed(0);

        let direction = generate_random_direction_within_radians(
            &mut random,
            0.0,
            16.0,
            7,
            0,
            -1.0,
            0.0,
            HALF_PI,
        );

        assert_eq!(direction, Some(BlockPos::new(-16, -5, -14)));
    }

    #[test]
    fn random_direction_within_radians_rejects_points_outside_square() {
        let mut random = LegacyRandom::from_seed(0);

        let direction =
            generate_random_direction_within_radians(&mut random, 1.0, 1.0, 0, 0, 1.0, 0.0, 0.0);

        assert_eq!(direction, None);
    }
}
