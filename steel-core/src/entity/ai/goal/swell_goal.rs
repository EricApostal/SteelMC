//! Vanilla `SwellGoal` (`goal/SwellGoal.java`): a creeper swells while its target is close and
//! visible, deflating when the target leaves or breaks line of sight.
//!
//! Vanilla stores the `Creeper` directly; Steel reaches the swell direction through
//! [`Entity::as_creeper`](crate::entity::Entity::as_creeper) so the goal only encodes the
//! vanilla timing logic.

use crate::entity::{PathfinderMob, SharedEntity};

use super::selector::{Goal, GoalControls};

/// Vanilla `SwellGoal.canUse`: maximum target range for starting the fuse.
const SWELL_START_RANGE_SQR: f64 = 9.0;
/// Vanilla `SwellGoal.tick`: the target distance beyond which the creeper deflates.
const SWELL_STOP_RANGE_SQR: f64 = 49.0;

/// Vanilla `SwellGoal`.
pub struct SwellGoal {
    target: Option<SharedEntity>,
}

impl SwellGoal {
    /// Vanilla `new SwellGoal(creeper)`.
    #[must_use]
    pub const fn new() -> Self {
        Self { target: None }
    }
}

impl Default for SwellGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for SwellGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(creeper) = mob.as_creeper() else {
            return false;
        };
        if creeper.swell_dir() > 0 {
            return true;
        }

        let Some(target) = mob.target() else {
            return false;
        };
        // Vanilla `LivingEntity.isDeadOrDying` is `dead || health <= 0`, i.e. not alive.
        if !target.is_alive() {
            return false;
        }

        mob.position().distance_squared(target.position()) < SWELL_START_RANGE_SQR
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        mob.mob_base().navigation().lock().stop();
        self.target = mob.target();
    }

    fn stop(&mut self, _mob: &dyn PathfinderMob) {
        self.target = None;
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(creeper) = mob.as_creeper() else {
            return;
        };

        let hold_fuse = self.target.as_ref().is_some_and(|target| target.is_alive());

        if !hold_fuse {
            creeper.set_swell_dir(-1);
            return;
        }

        let Some(target) = &self.target else {
            creeper.set_swell_dir(-1);
            return;
        };

        if mob.position().distance_squared(target.position()) > SWELL_STOP_RANGE_SQR {
            creeper.set_swell_dir(-1);
        } else if !mob.has_line_of_sight_cached(target.as_ref()) {
            creeper.set_swell_dir(-1);
        } else {
            creeper.set_swell_dir(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swell_goal_claims_only_the_move_control() {
        let goal = SwellGoal::new();

        assert_eq!(goal.controls(), GoalControls::MOVE);
        assert!(goal.requires_update_every_tick());
    }
}
