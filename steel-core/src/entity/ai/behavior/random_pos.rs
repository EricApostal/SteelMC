//! Behaviour-local random-position helpers.
//!
//! Vanilla `RandomPos`/`LandRandomPos`/`DefaultRandomPos` are already ported in
//! `ai::goal::random_pos`, but those helpers are `pub(super)` to the goal module and
//! unreachable from `behavior`. This is a compact reimplementation that keeps
//! vanilla's 10-attempt contract but substitutes a walkability test for the full
//! pathfinder malus (move + path validity). Promote the goal helpers to `pub(crate)`
//! and delete this file to restore the single implementation.

use glam::DVec3;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_utils::BlockPos;

use crate::entity::PathfinderMob;
use crate::world::LevelReader as _;

/// Vanilla `RandomPos.RANDOM_POS_ATTEMPTS`.
const RANDOM_POS_ATTEMPTS: usize = 10;

/// Vanilla `LandRandomPos.getPos(mob, xzDist, yDist)`: a walkable landing position.
#[must_use]
pub fn land_random_pos(
    mob: &dyn PathfinderMob,
    horizontal_dist: i32,
    vertical_dist: i32,
) -> Option<DVec3> {
    rand_position(mob, horizontal_dist, vertical_dist, None, true)
}

/// Vanilla `DefaultRandomPos.getPosTowards(mob, xzDist, yDist, towards, maxXzRadians)`.
#[must_use]
pub fn default_random_pos_towards(
    mob: &dyn PathfinderMob,
    horizontal_dist: i32,
    vertical_dist: i32,
    towards_pos: DVec3,
    max_xz_radians_from_dir: f64,
) -> Option<DVec3> {
    rand_position(
        mob,
        horizontal_dist,
        vertical_dist,
        Some((towards_pos, max_xz_radians_from_dir)),
        false,
    )
}

fn rand_position(
    mob: &dyn PathfinderMob,
    horizontal_dist: i32,
    vertical_dist: i32,
    direction: Option<(DVec3, f64)>,
    require_land: bool,
) -> Option<DVec3> {
    let origin = mob.position();
    let world = mob.level()?;

    for _ in 0..RANDOM_POS_ATTEMPTS {
        let angle = match direction {
            Some((towards, max_radians)) => {
                let delta = towards - origin;
                let base = delta.z.atan2(delta.x);
                base + rand::random_range(-max_radians..max_radians)
            }
            None => rand::random_range(0.0..std::f64::consts::TAU),
        };
        let radius = rand::random::<f64>() * f64::from(horizontal_dist);
        let dy = rand::random_range(-vertical_dist..=vertical_dist);
        let candidate = BlockPos::containing(
            origin.x + angle.cos() * radius,
            origin.y + f64::from(dy),
            origin.z + angle.sin() * radius,
        );

        if require_land
            && !(world.get_block_state(candidate).is_air() && mob.is_stable_destination(candidate))
        {
            continue;
        }

        let (x, y, z) = candidate.get_bottom_center();
        return Some(DVec3::new(x, y, z));
    }

    None
}
