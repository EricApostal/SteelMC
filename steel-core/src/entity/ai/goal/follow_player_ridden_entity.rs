//! Vanilla `FollowPlayerRiddenEntityGoal` (`goal/FollowPlayerRiddenEntityGoal.java`): a
//! dolphin trails the boat (or other ridden entity) a player is steering.
//!
//! Vanilla stores the concrete `PathfinderMob` plus the followed entity class; Steel has no
//! class hierarchy over entity types, so the class is a type filter
//! ([`FollowPlayerRiddenEntityGoal::new_matching`]) and the concrete mob arrives as
//! `&dyn PathfinderMob`.

use std::sync::Arc;

use glam::DVec3;
use steel_registry::entity_type::EntityTypeRef;

use super::reduced_tick_delay;
use super::selector::{Goal, GoalControls};
use crate::entity::{Entity, PathfinderMob, SharedEntity};
use crate::physics::MoverType;

/// Vanilla `FollowPlayerRiddenEntityGoal.canContinueToUse`/`start` search box inflation.
const SEARCH_RADIUS: f64 = 5.0;
/// Vanilla `FollowPlayerRiddenEntityGoal.tick` path recalculation interval.
const PATH_RECALC_INTERVAL_TICKS: i32 = 10;
/// Vanilla distance gates between the two sub-goals.
const SWITCH_TO_DIRECTION_DISTANCE: f64 = 4.0;
const SWITCH_TO_ENTITY_DISTANCE: f64 = 12.0;
/// Vanilla `FollowPlayerRiddenEntityGoal` follow look-ahead distance.
const AHEAD_DISTANCE: i32 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FollowEntityGoal {
    GoToEntity,
    GoInEntityDirection,
}

type EntityTypeFilter = Box<dyn Fn(EntityTypeRef) -> bool + Send + Sync>;

/// Vanilla `FollowPlayerRiddenEntityGoal`.
pub struct FollowPlayerRiddenEntityGoal {
    entity_type_to_follow: EntityTypeFilter,
    following: Option<SharedEntity>,
    time_to_recalc_path: i32,
    current_goal: FollowEntityGoal,
}

impl FollowPlayerRiddenEntityGoal {
    /// Vanilla `new FollowPlayerRiddenEntityGoal(mob, entityTypeToFollow)` for a single type.
    #[must_use]
    pub fn new(entity_type_to_follow: EntityTypeRef) -> Self {
        Self::new_matching(move |entity_type| std::ptr::eq(entity_type, entity_type_to_follow))
    }

    /// Vanilla's `Class<? extends Entity>` argument, which matches subclasses: Steel entity
    /// types are flat, so families (`AbstractBoat`, `AbstractNautilus`) are passed as a
    /// predicate over the candidate's type.
    #[must_use]
    pub fn new_matching(
        entity_type_to_follow: impl Fn(EntityTypeRef) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            entity_type_to_follow: Box::new(entity_type_to_follow),
            following: None,
            time_to_recalc_path: 0,
            current_goal: FollowEntityGoal::GoToEntity,
        }
    }

    /// Vanilla `FollowPlayerRiddenEntityGoal.riddenEntities(mob)`: the followed instances near
    /// the mob whose controlling passenger moved recently.
    fn nearby_ridden_entities(&self, mob: &dyn PathfinderMob) -> Vec<SharedEntity> {
        let Some(world) = mob.level() else {
            return Vec::new();
        };

        let matches_type = &self.entity_type_to_follow;
        world.get_entities_in_aabb_matching(&mob.bounding_box().inflate(SEARCH_RADIUS), |entity| {
            matches_type(entity.entity_type())
        })
    }

    /// Vanilla `Entity.hasMovedHorizontallyRecently`:
    /// `Math.abs(lastKnownSpeed.horizontalDistance()) > 1.0E-5F`.
    fn has_moved_horizontally_recently(entity: &dyn Entity) -> bool {
        let known_speed = entity.base().known_speed();
        (known_speed.x * known_speed.x + known_speed.z * known_speed.z).sqrt() > 1.0e-5
    }

    /// Vanilla `Entity.getControllingPassenger` resolved to a player.
    fn controlling_player(entity: &SharedEntity) -> Option<SharedEntity> {
        entity
            .controlling_passenger()
            .filter(|passenger| passenger.as_player().is_some())
    }
}

impl Goal for FollowPlayerRiddenEntityGoal {
    /// Vanilla's constructor sets no flags: the goal drives the mob directly and runs
    /// alongside whatever `MOVE` goal is active.
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if let Some(following) = &self.following {
            if Self::has_moved_horizontally_recently(following.as_ref()) {
                return true;
            }
        }

        self.nearby_ridden_entities(mob).iter().any(|entity| {
            Self::controlling_player(entity)
                .is_some_and(|player| Self::has_moved_horizontally_recently(player.as_ref()))
        })
    }

    fn is_interruptable(&self) -> bool {
        true
    }

    fn can_continue_to_use(&mut self, _mob: &dyn PathfinderMob) -> bool {
        self.following.as_ref().is_some_and(|following| {
            following.is_passenger() && Self::has_moved_horizontally_recently(following.as_ref())
        })
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.following = self
            .nearby_ridden_entities(mob)
            .iter()
            .find_map(Self::controlling_player);
        self.time_to_recalc_path = 0;
        self.current_goal = FollowEntityGoal::GoToEntity;
    }

    fn stop(&mut self, _mob: &dyn PathfinderMob) {
        self.following = None;
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        // Vanilla `FollowPlayerRiddenEntityGoal.tick` starts with the strafe-input push and a
        // self move before the path recalculation.
        let speed = if self.current_goal == FollowEntityGoal::GoInEntityDirection {
            0.01
        } else {
            0.015
        };
        let input = mob.travel_input();
        mob.move_relative(
            speed,
            DVec3::new(
                f64::from(input.sideways()),
                f64::from(input.vertical()),
                f64::from(input.forward()),
            ),
        );
        Arc::clone(entity).move_entity(MoverType::SelfMovement, mob.velocity());

        let Some(following) = self.following.clone() else {
            return;
        };

        self.time_to_recalc_path -= 1;
        if self.time_to_recalc_path > 0 {
            return;
        }
        self.time_to_recalc_path = reduced_tick_delay(PATH_RECALC_INTERVAL_TICKS);

        match self.current_goal {
            FollowEntityGoal::GoToEntity => {
                // Vanilla: one block behind the player, one block down.
                let behind = following.block_position().offset(
                    -following.direction_yaw().offset().0,
                    -1,
                    -following.direction_yaw().offset().2,
                );
                mob.move_to_pos(
                    DVec3::new(
                        f64::from(behind.x()) + 0.5,
                        f64::from(behind.y()),
                        f64::from(behind.z()) + 0.5,
                    ),
                    1.0,
                );
                if mob.position().distance(following.position()) < SWITCH_TO_DIRECTION_DISTANCE {
                    self.time_to_recalc_path = 0;
                    self.current_goal = FollowEntityGoal::GoInEntityDirection;
                }
            }
            FollowEntityGoal::GoInEntityDirection => {
                let forward = following.direction_yaw().offset();
                let ahead = following.block_position().offset(
                    forward.0 * AHEAD_DISTANCE,
                    -1,
                    forward.2 * AHEAD_DISTANCE,
                );
                mob.move_to_pos(
                    DVec3::new(
                        f64::from(ahead.x()) + 0.5,
                        f64::from(ahead.y()),
                        f64::from(ahead.z()) + 0.5,
                    ),
                    1.0,
                );
                if mob.position().distance(following.position()) > SWITCH_TO_ENTITY_DISTANCE {
                    self.time_to_recalc_path = 0;
                    self.current_goal = FollowEntityGoal::GoToEntity;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follow_player_ridden_entity_goal_claims_no_controls_like_vanilla() {
        let goal = FollowPlayerRiddenEntityGoal::new(&steel_registry::vanilla_entities::OAK_BOAT);

        assert_eq!(goal.controls(), GoalControls::EMPTY);
        assert!(goal.is_interruptable());
    }
}
