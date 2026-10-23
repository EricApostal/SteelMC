//! Vanilla `MoveBackToVillageGoal` (`goal/MoveBackToVillageGoal.java`).
//!
//! A `RandomStrollGoal` variant that only walks when the mob is outside a village and steers
//! toward the closest village section within two sections of its own.

use std::f64::consts::FRAC_PI_2;

use glam::DVec3;
use steel_utils::SectionPos;

use super::random_pos::default_random_pos_towards;
use super::random_stroll::RandomStrollGoal;
use super::selector::{Goal, GoalControls};
use crate::entity::PathfinderMob;
use crate::world::village::poi::{find_section_closest_to_village, section_center};

const MAX_XZ_DIST: i32 = 10;
const MAX_Y_DIST: i32 = 7;
const STROLL_INTERVAL: i32 = 10;
const VILLAGE_SEARCH_RADIUS: i32 = 2;

pub struct MoveBackToVillageGoal {
    stroll: RandomStrollGoal,
}

impl MoveBackToVillageGoal {
    /// Creates the goal; `check_no_action_time` mirrors vanilla's constructor argument.
    #[must_use]
    pub const fn new(speed_modifier: f64, check_no_action_time: bool) -> Self {
        Self {
            stroll: RandomStrollGoal::with_interval_and_no_action_time_check(
                speed_modifier,
                STROLL_INTERVAL,
                check_no_action_time,
            ),
        }
    }

    /// Vanilla `MoveBackToVillageGoal.getPosition`.
    fn village_position(mob: &dyn PathfinderMob) -> Option<DVec3> {
        let world = mob.level()?;
        let section_pos = SectionPos::from_block_pos(mob.block_position());
        let optimal_section_pos =
            find_section_closest_to_village(world.as_ref(), section_pos, VILLAGE_SEARCH_RADIUS);
        if optimal_section_pos == section_pos {
            return None;
        }

        let (x, y, z) = section_center(optimal_section_pos).get_bottom_center();
        default_random_pos_towards(mob, MAX_XZ_DIST, MAX_Y_DIST, DVec3::new(x, y, z), FRAC_PI_2)
    }
}

impl Goal for MoveBackToVillageGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if mob
            .level()
            .is_some_and(|world| world.is_village(mob.block_position()))
        {
            return false;
        }

        self.stroll
            .can_use_with_position(mob, MoveBackToVillageGoal::village_position)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.stroll.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.stroll.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.stroll.stop(mob);
    }
}
