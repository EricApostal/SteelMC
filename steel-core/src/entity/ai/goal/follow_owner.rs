//! Vanilla `FollowOwnerGoal` (`goal/FollowOwnerGoal.java`): a tamed animal follows its owner,
//! teleporting when the owner gets too far away.
//!
//! Vanilla stores the `TamableAnimal` and its `PathNavigation`; Steel reads the owner and
//! teleport decisions through [`Tameable`](crate::entity::ai::goal::Tameable) and moves with
//! `PathfinderMob::move_to_pos`, which is the object-safe form of `navigation.moveTo`.
//!
//! Deliberate difference: vanilla's constructor rejects mobs whose navigation is neither
//! `GroundPathNavigation` nor `FlyingPathNavigation`. Steel has a single merged
//! `PathNavigation`, so every tamed mob with a goal selector is supported.

use crate::entity::ai::path::PathType;
use crate::entity::{PathfinderMob, SharedEntity};

use super::reduced_tick_delay;
use super::selector::{Goal, GoalControls};

/// Vanilla `FollowOwnerGoal.tick`: look speed and pitch clamp toward the owner.
const LOOK_Y_MAX_ROT_SPEED: f32 = 10.0;
/// Vanilla `FollowOwnerGoal.tick` path recalculation interval.
const PATH_RECALC_INTERVAL_TICKS: i32 = 10;

/// Vanilla `FollowOwnerGoal`.
pub struct FollowOwnerGoal {
    owner: Option<SharedEntity>,
    speed_modifier: f64,
    time_to_recalc_path: i32,
    stop_distance: f32,
    start_distance: f32,
    old_water_cost: f32,
}

impl FollowOwnerGoal {
    /// Vanilla `new FollowOwnerGoal(tamable, speedModifier, startDistance, stopDistance)`.
    #[must_use]
    pub const fn new(speed_modifier: f64, start_distance: f32, stop_distance: f32) -> Self {
        Self {
            owner: None,
            speed_modifier,
            time_to_recalc_path: 0,
            stop_distance,
            start_distance,
            old_water_cost: 0.0,
        }
    }

    fn stop_distance_sqr(&self) -> f64 {
        f64::from(self.stop_distance) * f64::from(self.stop_distance)
    }
}

impl Goal for FollowOwnerGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(tameable) = mob.as_tameable() else {
            return false;
        };

        let Some(owner) = tameable.owner() else {
            return false;
        };
        if tameable.unable_to_move_to_owner() {
            return false;
        }

        let start_distance = f64::from(self.start_distance);
        if mob.position().distance_squared(owner.position()) < start_distance * start_distance {
            return false;
        }

        self.owner = Some(owner);
        true
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(owner) = &self.owner else {
            return false;
        };
        if mob.mob_base().navigation().lock().is_done() {
            return false;
        }
        if mob
            .as_tameable()
            .is_some_and(|tameable| tameable.unable_to_move_to_owner())
        {
            return false;
        }

        mob.position().distance_squared(owner.position()) > self.stop_distance_sqr()
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.time_to_recalc_path = 0;
        self.old_water_cost = mob.get_pathfinding_malus(PathType::Water);
        mob.set_pathfinding_malus(PathType::Water, 0.0);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.owner = None;
        mob.mob_base().navigation().lock().stop();
        mob.set_pathfinding_malus(PathType::Water, self.old_water_cost);
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(owner) = self.owner.clone() else {
            return;
        };

        let owner_far_away = mob
            .as_tameable()
            .is_some_and(|tameable| tameable.should_try_teleport_to_owner());

        if !owner_far_away {
            let owner_position = owner.position();
            mob.mob_base().controls().lock().look_control.set_look_at(
                glam::DVec3::new(owner_position.x, owner.get_eye_y(), owner_position.z),
                LOOK_Y_MAX_ROT_SPEED,
                mob.max_head_x_rot(),
            );
        }

        self.time_to_recalc_path -= 1;
        if self.time_to_recalc_path > 0 {
            return;
        }
        self.time_to_recalc_path = reduced_tick_delay(PATH_RECALC_INTERVAL_TICKS);

        if owner_far_away {
            if let Some(tameable) = mob.as_tameable() {
                tameable.try_to_teleport_to_owner();
            }
        } else {
            mob.move_to_pos(owner.position(), self.speed_modifier);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follow_owner_goal_uses_move_and_look_controls() {
        let goal = FollowOwnerGoal::new(1.0, 10.0, 2.0);

        assert_eq!(goal.controls(), GoalControls::MOVE | GoalControls::LOOK);
        assert_eq!(goal.stop_distance_sqr(), 4.0);
    }
}
