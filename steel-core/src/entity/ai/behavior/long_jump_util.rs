//! Vanilla `LongJumpUtil`: ballistic jump-vector maths for long jumps.
//!
//! Helpers-only module: it declares no behavior, so it is registered as
//! `pub(crate) mod long_jump_util;`.

use glam::DVec3;
use steel_utils::WorldAabb;

use crate::behavior::BlockCollisionContext;
use crate::entity::SharedEntity;
use crate::physics::{CollisionWorld as _, WorldCollisionProvider};

/// Vanilla `LongJumpUtil.calculateJumpVectorForAngle`.
///
/// Returns the launch velocity that lands the body on `target_pos` at `angle`
/// degrees, or `None` when that angle cannot reach the target or would exceed
/// `max_jump_velocity`.
#[must_use]
pub fn calculate_jump_vector_for_angle(
    body: &SharedEntity,
    target_pos: DVec3,
    max_jump_velocity: f32,
    angle: i32,
    check_collision: bool,
) -> Option<DVec3> {
    let mob_pos = body.position();
    let direction_vector_plane =
        DVec3::new(target_pos.x - mob_pos.x, 0.0, target_pos.z - mob_pos.z).normalize_or_zero()
            * 0.5;
    let target_position = target_pos - direction_vector_plane;
    let direction_vector = target_position - mob_pos;
    let angrad = angle as f32 * std::f32::consts::PI / 180.0;
    let xz_ang = direction_vector.z.atan2(direction_vector.x);
    let r2 = (direction_vector - DVec3::new(0.0, direction_vector.y, 0.0)).length_squared();
    let r = r2.sqrt();
    let y = direction_vector.y;
    let g = body.get_gravity();
    let sin2ang = f64::from(2.0 * angrad).sin();
    let cosangsqr = f64::from(angrad).cos().powi(2);
    let sinangrad = f64::from(angrad).sin();
    let cosangrad = f64::from(angrad).cos();
    let sinxz_ang = xz_ang.sin();
    let cosxz_ang = xz_ang.cos();

    let denominator = r * sin2ang - 2.0 * y * cosangsqr;
    if denominator.abs() < f64::EPSILON {
        return None;
    }
    let v0sqr = r2 * g / denominator;
    if v0sqr < 0.0 {
        return None;
    }

    let v0 = v0sqr.sqrt();
    if v0 > f64::from(max_jump_velocity) {
        return None;
    }

    let v0r = v0 * cosangrad;
    let v0y = v0 * sinangrad;
    if check_collision {
        let samples = ceil_to_i32(r / v0r) * 2;
        if samples > 1 {
            let mut ri = 0.0;
            let mut previous_pos: Option<DVec3> = None;
            let dimensions = body.bounding_box();
            let width = dimensions.width();
            let height = dimensions.height();

            for _ in 0..samples - 1 {
                ri += r / f64::from(samples);
                let yi =
                    sinangrad / cosangrad * ri - ri.powi(2) * g / (2.0 * v0sqr * cosangrad.powi(2));
                let xi = ri * cosxz_ang;
                let zi = ri * sinxz_ang;
                let sample_pos = DVec3::new(mob_pos.x + xi, mob_pos.y + yi, mob_pos.z + zi);
                if let Some(previous_pos) = previous_pos
                    && !is_clear_transition(
                        body,
                        width as f32,
                        height as f32,
                        previous_pos,
                        sample_pos,
                    )
                {
                    return None;
                }

                previous_pos = Some(sample_pos);
            }
        }
    }

    Some(DVec3::new(v0r * cosxz_ang, v0y, v0r * sinxz_ang) * 0.95)
}

/// Vanilla `LongJumpUtil.isClearTransition`.
fn is_clear_transition(
    body: &SharedEntity,
    width: f32,
    height: f32,
    position1: DVec3,
    position2: DVec3,
) -> bool {
    let direction = position2 - position1;
    let min_dimension = f64::from(width.min(height));
    if min_dimension <= 0.0 {
        return true;
    }

    let checks = ceil_to_i32(direction.length() / min_dimension);
    let normalized_direction = direction.normalize_or_zero();
    let mut next_point_to_check = position1;

    for i in 0..checks {
        next_point_to_check = if i == checks - 1 {
            position2
        } else {
            next_point_to_check + normalized_direction * (min_dimension * 0.9)
        };

        // Vanilla `level.noCollision(body, dimensions.makeBoundingBox(point))`.
        let half_width = f64::from(width) / 2.0;
        let bounds = WorldAabb::new(
            next_point_to_check.x - half_width,
            next_point_to_check.y,
            next_point_to_check.z - half_width,
            next_point_to_check.x + half_width,
            next_point_to_check.y + f64::from(height),
            next_point_to_check.z + half_width,
        );
        if !has_no_collision(body, &bounds) {
            return false;
        }
    }

    true
}

/// Vanilla `Level.noCollision(Entity, AABB)`.
fn has_no_collision(body: &SharedEntity, bounds: &WorldAabb) -> bool {
    let Some(world) = body.level() else {
        return false;
    };
    let collision_world = WorldCollisionProvider::for_entity(&world, body.as_entity_event_source());
    !collision_world.has_collision_with_context(bounds, BlockCollisionContext::empty())
}

/// Vanilla `Mth.ceil`.
fn ceil_to_i32(value: f64) -> i32 {
    value.ceil() as i32
}
