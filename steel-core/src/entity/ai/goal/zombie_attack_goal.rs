//! Vanilla `ZombieAttackGoal` (`goal/ZombieAttackGoal.java`): the zombie melee goal that
//! raises its arms as the next hit approaches.
//!
//! Vanilla extends `MeleeAttackGoal` with the concrete `Zombie`; Steel embeds the landed
//! [`MeleeAttackGoal`] and reads its attack timer through the module-private accessors, since
//! Steel goals cannot subclass.
//!
//! Vanilla also sets the zombie's aggressive flag (`Mob.setAggressive`), which Steel's
//! [`Mob`](crate::entity::Mob) already exposes.

use crate::entity::{PathfinderMob, SharedEntity};

use super::melee_attack::MeleeAttackGoal;
use super::selector::{Goal, GoalControls};

/// Vanilla `ZombieAttackGoal.tick`: ticks before the raised-arm window can start.
const RAISE_ARM_TICKS: i32 = 5;

/// Vanilla `ZombieAttackGoal`.
pub struct ZombieAttackGoal {
    melee: MeleeAttackGoal,
    raise_arm_ticks: i32,
}

impl ZombieAttackGoal {
    /// Vanilla `new ZombieAttackGoal(zombie, speedModifier, trackTarget)`.
    #[must_use]
    pub(crate) const fn new(speed_modifier: f64, track_target: bool) -> Self {
        Self {
            melee: MeleeAttackGoal::new(speed_modifier, track_target),
            raise_arm_ticks: 0,
        }
    }
}

impl Goal for ZombieAttackGoal {
    fn controls(&self) -> GoalControls {
        self.melee.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.melee.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.melee.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.melee.start(mob);
        self.raise_arm_ticks = 0;
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.melee.stop(mob);
        mob.set_aggressive(false);
    }

    fn requires_update_every_tick(&self) -> bool {
        self.melee.requires_update_every_tick()
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.melee.tick(mob, entity);
        self.raise_arm_ticks += 1;

        let interval = MeleeAttackGoal::attack_interval();
        let raise_arms = self.raise_arm_ticks >= RAISE_ARM_TICKS
            && self.melee.get_ticks_until_next_attack() < interval / 2;
        mob.set_aggressive(raise_arms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zombie_attack_goal_delegates_the_melee_controls() {
        let goal = ZombieAttackGoal::new(1.0, false);

        assert_eq!(goal.controls(), GoalControls::MOVE | GoalControls::LOOK);
        assert!(goal.requires_update_every_tick());
        assert_eq!(goal.raise_arm_ticks, 0);
    }
}
