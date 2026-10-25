//! Vibration filtering shared by Vanilla's `VibrationSystem` listeners.
//!
//! Ports the `VibrationSystem.Listener` world queries that `steel-core/src/world/game_event/vibration.rs`
//! needs: the occlusion raycast (`isOccluded` against `#occludes_vibration_signals`), the
//! `#dampens_vibrations` checks in `VibrationSystem.User.isValidVibration`, the adjacent-chunk
//! gate in `VibrationSystem.Ticker.receiveVibration`, and the `avoid_vibration` criterion raise.
//!
//! The pure math (frequency table, selector, redstone strength, travel time) stays in
//! [`crate::world::game_event::vibration`] next to the listener that owns it.

pub mod damping;
pub mod occlusion;

use std::sync::Arc;

use glam::DVec3;
use steel_registry::advancement::triggers::TriggerEvent;
use steel_utils::Identifier;
use steel_utils::{BlockPos, ChunkPos};

use crate::entity::Entity as _;
use crate::entity::living_loot_equipment;
use crate::player::Player;
use crate::player::advancement::triggers::{WorldLevelView, event_base, player_view, raise};
use crate::world::World;

/// Returns the vanilla `Mth.sign(double)`: `-1`, `0` or `1` with `NaN` mapping to `0`.
///
/// Rust's `f64::signum` maps `+0.0` to `1.0` and preserves `NaN`, so the DDA traversal that
/// mirrors `BlockGetter.traverseBlocks` rolls its own sign.
pub(crate) const fn mth_sign(value: f64) -> i32 {
    if value > 0.0 {
        1
    } else if value < 0.0 {
        -1
    } else {
        0
    }
}

/// Vanilla `VibrationSystem.Listener.isOccluded`: every `isBlockInLine` walk starts at the
/// floored source/destination block *center* (the direction nudge keeps the start inside the
/// source block) and is `true` only when **all six** directions hit an occluding block.
#[must_use]
pub fn is_occluded(world: &World, source: DVec3, destination: DVec3) -> bool {
    // Vanilla `Vec3.relative(direction, 1.0E-5F)` (the float widens to this double).
    const NUDGE: f64 = 1.0e-5_f32 as f64;
    let from = occlusion::block_center(source);
    let to = occlusion::block_center(destination);
    for direction in steel_utils::Direction::ALL {
        let (step_x, step_y, step_z) = direction.offset();
        let start = DVec3::new(
            from.x + f64::from(step_x) * NUDGE,
            from.y + f64::from(step_y) * NUDGE,
            from.z + f64::from(step_z) * NUDGE,
        );
        if !occlusion::is_block_in_line(world, start, to, damping::occludes_vibrations) {
            return false;
        }
    }
    true
}

/// Vanilla `VibrationSystem.Ticker.areAdjacentChunksTicking`.
///
/// Requires every chunk in the 3x3 neighbourhood around `pos` to be present and block ticking,
/// exactly like the chunk-source phase that gates Steel's scheduled ticks.
#[must_use]
pub fn are_adjacent_chunks_ticking(world: &World, pos: BlockPos) -> bool {
    let center = ChunkPos::from_block_pos(pos);
    (-1..=1).all(|dx| {
        (-1..=1).all(|dz| {
            world
                .chunk_map
                .is_block_ticking_full_chunk_simulated(ChunkPos::new(
                    center.0.x + dx,
                    center.0.y + dz,
                ))
        })
    })
}

/// Vanilla `CriteriaTriggers.AVOID_VIBRATION` raise when a sneaking player avoids an
/// [`ignores_vibrations_when_sneaking`](crate::world::game_event::vibration::ignores_vibrations_when_sneaking)
/// vibration on a listener whose user returns `canTriggerAvoidVibration` (the sculk sensors).
pub fn notify_avoid_vibration(player: &Player, world: &Arc<World>) {
    let level = WorldLevelView::new(world, player.block_position());
    let equipment = living_loot_equipment(player);
    let known = player.recipe_book().unlocked_identifiers().to_vec();
    let view = player_view(player, &level, Some(&equipment), &known);
    raise(
        player,
        &Identifier::vanilla_static("avoid_vibration"),
        &TriggerEvent::Player(event_base(&view, &level, player.block_position())),
    );
}

#[cfg(test)]
mod tests {
    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_blocks};
    use steel_utils::types::UpdateFlags;
    use steel_utils::{BlockPos, ChunkPos};

    use crate::test_support::{fresh_test_world, insert_ready_full_chunk};

    use super::*;

    fn load_block_ticking_neighbourhood(world: &Arc<World>, pos: BlockPos) {
        let center = ChunkPos::from_block_pos(pos);
        for dx in -1..=1 {
            for dz in -1..=1 {
                if dx == 0 && dz == 0 {
                    continue;
                }
                insert_ready_full_chunk(world, ChunkPos::new(center.0.x + dx, center.0.y + dz));
            }
        }
    }

    #[test]
    fn vibration_is_occluded_only_when_every_offset_ray_hits_wool() {
        init_vanilla_registry();
        let world_fixture = fresh_test_world("vibration_occlusion");
        let world = &world_fixture.world;
        let pos = BlockPos::new(8, 64, 8);
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));
        // The listener sits at `pos`; the vibration originates at `source`, two blocks away on Z.
        let center = DVec3::new(8.5, 64.5, 8.5);
        let source = DVec3::new(8.5, 64.5, 12.5);

        assert!(!is_occluded(world, source, center));

        // Wool directly between the source and the listener: every one of the six nudged rays
        // passes through it, so Vanilla suppresses the vibration.
        world.set_block(
            BlockPos::new(8, 64, 10),
            vanilla_blocks::WHITE_WOOL.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
        assert!(is_occluded(world, source, center));

        // The same wall one block to the side is not on any of the six rays.
        world.set_block(
            BlockPos::new(8, 64, 10),
            vanilla_blocks::AIR.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
        world.set_block(
            BlockPos::new(9, 64, 10),
            vanilla_blocks::WHITE_WOOL.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
        assert!(!is_occluded(world, source, center));
    }

    #[test]
    fn adjacent_chunks_require_block_ticking_readiness() {
        init_vanilla_registry();
        let world_fixture = fresh_test_world("vibration_adjacent_chunks");
        let world = &world_fixture.world;
        let pos = BlockPos::new(8, 64, 8);
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));

        // The listener's own chunk ticks, but its neighbours are not loaded.
        assert!(!are_adjacent_chunks_ticking(world, pos));

        load_block_ticking_neighbourhood(world, pos);
        assert!(are_adjacent_chunks_ticking(world, pos));
    }
}
