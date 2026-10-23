//! Vanilla `VillageBoundRandomStroll`: stroll inside, or towards, the nearest village.

use std::f64::consts::FRAC_PI_2;
use std::sync::Arc;

use steel_utils::SectionPos;

use super::random_pos;
use super::utils;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::tracker::{BlockPosTracker, WalkTarget};
use crate::world::village::{find_section_closest_to_village, section_center};

/// Vanilla `VillageBoundRandomStroll.MAX_XZ_DIST`.
const MAX_XZ_DIST: i32 = 10;
/// Vanilla `VillageBoundRandomStroll.MAX_Y_DIST`.
const MAX_Y_DIST: i32 = 7;
/// The `findSectionClosestToVillage(level, sectionPos, 2)` radius in vanilla's lambda.
const SECTION_SEARCH_RADIUS: i32 = 2;

/// Vanilla `VillageBoundRandomStroll`.
pub struct VillageBoundRandomStroll {
    speed_modifier: f32,
    max_xz_dist: i32,
    max_y_dist: i32,
}

impl VillageBoundRandomStroll {
    /// Vanilla `VillageBoundRandomStroll.create(speedModifier)`.
    #[must_use]
    pub fn create(speed_modifier: f32) -> OneShot<Self> {
        Self::create_with_params(speed_modifier, MAX_XZ_DIST, MAX_Y_DIST)
    }

    /// Vanilla `VillageBoundRandomStroll.create(speedModifier, maxXyDist, maxYDist)`.
    #[must_use]
    pub fn create_with_params(
        speed_modifier: f32,
        max_xz_dist: i32,
        max_y_dist: i32,
    ) -> OneShot<Self> {
        OneShot::new(Self {
            speed_modifier,
            max_xz_dist,
            max_y_dist,
        })
        .with_required_memories(vec![&mem::WALK_TARGET])
    }

    /// The stroll target of vanilla's lambda: a landing position in a village, or
    /// one towards the closest village section when outside of one.
    fn stroll_target(
        ctx: &BehaviorContext<'_>,
        max_xz_dist: i32,
        max_y_dist: i32,
    ) -> Option<glam::DVec3> {
        let mob = utils::pathfinder_mob(ctx)?;
        let body_pos = ctx.body.block_position();

        if ctx.level.is_village(body_pos) {
            return random_pos::land_random_pos(mob, max_xz_dist, max_y_dist);
        }

        let section_pos = SectionPos::from_block_pos(body_pos);
        let optimal_section_pos =
            find_section_closest_to_village(ctx.level, section_pos, SECTION_SEARCH_RADIUS);
        if optimal_section_pos != section_pos {
            let towards = utils::block_bottom_center(section_center(optimal_section_pos));
            random_pos::default_random_pos_towards(mob, max_xz_dist, max_y_dist, towards, FRAC_PI_2)
        } else {
            random_pos::land_random_pos(mob, max_xz_dist, max_y_dist)
        }
    }
}

impl Trigger for VillageBoundRandomStroll {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if ctx.has_memory_value(&mem::WALK_TARGET) {
            return false;
        }

        let target = Self::stroll_target(ctx, self.max_xz_dist, self.max_y_dist);
        ctx.set_memory_or_erase(
            &mem::WALK_TARGET,
            target.map(|pos| {
                WalkTarget::new(
                    Arc::new(BlockPosTracker::from_vec(pos)),
                    self.speed_modifier,
                    0,
                )
            }),
        );
        true
    }
}
