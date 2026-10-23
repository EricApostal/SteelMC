//! Vanilla `MoveToSkySeeingSpot`: walk to a nearby spot that can see the sky.

use crate::chunk::heightmap::HeightmapType;
use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::tracker::BlockPosTracker;
use crate::entity::ai::brain::tracker::WalkTarget;
use crate::world::World;
use steel_utils::BlockPos;

/// Vanilla `MoveToSkySeeingSpot.create(speedModifier)`.
pub struct MoveToSkySeeingSpot {
    speed_modifier: f32,
}

impl MoveToSkySeeingSpot {
    #[must_use]
    pub fn new(speed_modifier: f32) -> Self {
        Self { speed_modifier }
    }

    /// Vanilla `MoveToSkySeeingSpot.create(speedModifier)`.
    #[must_use]
    pub fn create(speed_modifier: f32) -> OneShot<Self> {
        OneShot::new(Self::new(speed_modifier)).with_required_memories(vec![&mem::WALK_TARGET])
    }

    /// Vanilla `MoveToSkySeeingSpot.getOutdoorPosition`.
    fn outdoor_position(level: &World, body: &SharedEntity) -> Option<BlockPos> {
        let pos = body.block_position();

        for _ in 0..10 {
            let random_pos = pos.offset(
                rand::random_range(0..20) - 10,
                rand::random_range(0..6) - 3,
                rand::random_range(0..20) - 10,
            );
            if has_no_blocks_above(level, body, random_pos) {
                return Some(random_pos);
            }
        }

        None
    }
}

impl Trigger for MoveToSkySeeingSpot {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if ctx.has_memory_value(&mem::WALK_TARGET) {
            return false;
        }

        let body = ctx.body;
        if ctx.level.can_see_sky(body.block_position()) {
            return false;
        }

        if let Some(pos) = Self::outdoor_position(ctx.level, body) {
            let (x, y, z) = pos.get_bottom_center();
            ctx.set_memory(
                &mem::WALK_TARGET,
                WalkTarget::new(
                    std::sync::Arc::new(BlockPosTracker::from_vec(glam::DVec3::new(x, y, z))),
                    self.speed_modifier,
                    0,
                ),
            );
        }

        true
    }
}

/// Vanilla `MoveToSkySeeingSpot.hasNoBlocksAbove`.
#[must_use]
pub fn has_no_blocks_above(level: &World, body: &SharedEntity, target: BlockPos) -> bool {
    level.can_see_sky(target)
        && level
            .height_at(HeightmapType::MotionBlocking, target.x(), target.z())
            .is_some_and(|height| f64::from(height) <= body.position().y)
}
