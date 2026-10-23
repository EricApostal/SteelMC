//! Vanilla `RunAroundLikeCrazyGoal` (`goal/RunAroundLikeCrazyGoal.java`): an untamed horse
//! with a rider bucks around and eventually throws the player off.
//!
//! Vanilla stores the `AbstractHorse`; Steel reads the tame/temper state through
//! [`AbstractHorse`](crate::entity::ai::goal::AbstractHorse) and picks the target with the
//! shared `DefaultRandomPos` port.

use glam::DVec3;

use super::random_pos::default_random_pos;
use super::selector::{Goal, GoalControls};
use crate::entity::{PathfinderMob, SharedEntity};

/// Vanilla `RunAroundLikeCrazyGoal` random position search distances.
const RANDOM_POS_HORIZONTAL: i32 = 5;
const RANDOM_POS_VERTICAL: i32 = 4;
/// Vanilla `RunAroundLikeCrazyGoal.tick` buck-off roll interval (`adjustedTickDelay(50)`).
const BUCK_OFF_INTERVAL_TICKS: i32 = 25;
/// Vanilla `RunAroundLikeCrazyGoal.tick` temper reward for staying on.
const TEMPER_BONUS: i32 = 5;

/// Vanilla `RunAroundLikeCrazyGoal`.
pub struct RunAroundLikeCrazyGoal {
    speed_modifier: f64,
    pos_x: f64,
    pos_y: f64,
    pos_z: f64,
}

impl RunAroundLikeCrazyGoal {
    /// Vanilla `new RunAroundLikeCrazyGoal(mob, speedModifier)`.
    #[must_use]
    pub const fn new(speed_modifier: f64) -> Self {
        Self {
            speed_modifier,
            pos_x: 0.0,
            pos_y: 0.0,
            pos_z: 0.0,
        }
    }
}

impl Goal for RunAroundLikeCrazyGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(horse) = mob.as_abstract_horse() else {
            return false;
        };
        if horse.is_mob_controlled() || horse.is_tamed() || !mob.is_vehicle() {
            return false;
        }

        let Some(pos) = default_random_pos(mob, RANDOM_POS_HORIZONTAL, RANDOM_POS_VERTICAL) else {
            return false;
        };

        self.pos_x = pos.x;
        self.pos_y = pos.y;
        self.pos_z = pos.z;
        true
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(horse) = mob.as_abstract_horse() else {
            return false;
        };
        !horse.is_tamed() && !mob.mob_base().navigation().lock().is_done() && mob.is_vehicle()
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        mob.move_to_pos(
            DVec3::new(self.pos_x, self.pos_y, self.pos_z),
            self.speed_modifier,
        );
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(horse) = mob.as_abstract_horse() else {
            return;
        };
        if horse.is_tamed() || rand::random_range(0..BUCK_OFF_INTERVAL_TICKS) != 0 {
            return;
        }

        let Some(passenger) = mob.first_passenger() else {
            return;
        };
        if passenger.as_player().is_none() {
            return;
        };

        // Vanilla `AbstractHorse.tameWithName` succeeds when the tem-per roll beats the
        // player; otherwise the horse gets angrier and throws them.
        let max_temper = horse.max_temper();
        if max_temper > 0 && rand::random_range(0..max_temper) < horse.temper() {
            horse.tame_with_name(&passenger);
            return;
        }

        horse.modify_temper(TEMPER_BONUS);
        horse.eject_passengers();
        horse.make_mad(&passenger);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_around_like_crazy_goal_claims_only_the_move_control() {
        let goal = RunAroundLikeCrazyGoal::new(1.2);

        assert_eq!(goal.controls(), GoalControls::MOVE);
    }
}
