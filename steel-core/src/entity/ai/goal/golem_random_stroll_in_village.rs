//! Vanilla `GolemRandomStrollInVillageGoal` (`goal/GolemRandomStrollInVillageGoal.java`).
//!
//! Strolls toward a villager who wants a golem (70%), else toward an occupied POI inside a
//! village section (30%), falling back to anywhere.

use glam::DVec3;
use steel_registry::vanilla_entities;
use steel_utils::{BlockPos, SectionPos};

use super::random_pos::{land_random_pos, land_random_pos_towards};
use super::random_stroll::RandomStrollGoal;
use super::selector::{Goal, GoalControls};
use crate::entity::PathfinderMob;
use crate::poi::OccupationStatus;
use crate::world::World;
use crate::world::village::golem::wants_to_spawn_golem;
use crate::world::village::poi::{section_center, sections_in_cube};

const STROLL_INTERVAL: i32 = 240;
const POI_SECTION_SCAN_RADIUS: i32 = 2;
const VILLAGER_SCAN_RADIUS: f64 = 32.0;
const RANDOM_POS_XY_DISTANCE: i32 = 10;
const RANDOM_POS_Y_DISTANCE: i32 = 7;

pub struct GolemRandomStrollInVillageGoal {
    stroll: RandomStrollGoal,
}

impl GolemRandomStrollInVillageGoal {
    #[must_use]
    pub const fn new(speed_modifier: f64) -> Self {
        Self {
            stroll: RandomStrollGoal::with_interval_and_no_action_time_check(
                speed_modifier,
                STROLL_INTERVAL,
                false,
            ),
        }
    }

    /// Vanilla `GolemRandomStrollInVillageGoal.getPosition`.
    fn village_position(mob: &dyn PathfinderMob) -> Option<DVec3> {
        if rand::random::<f32>() < 0.3 {
            return land_random_pos(mob, RANDOM_POS_XY_DISTANCE, RANDOM_POS_Y_DISTANCE);
        }

        let target = if rand::random::<f32>() < 0.7 {
            Self::position_towards_villager_who_wants_golem(mob)
                .or_else(|| Self::position_towards_poi(mob))
        } else {
            Self::position_towards_poi(mob)
                .or_else(|| Self::position_towards_villager_who_wants_golem(mob))
        };

        target.or_else(|| land_random_pos(mob, RANDOM_POS_XY_DISTANCE, RANDOM_POS_Y_DISTANCE))
    }

    /// Vanilla `getPositionTowardsVillagerWhoWantsGolem`.
    fn position_towards_villager_who_wants_golem(mob: &dyn PathfinderMob) -> Option<DVec3> {
        let world = mob.level()?;
        let game_time = world.game_time();
        let search_box = mob.bounding_box().inflate_xyz(
            VILLAGER_SCAN_RADIUS,
            VILLAGER_SCAN_RADIUS,
            VILLAGER_SCAN_RADIUS,
        );
        let villagers: Vec<DVec3> = world
            .get_entities_in_aabb_matching(&search_box, |entity| {
                entity.entity_type() == &vanilla_entities::VILLAGER
            })
            .into_iter()
            .filter(|villager| {
                villager
                    .as_mob()
                    .is_some_and(|mob| wants_to_spawn_golem(mob, game_time))
            })
            .map(|villager| villager.position())
            .collect();

        if villagers.is_empty() {
            return None;
        }

        let target = villagers[rand::random_range(0..villagers.len())];
        land_random_pos_towards(mob, RANDOM_POS_XY_DISTANCE, RANDOM_POS_Y_DISTANCE, target)
    }

    /// Vanilla `getPositionTowardsPoi`.
    fn position_towards_poi(mob: &dyn PathfinderMob) -> Option<DVec3> {
        let world = mob.level()?;
        let section = Self::random_village_section(mob, world.as_ref())?;
        let target = Self::random_poi_within_section(world.as_ref(), section)?;
        land_random_pos_towards(
            mob,
            RANDOM_POS_XY_DISTANCE,
            RANDOM_POS_Y_DISTANCE,
            DVec3::from(target.get_bottom_center()),
        )
    }

    /// Vanilla `getRandomVillageSection`.
    fn random_village_section(mob: &dyn PathfinderMob, world: &World) -> Option<SectionPos> {
        let section_pos = SectionPos::from_block_pos(mob.block_position());
        let sections: Vec<SectionPos> = sections_in_cube(section_pos, POI_SECTION_SCAN_RADIUS)
            .into_iter()
            .filter(|section| world.sections_to_village(*section) == 0)
            .collect();
        if sections.is_empty() {
            return None;
        }

        Some(sections[rand::random_range(0..sections.len())])
    }

    /// Vanilla `getRandomPoiWithinSection`: any occupied POI within 8 blocks of the section center.
    fn random_poi_within_section(world: &World, section: SectionPos) -> Option<BlockPos> {
        let pois = world.poi_storage.lock().get_in_range(
            &|_| true,
            section_center(section),
            8,
            OccupationStatus::Occupied,
        );
        if pois.is_empty() {
            return None;
        }

        Some(pois[rand::random_range(0..pois.len())].1)
    }
}

impl Goal for GolemRandomStrollInVillageGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.stroll
            .can_use_with_position(mob, GolemRandomStrollInVillageGoal::village_position)
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
