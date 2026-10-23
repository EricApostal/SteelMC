//! Goal wiring shared by every raider (vanilla `Raider.registerGoals` plus the raid goals).

use crate::entity::MobBase;
use crate::entity::ai::goal::{
    HoldGroundAttackGoal, PathfindToRaidGoal, RaiderCelebrationGoal, RaiderMoveThroughVillageGoal,
    RaiderObtainLeaderBannerGoal, RangedCrossbowAttackGoal,
};
use crate::entity::raid::RaidSnapshot;

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

/// Vanilla `Pillager.applyRaidBuffs`/`Vindicator.applyRaidBuffs` wave gate.
///
/// Returns the post-wave number (`3` or `5`) that vanilla passes to
/// `VanillaEnchantmentProviders.RAID_*`, or `None` when the wave is below both gates. Steel
/// has no enchantment-provider registry, so the caller only uses this for the gate; the
/// enchantment application itself is a reported dependency.
#[must_use]
pub fn raid_enchant_provider_wave(raid: &RaidSnapshot, wave: i32) -> Option<i32> {
    let normal_groups =
        crate::entity::raid::Raid::num_groups(steel_utils::types::Difficulty::Normal);
    let easy_groups = crate::entity::raid::Raid::num_groups(steel_utils::types::Difficulty::Easy);

    if wave > normal_groups {
        Some(5)
    } else if wave > easy_groups {
        Some(3)
    } else {
        None
    }
    .filter(|_| raid.num_groups >= easy_groups)
}
