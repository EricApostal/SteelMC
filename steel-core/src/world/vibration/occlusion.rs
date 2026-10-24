//! Occlusion traversal for vibration delivery.
//!
//! Ports `BlockGetter.isBlockInLine`/`traverseBlocks` as used by
//! `VibrationSystem.Listener.isOccluded`: a DDA walk over the segment that visits the start
//! block first and reports whether any visited block matches the predicate
//! (`#occludes_vibration_signals`).

use glam::DVec3;
use steel_utils::{BlockPos, BlockStateId};

use super::mth_sign;
use crate::world::World;

/// Vanilla `BlockPos.containing` floored to the block center, both endpoints of the occlusion
/// walk (theListener uses the center, not the raw source position).
#[must_use]
pub fn block_center(pos: DVec3) -> DVec3 {
    DVec3::new(
        pos.x.floor() + 0.5,
        pos.y.floor() + 0.5,
        pos.z.floor() + 0.5,
    )
}

/// Vanilla `BlockGetter.traverseBlocks` specialization for an occlusion predicate.
///
/// Visits blocks in segment order, starting with the block containing `from`
/// (`Mth.lerp(-1.0E-7, to.x, from.x)` endpoints, `Mth.sign` component steps), and returns
/// `true` as soon as `predicate` accepts a visited state.
#[must_use]
pub fn is_block_in_line(
    world: &World,
    from: DVec3,
    to: DVec3,
    predicate: impl Fn(BlockStateId) -> bool,
) -> bool {
    // Vanilla `Vec3.equals`: identical endpoints can never cross a block.
    if from == to {
        return false;
    }
    // Vanilla `traverseBlocks` endpoints: `Mth.lerp(-1.0E-7, to.x, from.x)` and the mirror
    // start. `Mth.lerp(delta, start, end)` is `start + delta * (end - start)`.
    const ADJUST: f64 = -1.0e-7;
    let to_adjusted = DVec3::new(
        to.x + ADJUST * (from.x - to.x),
        to.y + ADJUST * (from.y - to.y),
        to.z + ADJUST * (from.z - to.z),
    );
    let from_adjusted = DVec3::new(
        from.x + ADJUST * (to.x - from.x),
        from.y + ADJUST * (to.y - from.y),
        from.z + ADJUST * (to.z - from.z),
    );

    // The DDA difference is taken against the *adjusted* endpoints in vanilla.
    let difference = to_adjusted - from_adjusted;
    let step_x = mth_sign(difference.x);
    let step_y = mth_sign(difference.y);
    let step_z = mth_sign(difference.z);
    let delta_x = if step_x == 0 {
        f64::MAX
    } else {
        f64::from(step_x) / difference.x
    };
    let delta_y = if step_y == 0 {
        f64::MAX
    } else {
        f64::from(step_y) / difference.y
    };
    let delta_z = if step_z == 0 {
        f64::MAX
    } else {
        f64::from(step_z) / difference.z
    };

    let mut block = BlockPos::new(
        from_adjusted.x.floor() as i32,
        from_adjusted.y.floor() as i32,
        from_adjusted.z.floor() as i32,
    );
    // Vanilla visits the start block before stepping.
    if predicate(world.get_block_state(block)) {
        return true;
    }

    let mut next = DVec3::new(
        delta_x
            * if step_x > 0 {
                1.0 - (from_adjusted.x - from_adjusted.x.floor())
            } else {
                from_adjusted.x - from_adjusted.x.floor()
            },
        delta_y
            * if step_y > 0 {
                1.0 - (from_adjusted.y - from_adjusted.y.floor())
            } else {
                from_adjusted.y - from_adjusted.y.floor()
            },
        delta_z
            * if step_z > 0 {
                1.0 - (from_adjusted.z - from_adjusted.z.floor())
            } else {
                from_adjusted.z - from_adjusted.z.floor()
            },
    );

    while next.x <= 1.0 || next.y <= 1.0 || next.z <= 1.0 {
        if next.x < next.y && next.x < next.z {
            block = BlockPos::new(block.x() + step_x, block.y(), block.z());
            next.x += delta_x;
        } else if next.y < next.z {
            block = BlockPos::new(block.x(), block.y() + step_y, block.z());
            next.y += delta_y;
        } else {
            block = BlockPos::new(block.x(), block.y(), block.z() + step_z);
            next.z += delta_z;
        }
        if predicate(world.get_block_state(block)) {
            return true;
        }
    }

    false
}
