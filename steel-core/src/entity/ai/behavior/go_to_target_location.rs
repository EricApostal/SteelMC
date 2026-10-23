//! Vanilla `GoToTargetLocation`: walk to a remembered block position.

use steel_utils::BlockPos;

use super::utils;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::{MemoryModuleType, module_type as mem};

/// Vanilla `GoToTargetLocation.create(MemoryModuleType<BlockPos> locationMemory,
/// int closeEnoughDist, float speedModifier)`: walks to a jittered copy of the
/// remembered block position unless already close enough.
pub struct GoToTargetLocation {
    location_memory: &'static MemoryModuleType<BlockPos>,
    close_enough_dist: i32,
    speed_modifier: f32,
}

impl GoToTargetLocation {
    /// Vanilla `GoToTargetLocation.create(MemoryModuleType<BlockPos>,
    /// int closeEnoughDist, float speedModifier)`.
    #[must_use]
    pub fn create(
        location_memory: &'static MemoryModuleType<BlockPos>,
        close_enough_dist: i32,
        speed_modifier: f32,
    ) -> OneShot<Self> {
        OneShot::new(Self {
            location_memory,
            close_enough_dist,
            speed_modifier,
        })
        .with_required_memories(vec![
            location_memory,
            &mem::ATTACK_TARGET,
            &mem::WALK_TARGET,
            &mem::LOOK_TARGET,
        ])
    }

    /// Vanilla `GoToTargetLocation.getRandomOffset`: `random.nextInt(3) - 1`.
    fn random_offset() -> i32 {
        rand::random_range(0..3) - 1
    }

    /// Vanilla `GoToTargetLocation.getNearbyPos`.
    fn nearby_pos(pos: BlockPos) -> BlockPos {
        pos.offset(Self::random_offset(), 0, Self::random_offset())
    }
}

impl Trigger for GoToTargetLocation {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        // Vanilla entry conditions: `absent(ATTACK_TARGET)`, `absent(WALK_TARGET)`.
        if ctx.has_memory_value(&mem::ATTACK_TARGET) || ctx.has_memory_value(&mem::WALK_TARGET) {
            return false;
        }

        let Some(location) = ctx.get_memory(self.location_memory) else {
            return false;
        };

        let close_enough_to_target = location.closer_to_center_than(
            utils::block_center(ctx.body.block_position()),
            f64::from(self.close_enough_dist),
        );
        if !close_enough_to_target {
            utils::set_walk_and_look_target_to_pos(
                ctx,
                Self::nearby_pos(location),
                self.speed_modifier,
                self.close_enough_dist,
            );
        }

        // Vanilla always reports success, whether or not it set a walk target.
        true
    }
}
