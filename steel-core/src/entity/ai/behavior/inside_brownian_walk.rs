//! Vanilla `InsideBrownianWalk`: wander to a nearby spot that cannot see the sky.

use std::sync::Arc;

use rand::seq::SliceRandom as _;
use steel_utils::BlockPos;

use super::utils;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::tracker::BlockPosTracker;

/// Vanilla `InsideBrownianWalk`.
pub struct InsideBrownianWalk {
    speed_modifier: f32,
}

impl InsideBrownianWalk {
    /// Vanilla `InsideBrownianWalk.create(speedModifier)`.
    #[must_use]
    pub fn create(speed_modifier: f32) -> OneShot<Self> {
        OneShot::new(Self { speed_modifier }).with_required_memories(vec![&mem::WALK_TARGET])
    }

    /// Vanilla's shuffled offset scan: a neighbour that also cannot see the sky.
    ///
    /// Gap: Steel has no `Level.noCollision(Entity)` counterpart, so that filter is
    /// dropped; the stand-ability filter uses `PathfinderMob::is_stable_destination`
    /// (block below is solid) in place of `LoadedAndEntityCanStandOn`.
    fn hidden_neighbour(ctx: &BehaviorContext<'_>, body_pos: BlockPos) -> Option<BlockPos> {
        let mob = ctx.body.as_pathfinder_mob()?;
        let mut poses = Vec::with_capacity(27);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    poses.push(body_pos.offset(dx, dy, dz));
                }
            }
        }
        poses.shuffle(&mut rand::rng());

        poses
            .into_iter()
            .find(|pos| !ctx.level.can_see_sky(*pos) && mob.is_stable_destination(*pos))
    }
}

impl Trigger for InsideBrownianWalk {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if ctx.has_memory_value(&mem::WALK_TARGET) {
            return false;
        }

        let body_pos = ctx.body.block_position();
        if ctx.level.can_see_sky(body_pos) {
            return false;
        }

        let target = Self::hidden_neighbour(ctx, body_pos);
        if let Some(pos) = target {
            utils::set_walk_target(
                ctx,
                Arc::new(BlockPosTracker::from_block_pos(pos)),
                self.speed_modifier,
                0,
            );
        }
        true
    }
}
