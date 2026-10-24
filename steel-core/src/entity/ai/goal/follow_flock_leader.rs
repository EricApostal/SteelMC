//! Vanilla `FollowFlockLeaderGoal` (`goal/FollowFlockLeaderGoal.java`): a schooling fish that
//! is not already following a leader looks for one nearby and swims behind it.
//!
//! Vanilla stores the concrete `AbstractSchoolingFish`; Steel reads the flock membership
//! through [`SchoolingFish`](crate::entity::ai::goal::SchoolingFish), the capability every
//! schooling fish implements.

use crate::entity::{PathfinderMob, SharedEntity};

use super::capabilities::SchoolingFish;
use super::reduced_tick_delay;
use super::selector::{Goal, GoalControls};

/// Vanilla `FollowFlockLeaderGoal.INTERVAL_TICKS`.
const INTERVAL_TICKS: i32 = 200;
/// Vanilla `FollowFlockLeaderGoal.nextStartTick`: `200 + random.nextInt(200) % 20`.
const RANDOM_START_TICKS: i32 = 200;
const START_TICK_MODULUS: i32 = 20;
/// Vanilla `FollowFlockLeaderGoal.canUse` leader search box inflation.
const LEADER_SEARCH_RADIUS: f64 = 8.0;
/// Vanilla `FollowFlockLeaderGoal.tick` path recalculation interval.
const PATH_RECALC_INTERVAL_TICKS: i32 = 10;

/// Vanilla `FollowFlockLeaderGoal`.
pub struct FollowFlockLeaderGoal {
    time_to_recalc_path: i32,
    next_start_tick: i32,
}

impl FollowFlockLeaderGoal {
    /// Vanilla `new FollowFlockLeaderGoal(mob)`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            time_to_recalc_path: 0,
            next_start_tick: Self::next_start_tick(),
        }
    }

    /// Vanilla `FollowFlockLeaderGoal.nextStartTick(mob)`.
    fn next_start_tick() -> i32 {
        reduced_tick_delay(
            INTERVAL_TICKS + rand::random_range(0..RANDOM_START_TICKS) % START_TICK_MODULUS,
        )
    }

    /// Vanilla `FollowFlockLeaderGoal.canUse` candidate collection: same-class fish that can
    /// be followed or are not followers yet.
    fn nearby_flock_mates(mob: &dyn PathfinderMob) -> Vec<SharedEntity> {
        let Some(world) = mob.level() else {
            return Vec::new();
        };

        let entity_type = mob.entity_type();
        let search_box = mob.bounding_box().inflate(LEADER_SEARCH_RADIUS);
        world.get_entities_in_aabb_matching(&search_box, |entity| {
            entity.entity_type() == entity_type
                && entity
                    .as_schooling_fish()
                    .is_some_and(|fish| fish.can_be_followed() || !fish.is_follower())
        })
    }
}

impl Default for FollowFlockLeaderGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for FollowFlockLeaderGoal {
    /// Vanilla `FollowFlockLeaderGoal` never calls `setFlags`, so it claims no controls.
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(self_fish) = mob.as_schooling_fish() else {
            return false;
        };

        if self_fish.has_followers() {
            return false;
        }
        if self_fish.is_follower() {
            return true;
        }
        if self.next_start_tick > 0 {
            self.next_start_tick -= 1;
            return false;
        }

        self.next_start_tick = Self::next_start_tick();
        let mates = Self::nearby_flock_mates(mob);

        // Vanilla `leader.addFollowers(stream.filter(fish -> !fish.isFollower()))` over the
        // nearby list, which `getEntitiesOfClass` already excludes this fish from; the
        // receiving fish drops itself through `filter(f -> f != this)`.
        let self_id = mob.id();
        let followers: Vec<SharedEntity> = mates
            .iter()
            .filter(|entity| {
                entity.id() != self_id
                    && !entity
                        .as_schooling_fish()
                        .is_some_and(SchoolingFish::is_follower)
            })
            .cloned()
            .collect();

        let leader = mates
            .iter()
            .find(|entity| {
                entity
                    .as_schooling_fish()
                    .is_some_and(SchoolingFish::can_be_followed)
            })
            .cloned();
        match leader {
            Some(leader) => {
                let leader_id = leader.id();
                if let Some(leader_fish) = leader.as_schooling_fish() {
                    leader_fish.add_followers(
                        followers
                            .into_iter()
                            .filter(|fish| fish.id() != leader_id)
                            .collect(),
                    );
                }
            }
            None => self_fish.add_followers(followers),
        }

        self_fish.is_follower()
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        mob.as_schooling_fish()
            .is_some_and(|fish| fish.is_follower() && fish.in_range_of_leader())
    }

    fn start(&mut self, _mob: &dyn PathfinderMob) {
        self.time_to_recalc_path = 0;
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if let Some(fish) = mob.as_schooling_fish() {
            fish.stop_following();
        }
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.time_to_recalc_path -= 1;
        if self.time_to_recalc_path > 0 {
            return;
        }

        self.time_to_recalc_path = reduced_tick_delay(PATH_RECALC_INTERVAL_TICKS);
        if let Some(fish) = mob.as_schooling_fish() {
            fish.path_to_leader(entity);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follow_flock_leader_goal_claims_no_controls() {
        let goal = FollowFlockLeaderGoal::new();

        assert_eq!(goal.controls(), GoalControls::EMPTY);
        assert!(!goal.requires_update_every_tick());
    }
}
