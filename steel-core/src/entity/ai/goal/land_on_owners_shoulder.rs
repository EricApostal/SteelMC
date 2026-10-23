//! Vanilla `LandOnOwnersShoulderGoal` (`goal/LandOnOwnersShoulderGoal.java`): a parrot hops
//! onto its owner's shoulder once the owner is in a state that allows it.
//!
//! Vanilla stores the `ShoulderRidingEntity`; Steel reads that state through
//! [`ShoulderRiding`](crate::entity::ai::goal::ShoulderRiding).

use super::selector::{Goal, GoalControls};
use crate::entity::{Entity, PathfinderMob, SharedEntity};

/// Vanilla `LandOnOwnersShoulderGoal`.
pub struct LandOnOwnersShoulderGoal {
    is_sitting_on_shoulder: bool,
}

impl LandOnOwnersShoulderGoal {
    /// Vanilla `new LandOnOwnersShoulderGoal(entity)`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            is_sitting_on_shoulder: false,
        }
    }
}

impl Default for LandOnOwnersShoulderGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for LandOnOwnersShoulderGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(shoulder_rider) = mob.as_shoulder_riding() else {
            return false;
        };
        let Some(owner) = shoulder_rider.owner() else {
            return false;
        };
        let Some(player) = owner.as_player() else {
            return false;
        };

        // Vanilla `!owner.isSpectator() && !owner.getAbilities().flying && !owner.isInWater()
        // && !owner.isInPowderSnow`.
        let owner_that_can_be_sat_on = !player.is_spectator()
            && !player.is_flying()
            && !player.is_in_water()
            && !player.is_in_powder_snow();

        !shoulder_rider.is_ordered_to_sit()
            && owner_that_can_be_sat_on
            && shoulder_rider.can_sit_on_shoulder()
    }

    fn is_interruptable(&self) -> bool {
        !self.is_sitting_on_shoulder
    }

    fn start(&mut self, _mob: &dyn PathfinderMob) {
        self.is_sitting_on_shoulder = false;
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        if self.is_sitting_on_shoulder {
            return;
        }

        let Some(shoulder_rider) = mob.as_shoulder_riding() else {
            return;
        };
        if shoulder_rider.is_in_sitting_pose() || mob.is_leashed() {
            return;
        }

        let Some(owner) = shoulder_rider.owner() else {
            return;
        };
        if !mob.bounding_box().intersects(owner.bounding_box()) {
            return;
        }

        self.is_sitting_on_shoulder = shoulder_rider.set_entity_on_shoulder(&owner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn land_on_owners_shoulder_goal_is_interruptable_until_perched() {
        let goal = LandOnOwnersShoulderGoal::new();

        assert_eq!(goal.controls(), GoalControls::EMPTY);
        assert!(goal.is_interruptable());
    }
}
