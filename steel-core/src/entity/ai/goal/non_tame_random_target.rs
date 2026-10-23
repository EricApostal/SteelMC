//! Vanilla `NonTameRandomTargetGoal` (`goal/target/NonTameRandomTargetGoal.java`): like the
//! base nearest-attackable target goal, except a tamed mob never uses it.

use crate::entity::PathfinderMob;

use super::nearest_attackable_target::NearestAttackableTargetGoal;
use super::selector::{Goal, GoalControls};

/// Vanilla `NonTameRandomTargetGoal`.
pub struct NonTameRandomTargetGoal {
    inner: NearestAttackableTargetGoal,
}

impl NonTameRandomTargetGoal {
    /// Vanilla `new NonTameRandomTargetGoal(mob, targetType, mustSee, subselector)`: the base
    /// class is built with interval 10, `mustReach = false` and the vanilla `DEFAULT`
    /// conditions (no selector).
    #[must_use]
    pub fn new(
        must_see: bool,
        selector: impl Fn(&dyn crate::entity::LivingEntity, &crate::world::World) -> bool
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            inner: NearestAttackableTargetGoal::new_with_interval(10, must_see, false, selector),
        }
    }

    /// Vanilla `NonTameRandomTargetGoal.setTarget`.
    pub fn set_target(&mut self, target: Option<crate::entity::SharedEntity>) {
        self.inner.set_target(target);
    }
}

impl Goal for NonTameRandomTargetGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::TARGET
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if mob.as_tameable().is_some_and(|tameable| tameable.is_tame()) {
            return false;
        }

        self.inner.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        // Vanilla `NonTameRandomTargetGoal.canContinueToUse` re-tests the stored
        // `targetConditions` against its own `target` field instead of falling back to
        // `TargetGoal.canContinueToUse`; `start` copies that field onto the mob, so Steel
        // re-tests the mob's live target.
        let Some(target) = mob.target() else {
            return false;
        };
        let Some(target_living) = target.as_living_entity() else {
            return false;
        };

        self.inner.target_matches_conditions(mob, target_living)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.inner.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.inner.stop(mob);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_tame_random_target_goal_claims_only_the_target_control() {
        let goal = NonTameRandomTargetGoal::new(false, |_, _| true);

        assert_eq!(goal.controls(), GoalControls::TARGET);
    }
}
