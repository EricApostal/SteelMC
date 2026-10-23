use super::selector::{Goal, GoalControls};
use crate::entity::PathfinderMob;
use crate::entity::ai::navigation::NavigationKind;
use crate::inventory::equipment::EquipmentSlot;

pub struct RestrictSunGoal;

impl RestrictSunGoal {
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self
    }
}

/// Vanilla `GoalUtils.hasGroundPathNavigation`: `navigation.canNavigateGround()`, true for
/// `GroundPathNavigation` (and `WallClimberNavigation`) and `AmphibiousPathNavigation`.
fn has_ground_path_navigation(mob: &dyn PathfinderMob) -> bool {
    let navigation = mob.mob_base().navigation().lock();
    !navigation.is_flying()
        && matches!(
            navigation.kind(),
            NavigationKind::Ground | NavigationKind::Amphibious | NavigationKind::WallClimber
        )
}

impl Goal for RestrictSunGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };

        world.is_bright_outside()
            && !mob.has_item_in_slot(EquipmentSlot::Head)
            && has_ground_path_navigation(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        if has_ground_path_navigation(mob) {
            mob.mob_base().navigation().lock().set_avoid_sun(true);
        }
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if has_ground_path_navigation(mob) {
            mob.mob_base().navigation().lock().set_avoid_sun(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::*;
    use crate::entity::Mob as _;
    use crate::entity::entities::PigEntity;

    fn pig() -> PigEntity {
        PigEntity::new(&vanilla_entities::PIG, 1, DVec3::ZERO, Weak::new())
    }

    #[test]
    fn restrict_sun_goal_claims_no_controls_like_vanilla() {
        let goal = RestrictSunGoal::new();

        assert_eq!(goal.controls(), GoalControls::EMPTY);
    }

    #[test]
    fn restrict_sun_goal_requires_world() {
        init_vanilla_registry();
        let mut goal = RestrictSunGoal::new();

        assert!(!goal.can_use(&pig()));
    }

    #[test]
    fn restrict_sun_goal_toggles_navigation_avoid_sun() {
        init_vanilla_registry();
        let mut goal = RestrictSunGoal::new();
        let pig = pig();

        assert!(!pig.mob_base().navigation().lock().avoid_sun());

        goal.start(&pig);
        assert!(pig.mob_base().navigation().lock().avoid_sun());

        goal.stop(&pig);
        assert!(!pig.mob_base().navigation().lock().avoid_sun());
    }
}
