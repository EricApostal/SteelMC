//! `NearestAttackableTargetGoal<Player>` filtered by vanilla `NeutralMob.isAngryAt`.
//!
//! Vanilla registers `new NearestAttackableTargetGoal<>(this, Player.class, 10, true, false,
//! this::isAngryAt)` on the iron golem. Steel's `NearestAttackableTargetGoal` selector only sees
//! the candidate, not the mob, so the anger gate lives in this dedicated goal.

use super::super::reduced_tick_delay;
use super::super::selector::{Goal, GoalControls};
use super::super::target_goal::{TargetGoalBase, follow_distance};
use crate::entity::ai::targeting::TargetingConditions;
use crate::entity::{PathfinderMob, SharedEntity};

const RANDOM_INTERVAL: i32 = 10;

pub(crate) struct AngryAtPlayerTargetGoal {
    target_goal: TargetGoalBase,
    random_interval: i32,
    target: Option<SharedEntity>,
    conditions: TargetingConditions,
}

impl AngryAtPlayerTargetGoal {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            // Vanilla `NearestAttackableTargetGoal(mob, Player.class, 10, true, false, ...)`.
            target_goal: TargetGoalBase::new(true, false),
            random_interval: reduced_tick_delay(RANDOM_INTERVAL),
            target: None,
            conditions: TargetingConditions::for_combat(),
        }
    }

    fn find_target(&mut self, mob: &dyn PathfinderMob) {
        let Some(world) = mob.level() else {
            self.target = None;
            return;
        };
        let Some(neutral) = mob.as_neutral_mob() else {
            self.target = None;
            return;
        };

        let follow = follow_distance(mob);
        let position = mob.position();
        let origin = glam::DVec3::new(position.x, mob.get_eye_y(), position.z);
        let conditions = self.conditions.clone().range(follow);

        self.target = world
            .nearest_player(origin, follow, |player| {
                conditions.test(world.as_ref(), Some(mob), player) && neutral.is_angry_at(player)
            })
            .map(|player| -> SharedEntity { player });
    }
}

impl Goal for AngryAtPlayerTargetGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::TARGET
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if self.random_interval > 0 && rand::random_range(0..self.random_interval) != 0 {
            return false;
        }

        self.find_target(mob);
        self.target.is_some()
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.target_goal.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let _ = mob.set_target(self.target.as_ref());
        self.target_goal.set_target_mob(self.target.clone());
        self.target_goal.start();
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.target_goal.stop(mob);
        self.target = None;
    }
}
