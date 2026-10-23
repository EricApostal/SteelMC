//! Vanilla `StrollThroughVillageGoal` (`goal/StrollThroughVillageGoal.java`).
//!
//! At night, a mob already near a village walks to a spot whose village distance is as small as
//! possible (`LandRandomPos` weight `-sectionsToVillage`); once the path is done it keeps pushing
//! toward the destination in 10-block steps.

use glam::DVec3;
use steel_utils::{BlockPos, SectionPos};

use super::random_pos::land_random_pos_with_weight;
use super::reduced_tick_delay;
use super::selector::{Goal, GoalControls};
use crate::chunk::heightmap::HeightmapType;
use crate::entity::PathfinderMob;

const DISTANCE_THRESHOLD: f64 = 10.0;
const LONG_DISTANCE_SCALE: f64 = 0.4;
const LONG_DISTANCE_STEP: f64 = 10.0;
const RANDOM_STEP_RADIUS: i32 = 8;
const CLOSE_TO_VILLAGE: i32 = 6;
const STROLL_RANGE_XZ: i32 = 15;
const STROLL_RANGE_Y: i32 = 7;

pub struct StrollThroughVillageGoal {
    interval: i32,
    wanted_pos: Option<BlockPos>,
}

impl StrollThroughVillageGoal {
    #[must_use]
    pub const fn new(interval: i32) -> Self {
        Self {
            interval: reduced_tick_delay(interval),
            wanted_pos: None,
        }
    }

    /// Vanilla `StrollThroughVillageGoal.moveRandomly`.
    fn move_randomly(&self, mob: &dyn PathfinderMob, world: &crate::world::World) {
        let pos = mob.block_position();
        let target = BlockPos::new(
            pos.x() - RANDOM_STEP_RADIUS + rand::random_range(0..2 * RANDOM_STEP_RADIUS),
            pos.y(),
            pos.z() - RANDOM_STEP_RADIUS + rand::random_range(0..2 * RANDOM_STEP_RADIUS),
        );
        // Vanilla `getHeightmapPos` fails on an unloaded column; fall back to the target's own y.
        let surface = world
            .height_at(
                HeightmapType::MotionBlockingNoLeaves,
                target.x(),
                target.z(),
            )
            .unwrap_or(target.y());
        mob.move_to_pos(
            DVec3::new(
                f64::from(target.x()),
                f64::from(surface),
                f64::from(target.z()),
            ),
            1.0,
        );
    }
}

impl Goal for StrollThroughVillageGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if mob.has_controlling_passenger() {
            return false;
        }
        let Some(world) = mob.level() else {
            return false;
        };
        if world.is_bright_outside() {
            return false;
        }
        if self.interval > 0 && rand::random_range(0..self.interval) != 0 {
            return false;
        }
        if !world.is_close_to_village(mob.block_position(), CLOSE_TO_VILLAGE) {
            return false;
        }

        let wanted = land_random_pos_with_weight(mob, STROLL_RANGE_XZ, STROLL_RANGE_Y, |pos| {
            -f64::from(world.sections_to_village(SectionPos::from_block_pos(pos)))
        });
        self.wanted_pos = wanted.map(|pos| BlockPos::containing(pos.x, pos.y, pos.z));
        self.wanted_pos.is_some()
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(wanted_pos) = self.wanted_pos else {
            return false;
        };
        let navigation = mob.mob_base().navigation().lock();
        !navigation.is_done() && navigation.target_pos() == Some(wanted_pos)
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &crate::entity::SharedEntity) {
        let Some(wanted_pos) = self.wanted_pos else {
            return;
        };
        let Some(world) = mob.level() else {
            return;
        };
        if !mob.mob_base().navigation().lock().is_done() {
            return;
        }

        let position = mob.position();
        if wanted_pos.closer_to_center_than(position, DISTANCE_THRESHOLD) {
            return;
        }

        let (target_x, target_y, target_z) = wanted_pos.get_bottom_center();
        let target = DVec3::new(target_x, target_y, target_z);
        let distance = position - target;
        let scaled = distance * LONG_DISTANCE_SCALE + target;
        let move_target = (scaled - position).normalize_or_zero() * LONG_DISTANCE_STEP + position;
        let path_target = BlockPos::containing(move_target.x, move_target.y, move_target.z);
        let surface = world
            .height_at(
                HeightmapType::MotionBlockingNoLeaves,
                path_target.x(),
                path_target.z(),
            )
            .unwrap_or(path_target.y());
        let surface_pos = BlockPos::new(path_target.x(), surface, path_target.z());
        // Vanilla `navigation.moveTo(pathTarget.getX(), pathTarget.getY(), pathTarget.getZ(), 1.0)`.
        let moved = mob.move_to_pos(
            DVec3::new(
                f64::from(surface_pos.x()),
                f64::from(surface_pos.y()),
                f64::from(surface_pos.z()),
            ),
            1.0,
        );
        if !moved {
            self.move_randomly(mob, world.as_ref());
        }
    }
}
