//! Vanilla `FollowPlayerRiddenEntityGoal` (`goal/FollowPlayerRiddenEntityGoal.java`): a
//! dolphin trails the boat (or other ridden entity) a player is steering.
//!
//! Vanilla stores the concrete `PathfinderMob` plus the followed entity class; Steel takes
//! that class as a constructor entity-type filter and stays otherwise identical.
//!
//! Deliberate difference: vanilla's `tick` starts with
//! `mob.moveRelative(speed, new Vec3(xxa, yya, zza))`, using the mob's strafe inputs. Steel
//! mobs do not retain `xxa/yya/zza`, and this goal's movement comes entirely from the
//! navigation calls below, so that no-op step is omitted.

use glam::DVec3;
use steel_registry::entity_type::EntityTypeRef;

use super::reduced_tick_delay;
use super::selector::{Goal, GoalControls};
use crate::entity::{Entity, PathfinderMob, SharedEntity};

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

/// Vanilla `FollowPlayerRiddenEntityGoal`.
pub struct FollowPlayerRiddenEntityGoal {
    entity_type_to_follow: EntityTypeRef,
    following: Option<SharedEntity>,
    time_to_recalc_path: i32,
    current_goal: FollowEntityGoal,
}

impl FollowPlayerRiddenEntityGoal {
    /// Vanilla `new FollowPlayerRiddenEntityGoal(mob, entityTypeToFollow)`.
    #[must_use]
    pub const fn new(entity_type_to_follow: EntityTypeRef) -> Self {
        Self {
            entity_type_to_follow,
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

        let entity_type = self.entity_type_to_follow;
        world.get_entities_in_aabb_matching(&mob.bounding_box().inflate(SEARCH_RADIUS), |entity| {
            std::ptr::eq(entity.entity_type(), entity_type)
        })
    }

    /// Vanilla `Entity.hasMovedHorizontallyRecently`: `|lastKnownSpeed| > 1.0E-5`.
    fn has_moved_horizontally_recently(entity: &dyn Entity) -> bool {
        entity.base().known_speed().length_squared() > 1.0e-5
    }

    /// Vanilla `Entity.getControllingPassenger` resolved to a player.
    fn controlling_player(entity: &SharedEntity) -> Option<SharedEntity> {
        entity
            .controlling_passenger()
            .filter(|passenger| passenger.as_player().is_some())
    }
}

impl Goal for FollowPlayerRiddenEntityGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
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

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
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
    fn follow_player_ridden_entity_goal_claims_only_the_move_control() {
        let goal = FollowPlayerRiddenEntityGoal::new(&steel_registry::vanilla_entities::OAK_BOAT);

        assert_eq!(goal.controls(), GoalControls::MOVE);
        assert!(goal.is_interruptable());
    }
}
