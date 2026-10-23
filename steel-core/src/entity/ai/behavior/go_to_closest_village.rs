//! Vanilla `GoToClosestVillage`: walk towards the village closest to the body.

use std::sync::Arc;

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_utils::{BlockPos, SectionPos};

use super::utils;
use crate::entity::PathfinderMob;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::tracker::BlockPosTracker;

/// Vanilla `RandomPos.RANDOM_POS_ATTEMPTS`, the attempt count `LandRandomPos` uses.
const RANDOM_POS_ATTEMPTS: usize = 10;

/// Vanilla `GoToClosestVillage`.
pub struct GoToClosestVillage {
    speed_modifier: f32,
    close_enough_distance: i32,
}

impl GoToClosestVillage {
    /// Vanilla `GoToClosestVillage.create(speedModifier, closeEnoughDistance)`.
    #[must_use]
    pub fn create(speed_modifier: f32, close_enough_distance: i32) -> OneShot<Self> {
        OneShot::new(Self {
            speed_modifier,
            close_enough_distance,
        })
        .with_required_memories(vec![&mem::WALK_TARGET])
    }

    /// Vanilla's five weighted attempts: a strictly closer landing position wins
    /// immediately, an equally close one is kept as a fallback.
    fn closest_village_target(
        ctx: &BehaviorContext<'_>,
        current_sections: i32,
    ) -> Option<BlockPos> {
        let mob = ctx.body.as_pathfinder_mob()?;
        let mut target_pos = None;

        for _ in 0..5 {
            let Some(land_pos) = land_random_pos_with_weight(mob, 15, 7, |pos| {
                -f64::from(
                    ctx.level
                        .sections_to_village(SectionPos::from_block_pos(pos)),
                )
            }) else {
                continue;
            };

            let land_sections = ctx
                .level
                .sections_to_village(SectionPos::from_block_pos(land_pos));
            if land_sections < current_sections {
                return Some(land_pos);
            }
            if land_sections == current_sections {
                target_pos = Some(land_pos);
            }
        }

        target_pos
    }
}

impl Trigger for GoToClosestVillage {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if ctx.has_memory_value(&mem::WALK_TARGET) {
            return false;
        }

        let body_pos = ctx.body.block_position();
        if ctx.level.is_village(body_pos) {
            return false;
        }

        let current_sections = ctx
            .level
            .sections_to_village(SectionPos::from_block_pos(body_pos));
        let target = Self::closest_village_target(ctx, current_sections);
        if let Some(pos) = target {
            utils::set_walk_target(
                ctx,
                Arc::new(BlockPosTracker::from_vec(utils::block_bottom_center(pos))),
                self.speed_modifier,
                self.close_enough_distance,
            );
        }
        true
    }
}

/// Vanilla `LandRandomPos.getPos(mob, xz, y, weight)`.
///
/// Candidate generation mirrors [`super::random_pos::land_random_pos`]: uniform
/// random offsets scored by `weight`, with `PathfinderMob::is_stable_destination`
/// standing in for vanilla's pathfinding malus (the goal module's weighted helper is
/// private to `ai::goal`).
fn land_random_pos_with_weight(
    mob: &dyn PathfinderMob,
    horizontal_dist: i32,
    vertical_dist: i32,
    weight: impl Fn(BlockPos) -> f64,
) -> Option<BlockPos> {
    let origin = mob.position();
    let world = mob.level()?;
    let mut best: Option<(f64, BlockPos)> = None;

    for _ in 0..RANDOM_POS_ATTEMPTS {
        let candidate = BlockPos::containing(
            origin.x + f64::from(rand::random_range(-horizontal_dist..=horizontal_dist)),
            origin.y + f64::from(rand::random_range(-vertical_dist..=vertical_dist)),
            origin.z + f64::from(rand::random_range(-horizontal_dist..=horizontal_dist)),
        );

        if !(world.get_block_state(candidate).is_air() && mob.is_stable_destination(candidate)) {
            continue;
        }

        let value = weight(candidate);
        if best
            .as_ref()
            .is_none_or(|(best_value, _)| value > *best_value)
        {
            best = Some((value, candidate));
        }
    }

    best.map(|(_, pos)| pos)
}
