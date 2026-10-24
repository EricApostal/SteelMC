//! Vanilla owner-retaliation target goals (`goal/target/OwnerHurtByTargetGoal.java`,
//! `goal/target/OwnerHurtTargetGoal.java`): a tamed mob defends its owner when its owner is
//! hurt or when its owner hurts another mob.
//!
//! Vanilla stores the `TamableAnimal` directly; Steel reaches owner and sitting state through
//! [`Tameable`] so the goals only encode the vanilla timestamp bookkeeping. The timestamp
//! comparison uses `OwnerHurt*` fields vanilla reads: the goal starts when the owner's
//! `lastHurtByMobTimestamp`/`lastHurtMobTimestamp` differs from the last one the goal acted
//! on.
//!
//! Difference: vanilla `OwnerHurtByTargetGoal` tests `owner.getLastDamageSource(100)`; Steel's
//! damage history only exposes the no-argument window (40 ticks), so a retaliation between 40
//! and 100 ticks old is not seen.

use steel_registry::vanilla_damage_type_tags::DamageTypeTag;

use crate::entity::ai::targeting::TargetingConditions;
use crate::entity::{PathfinderMob, SharedEntity};

use super::capabilities::Tameable;
use super::selector::{Goal, GoalControls};
use super::target_goal::TargetGoalBase;

/// Vanilla `OwnerHurtByTargetGoal`.
pub struct OwnerHurtByTargetGoal {
    target_goal: TargetGoalBase,
    target_conditions: TargetingConditions,
    owner_last_hurt_by: Option<SharedEntity>,
    timestamp: i32,
}

/// Vanilla `OwnerHurtTargetGoal`.
pub struct OwnerHurtTargetGoal {
    target_goal: TargetGoalBase,
    target_conditions: TargetingConditions,
    owner_last_hurt: Option<SharedEntity>,
    timestamp: i32,
}

impl OwnerHurtByTargetGoal {
    /// Vanilla `new OwnerHurtByTargetGoal(tameAnimal)`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            target_goal: TargetGoalBase::new(false, false),
            target_conditions: TargetingConditions::for_combat(),
            owner_last_hurt_by: None,
            timestamp: 0,
        }
    }
}

impl Default for OwnerHurtByTargetGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for OwnerHurtByTargetGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::TARGET
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(tameable) = mob.as_tameable() else {
            return false;
        };
        if !tameable.is_tame() || tameable.is_ordered_to_sit() {
            return false;
        }

        let Some(owner) = tameable.owner() else {
            return false;
        };
        let Some(owner_living) = owner.as_living_entity() else {
            return false;
        };

        // Vanilla `owner.getLastDamageSource(100) != null && !source.is(NO_WOLF_RETALIATION)`.
        let Some(source) = owner_living.last_damage_source() else {
            return false;
        };
        if source.is(&DamageTypeTag::NO_WOLF_RETALIATION) {
            return false;
        }

        self.owner_last_hurt_by = owner_living.last_hurt_by_mob();
        let timestamp = owner_living.last_hurt_by_mob_timestamp();
        if timestamp == self.timestamp {
            return false;
        }

        let Some(owner_last_hurt_by) = self.owner_last_hurt_by.clone() else {
            return false;
        };
        self.target_goal.can_attack(
            mob,
            owner_last_hurt_by.as_living_entity(),
            &self.target_conditions,
        ) && tameable.wants_to_attack(&owner_last_hurt_by, &owner)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.target_goal.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let _ = mob.set_target(self.owner_last_hurt_by.as_ref());
        if let Some(owner) = mob.as_tameable().and_then(Tameable::owner)
            && let Some(owner_living) = owner.as_living_entity()
        {
            self.timestamp = owner_living.last_hurt_by_mob_timestamp();
        }
        self.target_goal.start();
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.target_goal.stop(mob);
        self.owner_last_hurt_by = None;
    }
}

impl OwnerHurtTargetGoal {
    /// Vanilla `new OwnerHurtTargetGoal(tameAnimal)`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            target_goal: TargetGoalBase::new(false, false),
            target_conditions: TargetingConditions::for_combat(),
            owner_last_hurt: None,
            timestamp: 0,
        }
    }
}

impl Default for OwnerHurtTargetGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for OwnerHurtTargetGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::TARGET
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(tameable) = mob.as_tameable() else {
            return false;
        };
        if !tameable.is_tame() || tameable.is_ordered_to_sit() {
            return false;
        }

        let Some(owner) = tameable.owner() else {
            return false;
        };
        let Some(owner_living) = owner.as_living_entity() else {
            return false;
        };

        self.owner_last_hurt = owner_living.last_hurt_mob();
        let timestamp = owner_living.last_hurt_mob_timestamp();
        if timestamp == self.timestamp {
            return false;
        }

        let Some(owner_last_hurt) = self.owner_last_hurt.clone() else {
            return false;
        };
        self.target_goal.can_attack(
            mob,
            owner_last_hurt.as_living_entity(),
            &self.target_conditions,
        ) && tameable.wants_to_attack(&owner_last_hurt, &owner)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.target_goal.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let _ = mob.set_target(self.owner_last_hurt.as_ref());
        if let Some(owner) = mob.as_tameable().and_then(Tameable::owner)
            && let Some(owner_living) = owner.as_living_entity()
        {
            self.timestamp = owner_living.last_hurt_mob_timestamp();
        }
        self.target_goal.start();
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.target_goal.stop(mob);
        self.owner_last_hurt = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_target_goals_claim_only_the_target_control() {
        let hurt_by = OwnerHurtByTargetGoal::new();
        let hurt_target = OwnerHurtTargetGoal::new();

        assert_eq!(hurt_by.controls(), GoalControls::TARGET);
        assert_eq!(hurt_target.controls(), GoalControls::TARGET);
    }
}
