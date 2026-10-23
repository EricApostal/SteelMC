//! Vanilla `LocateHidingPlace`: remember a HOME POI as the place to hide.

use std::sync::Arc;

use steel_registry::vanilla_poi_types;
use steel_utils::BlockPos;

use super::poi::{self, Occupancy, PoiPredicate};
use super::utils;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::tracker::BlockPosTracker;

/// Vanilla `LocateHidingPlace`.
pub struct LocateHidingPlace {
    radius: i32,
    speed_modifier: f32,
    close_enough_dist: i32,
}

impl LocateHidingPlace {
    /// Vanilla `LocateHidingPlace.create(radius, speedModifier, closeEnoughDist)`.
    #[must_use]
    pub fn create(radius: i32, speed_modifier: f32, close_enough_dist: i32) -> OneShot<Self> {
        OneShot::new(Self {
            radius,
            speed_modifier,
            close_enough_dist,
        })
        .with_required_memories(vec![
            &mem::WALK_TARGET,
            &mem::HOME,
            &mem::HIDING_PLACE,
            &mem::PATH,
            &mem::LOOK_TARGET,
            &mem::BREED_TARGET,
            &mem::INTERACTION_TARGET,
        ])
    }

    /// Vanilla's `Optional` chain: the closest HOME within `closeEnoughDist + 1`,
    /// else a random HOME within `radius`, else the remembered HOME.
    fn hiding_pos(&self, ctx: &BehaviorContext<'_>) -> Option<BlockPos> {
        let body_pos = ctx.body.block_position();
        let body_position = ctx.body.position();
        let home = PoiPredicate::Type(&vanilla_poi_types::HOME);

        poi::find_all_closest_first_with_type(
            ctx.level,
            home,
            |_| true,
            body_pos,
            self.close_enough_dist + 1,
            Occupancy::Any,
        )
        .into_iter()
        .map(|(pos, _)| pos)
        .find(|pos| pos.closer_to_center_than(body_position, f64::from(self.close_enough_dist)))
        .or_else(|| {
            // Vanilla `PoiManager.getRandom` shuffles the in-range candidates and
            // takes the first, i.e. a uniform pick among them.
            let candidates = poi::find_all_closest_first_with_type(
                ctx.level,
                home,
                |_| true,
                body_pos,
                self.radius,
                Occupancy::Any,
            );
            if candidates.is_empty() {
                return None;
            }
            Some(candidates[rand::random_range(0..candidates.len())].0)
        })
        .or_else(|| ctx.get_memory(&mem::HOME).map(|home| home.pos))
    }
}

impl Trigger for LocateHidingPlace {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if ctx.has_memory_value(&mem::WALK_TARGET) {
            return false;
        }

        let Some(pos) = self.hiding_pos(ctx) else {
            return true;
        };

        ctx.erase_memory(&mem::PATH);
        ctx.erase_memory(&mem::LOOK_TARGET);
        ctx.erase_memory(&mem::BREED_TARGET);
        ctx.erase_memory(&mem::INTERACTION_TARGET);
        ctx.set_memory(&mem::HIDING_PLACE, poi::global_pos(ctx.level, pos));

        if !pos.closer_to_center_than(ctx.body.position(), f64::from(self.close_enough_dist)) {
            utils::set_walk_target(
                ctx,
                Arc::new(BlockPosTracker::from_block_pos(pos)),
                self.speed_modifier,
                self.close_enough_dist,
            );
        }
        true
    }
}
