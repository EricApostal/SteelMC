//! Vanilla `PathfindToRaidGoal`: raiders outside the village walk towards the raid
//! center and recruit nearby idle raiders.
//!
//! Registered by [`Raider`](crate::entity::raid::Raider) implementations; the
//! `mod`/`pub use` lines in `entity/ai/goal/mod.rs` are owned by the iron-golem slice and
//! are reported rather than added here.

use std::sync::Arc;

use glam::DVec3;

use super::random_pos::{HALF_PI, default_random_pos_towards};
use super::selector::{Goal, GoalControls};
use crate::entity::raid::{RaidSnapshot, can_join_raid};
use crate::entity::{PathfinderMob, SharedEntity};
use crate::world::World;

/// Vanilla `PathfindToRaidGoal.RECRUITMENT_SEARCH_TICK_DELAY`.
const RECRUITMENT_SEARCH_TICK_DELAY: i32 = 20;
/// Vanilla `PathfindToRaidGoal.SPEED_MODIFIER`.
const SPEED_MODIFIER: f32 = 1.0;
/// Vanilla `DefaultRandomPos.getPosTowards` horizontal/vertical bounds.
const RECRUIT_RADIUS: f64 = 16.0;
const TOWARDS_HORIZONTAL_RANGE: i32 = 15;
const TOWARDS_VERTICAL_RANGE: i32 = 4;

/// Vanilla `PathfindToRaidGoal`.
pub struct PathfindToRaidGoal {
    recruitment_tick: i32,
}

impl PathfindToRaidGoal {
    /// Creates the goal for one raider.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            recruitment_tick: 0,
        }
    }
}

impl Default for PathfindToRaidGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for PathfindToRaidGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };
        let Some(raider) = mob.as_raider() else {
            return false;
        };
        if mob.target().is_some() || mob.controlling_passenger().is_some() {
            return false;
        }
        let Some(raid) = raider.current_raid(&world) else {
            return false;
        };
        raid.is_active() && !raid.is_over() && !world.is_village(mob.block_position())
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };
        let Some(raider) = mob.as_raider() else {
            return false;
        };
        raider
            .current_raid(&world)
            .is_some_and(|raid| raid.is_active() && !raid.is_over())
            && !world.is_village(mob.block_position())
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        let Some(world) = mob.level() else {
            return;
        };
        let Some(raider) = mob.as_raider() else {
            return;
        };
        let Some(raid) = raider.current_raid(&world) else {
            return;
        };
        // Vanilla `if (hasActiveRaid())`: a stopped or finished raid does no work here.
        if !raid.is_active() {
            return;
        }

        if mob.tick_count() > self.recruitment_tick {
            // Vanilla uses the raw `RECRUITMENT_SEARCH_TICK_DELAY`, not a tick-adjusted one.
            self.recruitment_tick = mob.tick_count() + RECRUITMENT_SEARCH_TICK_DELAY;
            recruit_nearby(&world, raid, mob, entity);
        }

        if !mob.is_path_finding() {
            let center = raid.center;
            let towards = DVec3::new(
                f64::from(center.x()) + 0.5,
                f64::from(center.y()),
                f64::from(center.z()) + 0.5,
            );
            if let Some(target) = default_random_pos_towards(
                mob,
                TOWARDS_HORIZONTAL_RANGE,
                TOWARDS_VERTICAL_RANGE,
                towards,
                HALF_PI,
            ) {
                mob.move_to_pos(target, f64::from(SPEED_MODIFIER));
            }
        }
    }
}

/// Vanilla `PathfindToRaidGoal.recruitNearby`.
fn recruit_nearby(
    world: &Arc<World>,
    raid: RaidSnapshot,
    mob: &dyn PathfinderMob,
    _entity: &SharedEntity,
) {
    if !raid.is_active() {
        return;
    }

    let area = mob
        .bounding_box()
        .inflate_xyz(RECRUIT_RADIUS, RECRUIT_RADIUS, RECRUIT_RADIUS);
    let candidates = world.get_entities_in_aabb_matching(&area, |entity| {
        entity
            .as_raider()
            .is_some_and(|raider| !raider.has_active_raid(world) && can_join_raid(raider))
    });

    for candidate in candidates {
        world
            .raids()
            .lock()
            .join_raid(world, raid.id, &candidate, raid.groups_spawned, None, true);
    }
}
