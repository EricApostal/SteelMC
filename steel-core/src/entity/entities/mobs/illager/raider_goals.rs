//! Goal wiring shared by every raider (vanilla `Raider.registerGoals` plus the raid goals).

use crate::entity::MobBase;
use crate::entity::ai::goal::{
    HoldGroundAttackGoal, PathfindToRaidGoal, RaiderCelebrationGoal, RaiderMoveThroughVillageGoal,
    RaiderObtainLeaderBannerGoal, RangedCrossbowAttackGoal,
};

/// Vanilla `Raider.registerGoals`: banner hunt, raid pathfinding, village walk, celebration.
pub fn register_raider_goals(mob_base: &MobBase) {
    let mut goals = mob_base.goal_selector().lock();
    goals.add_goal(1, RaiderObtainLeaderBannerGoal::new());
    goals.add_goal(3, PathfindToRaidGoal::new());
    goals.add_goal(4, RaiderMoveThroughVillageGoal::new(1.05, 1));
    goals.add_goal(5, RaiderCelebrationGoal::new());
}

/// Vanilla `Raider.HoldGroundAttackGoal(this, radius)`.
pub fn hold_ground_attack(radius: f32) -> HoldGroundAttackGoal {
    HoldGroundAttackGoal::new(radius)
}

/// Vanilla `RangedCrossbowAttackGoal<>(this, speed, range)`.
pub fn crossbow_attack(speed: f64, range: f32) -> RangedCrossbowAttackGoal {
    RangedCrossbowAttackGoal::new(speed, range)
}
