//! Vanilla `SitWhenOrderedToGoal` (`goal/SitWhenOrderedToGoal.java`): a tamed animal stays
//! seated while it is ordered to sit and gets up for water, air time, or a distant owner.
//!
//! Vanilla stores the `TamableAnimal`; Steel reads the same state through
//! [`Tameable`](crate::entity::ai::goal::Tameable).

use std::sync::Arc;

use crate::entity::{PathfinderMob, SharedEntity};

use super::selector::{Goal, GoalControls};

/// Vanilla `SitWhenOrderedToGoal.canUse`: the owner distance below which the animal keeps
/// sitting even while the owner fights (`144.0` blocks squared, i.e. 12 blocks).
const OWNER_CLOSE_RANGE_SQR: f64 = 144.0;

/// Vanilla `SitWhenOrderedToGoal`.
pub struct SitWhenOrderedToGoal;

impl SitWhenOrderedToGoal {
    /// Vanilla `new SitWhenOrderedToGoal(tamableMob)`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for SitWhenOrderedToGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for SitWhenOrderedToGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::JUMP | GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(tameable) = mob.as_tameable() else {
            return false;
        };

        let ordered_to_sit = tameable.is_ordered_to_sit();
        if !ordered_to_sit && !tameable.is_tame() {
            return false;
        }
        if mob.is_in_water() {
            return false;
        }
        if !mob.on_ground() {
            return false;
        }

        let Some(owner) = tameable.owner() else {
            return true;
        };
        // Vanilla compares `owner.level() != this.mob.level()`: an owner in another
        // dimension cannot call the animal back.
        if !same_level(mob, &owner) {
            return true;
        }

        let owner_hurt_someone = owner
            .as_living_entity()
            .is_some_and(|owner| owner.last_hurt_by_mob().is_some());
        if mob.position().distance_squared(owner.position()) < OWNER_CLOSE_RANGE_SQR
            && owner_hurt_someone
        {
            return false;
        }

        ordered_to_sit
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        mob.as_tameable()
            .is_some_and(|tameable| tameable.is_ordered_to_sit())
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        mob.mob_base().navigation().lock().stop();
        if let Some(tameable) = mob.as_tameable() {
            tameable.set_in_sitting_pose(true);
        }
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if let Some(tameable) = mob.as_tameable() {
            tameable.set_in_sitting_pose(false);
        }
    }
}

/// Vanilla's `owner.level() == this.mob.level()` identity check.
fn same_level(mob: &dyn PathfinderMob, owner: &SharedEntity) -> bool {
    match (mob.level(), owner.level()) {
        (Some(mob_level), Some(owner_level)) => Arc::ptr_eq(&mob_level, &owner_level),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sit_when_ordered_to_goal_claims_the_jump_and_move_controls() {
        let goal = SitWhenOrderedToGoal::new();

        assert_eq!(goal.controls(), GoalControls::JUMP | GoalControls::MOVE);
    }
}
