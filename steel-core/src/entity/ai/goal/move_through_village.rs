//! Vanilla `MoveThroughVillageGoal` (`goal/MoveThroughVillageGoal.java`).
//!
//! A mob near a village (zombies, at night) paths to an unvisited village POI, optionally opening
//! doors on the way, and remembers the POIs it already reached.

use glam::DVec3;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_utils::BlockPos;

use super::random_pos::{HALF_PI, default_random_pos_towards, land_random_pos_with_weight};
use super::selector::{Goal, GoalControls};
use crate::behavior::BLOCK_BEHAVIORS;
use crate::entity::PathfinderMob;
use crate::entity::ai::path::Path;
use crate::poi::{OccupationStatus, PointOfInterestStorage};
use crate::world::village::poi::is_village_poi;

const MAX_VISITED: usize = 15;
const POI_SEARCH_RADIUS: i32 = 10;
const STROLL_RANGE_XZ: i32 = 15;
const STROLL_RANGE_Y: i32 = 7;
const CLOSE_TO_VILLAGE: i32 = 6;

pub struct MoveThroughVillageGoal {
    speed_modifier: f64,
    path: Option<Path>,
    poi_pos: BlockPos,
    only_at_night: bool,
    visited: Vec<BlockPos>,
    distance_to_poi: i32,
    can_deal_with_doors: bool,
}

impl MoveThroughVillageGoal {
    #[must_use]
    pub const fn new(
        speed_modifier: f64,
        only_at_night: bool,
        distance_to_poi: i32,
        can_deal_with_doors: bool,
    ) -> Self {
        Self {
            speed_modifier,
            path: None,
            poi_pos: BlockPos::ZERO,
            only_at_night,
            visited: Vec::new(),
            distance_to_poi,
            can_deal_with_doors,
        }
    }

    fn has_not_visited(&self, poi: BlockPos) -> bool {
        !self.visited.contains(&poi)
    }

    fn update_visited(&mut self) {
        if self.visited.len() > MAX_VISITED {
            self.visited.remove(0);
        }
    }

    /// Vanilla `MoveThroughVillageGoal.canUse`'s POI search around each candidate position
    /// (vanilla `PoiManager.find`, i.e. the first matching record in range order).
    fn find_unvisited_poi(
        storage: &PointOfInterestStorage,
        center: BlockPos,
        has_not_visited: &dyn Fn(BlockPos) -> bool,
    ) -> Option<BlockPos> {
        storage
            .get_in_range(
                &is_village_poi,
                center,
                POI_SEARCH_RADIUS,
                OccupationStatus::Occupied,
            )
            .into_iter()
            .map(|(_, pos)| pos)
            .find(|pos| has_not_visited(*pos))
    }
}

/// Vanilla `BlockPos.distSqr` (`distToLowCornerSqr`): squared distance between low corners.
fn dist_sqr(a: BlockPos, b: BlockPos) -> f64 {
    let (ax, ay, az) = (a.x(), a.y(), a.z());
    let (bx, by, bz) = (b.x(), b.y(), b.z());
    DVec3::new(f64::from(ax), f64::from(ay), f64::from(az)).distance_squared(DVec3::new(
        f64::from(bx),
        f64::from(by),
        f64::from(bz),
    ))
}

impl Goal for MoveThroughVillageGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if !super::door_interact::has_ground_path_navigation(mob) {
            return false;
        }

        self.update_visited();
        let Some(world) = mob.level() else {
            return false;
        };
        if self.only_at_night && world.is_bright_outside() {
            return false;
        }

        let pos = mob.block_position();
        if !world.is_close_to_village(pos, CLOSE_TO_VILLAGE) {
            return false;
        }

        let visited = self.visited.clone();
        let has_not_visited = move |poi: BlockPos| !visited.contains(&poi);
        let land_pos =
            land_random_pos_with_weight(mob, STROLL_RANGE_XZ, STROLL_RANGE_Y, |candidate| {
                if !world.is_village(candidate) {
                    return f64::NEG_INFINITY;
                }

                let storage = world.poi_storage.lock();
                MoveThroughVillageGoal::find_unvisited_poi(&storage, candidate, &has_not_visited)
                    .map_or(f64::NEG_INFINITY, |poi| -dist_sqr(poi, pos))
            });
        let Some(land_pos) = land_pos else {
            return false;
        };

        let target = {
            let storage = world.poi_storage.lock();
            MoveThroughVillageGoal::find_unvisited_poi(
                &storage,
                BlockPos::containing(land_pos.x, land_pos.y, land_pos.z),
                &has_not_visited,
            )
        };
        let Some(target) = target else {
            return false;
        };

        self.poi_pos = target;
        let navigation_can_open_doors = self.can_deal_with_doors;
        {
            let mut navigation = mob.mob_base().navigation().lock();
            navigation.set_can_open_doors(navigation_can_open_doors);
        }
        self.path = mob.create_path_to(self.poi_pos, 0);
        mob.mob_base().navigation().lock().set_can_open_doors(true);

        if self.path.is_none() {
            let (x, y, z) = self.poi_pos.get_bottom_center();
            let partial = default_random_pos_towards(mob, 10, 7, DVec3::new(x, y, z), HALF_PI);
            let Some(partial) = partial else {
                return false;
            };

            {
                let mut navigation = mob.mob_base().navigation().lock();
                navigation.set_can_open_doors(navigation_can_open_doors);
            }
            self.path =
                mob.create_path_to(BlockPos::containing(partial.x, partial.y, partial.z), 0);
            mob.mob_base().navigation().lock().set_can_open_doors(true);
            if self.path.is_none() {
                return false;
            }
        }

        let nodes: Vec<BlockPos> = self
            .path
            .as_ref()
            .map(|path| {
                (0..path.node_count())
                    .filter_map(|index| {
                        path.node(index)
                            .map(|node| BlockPos::new(node.x, node.y, node.z))
                    })
                    .collect()
            })
            .unwrap_or_default();
        for node in nodes {
            let door_pos = node.above();
            let state = world.get_block_state(door_pos);
            if BLOCK_BEHAVIORS
                .get_behavior(state.get_block())
                .is_wooden_door(state)
            {
                self.path = mob.create_path_to(node, 0);
                break;
            }
        }

        self.path.is_some()
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if mob.mob_base().navigation().lock().is_done() {
            return false;
        }

        let reach = f64::from(mob.bounding_box().width()) + f64::from(self.distance_to_poi);
        !self.poi_pos.closer_to_center_than(mob.position(), reach)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let path = self.path.take();
        mob.move_to_path(path, self.speed_modifier);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        let done = mob.mob_base().navigation().lock().is_done();
        if done
            || self
                .poi_pos
                .closer_to_center_than(mob.position(), f64::from(self.distance_to_poi))
        {
            self.visited.push(self.poi_pos);
        }
    }
}
