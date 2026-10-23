//! Vanilla `RandomStandGoal` (`goal/RandomStandGoal.java`): a horse occasionally rears and
//! plays its ambient stand sound.
//!
//! Vanilla stores the `AbstractHorse`; Steel reads the stand state through
//! [`AbstractHorse`](crate::entity::ai::goal::AbstractHorse).

use super::selector::{Goal, GoalControls};
use crate::entity::PathfinderMob;

/// Vanilla `RandomStandGoal.canUse`: the random gate is `nextInt(1000) < nextStand`, and the
/// goal also needs `nextInt(10) == 0`.
const STAND_CHANCE_BOUND: i32 = 1000;
const STAND_RETRY_BOUND: i32 = 10;

/// Vanilla `RandomStandGoal`.
pub struct RandomStandGoal {
    next_stand: i32,
}

impl RandomStandGoal {
    /// Vanilla `new RandomStandGoal(horse)`: the counter starts one interval below zero, so the
    /// goal stays dormant for `ambient_stand_interval` ticks.
    #[must_use]
    pub const fn new(ambient_stand_interval: i32) -> Self {
        Self {
            next_stand: -ambient_stand_interval,
        }
    }

    /// Vanilla `RandomStandGoal.resetStandInterval`: `nextStand = -getAmbientStandInterval()`.
    fn reset_stand_interval(&mut self, mob: &dyn PathfinderMob) {
        self.next_stand = -mob
            .as_abstract_horse()
            .map_or(0, |horse| horse.ambient_stand_interval());
    }
}

impl Default for RandomStandGoal {
    fn default() -> Self {
        Self::new(0)
    }
}

impl Goal for RandomStandGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(horse) = mob.as_abstract_horse() else {
            return false;
        };

        self.next_stand += 1;
        if self.next_stand <= 0 || rand::random_range(0..STAND_CHANCE_BOUND) >= self.next_stand {
            return false;
        }

        self.reset_stand_interval(mob);
        !horse.is_immobile() && rand::random_range(0..STAND_RETRY_BOUND) == 0
    }

    fn can_continue_to_use(&mut self, _mob: &dyn PathfinderMob) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let Some(horse) = mob.as_abstract_horse() else {
            return;
        };
        horse.stand_if_possible();
        if let Some(sound) = horse.ambient_stand_sound() {
            mob.play_sound(sound, 1.0, 1.0);
        }
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_stand_goal_claims_no_controls() {
        let goal = RandomStandGoal::new(180);

        assert_eq!(goal.controls(), GoalControls::EMPTY);
        assert!(goal.requires_update_every_tick());
    }
}
