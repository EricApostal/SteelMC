//! Vanilla-shaped goal selector and movement goals.

mod avoid_entity;
mod breath_air;
mod breed_goal;
mod climb_on_top_of_powder_snow;
mod door_interact;
mod eat_block_goal;
mod flee_sun;
mod float_goal;
mod follow_mob;
mod follow_parent;
mod golem_random_stroll_in_village;
mod hold_ground_attack;
mod hurt_by_target;
mod interact;
mod leap_at_target;
mod look_at_player;
mod look_at_trading_player;
mod melee_attack;
mod move_back_to_village;
mod move_through_village;
mod move_to_block;
mod move_towards_restriction;
mod move_towards_target;
mod nearest_attackable_target;
mod offer_flower;
mod open_door;
mod panic_goal;
mod pathfind_to_raid;
mod raid_raider_goals;
mod random_look_around;
mod random_pos;
mod random_stroll;
mod random_swimming;
mod ranged_crossbow_attack;
mod restrict_sun;
mod selector;
mod stroll_through_village;
mod target;
mod target_goal;
mod tempt_goal;
mod try_find_water;
mod use_item_goal;
mod water_avoiding_random_stroll;

pub(crate) use avoid_entity::AvoidEntityGoal;
pub(crate) use breed_goal::BreedGoal;
pub(crate) use climb_on_top_of_powder_snow::ClimbOnTopOfPowderSnowGoal;
pub(crate) use eat_block_goal::EatBlockGoal;
pub(crate) use float_goal::FloatGoal;
pub(crate) use follow_parent::FollowParentGoal;
pub(crate) use golem_random_stroll_in_village::GolemRandomStrollInVillageGoal;
pub(crate) use hold_ground_attack::HoldGroundAttackGoal;
pub(crate) use hurt_by_target::HurtByTargetGoal;
pub(crate) use interact::InteractGoal;
pub(crate) use look_at_player::LookAtPlayerGoal;
pub(crate) use melee_attack::MeleeAttackGoal;
pub(crate) use move_back_to_village::MoveBackToVillageGoal;
pub(crate) use move_towards_restriction::MoveTowardsRestrictionGoal;
pub(crate) use move_towards_target::MoveTowardsTargetGoal;
pub(crate) use nearest_attackable_target::NearestAttackableTargetGoal;
pub(crate) use offer_flower::OfferFlowerGoal;
pub(crate) use panic_goal::PanicGoal;
pub(crate) use pathfind_to_raid::PathfindToRaidGoal;
pub(crate) use raid_raider_goals::{
    RaiderCelebrationGoal, RaiderMoveThroughVillageGoal, RaiderObtainLeaderBannerGoal,
    RaiderOpenDoorGoal,
};
pub(crate) use random_look_around::RandomLookAroundGoal;
pub(crate) use random_pos::default_random_pos_away;
pub(crate) use random_stroll::RandomStrollGoal;
pub(crate) use ranged_crossbow_attack::RangedCrossbowAttackGoal;
pub(crate) use selector::{Goal, GoalControl, GoalControls, GoalSelector};
pub(crate) use target::{
    AngryAtPlayerTargetGoal, DefendVillageTargetGoal, ResetUniversalAngerTargetGoal,
};
pub(crate) use tempt_goal::TemptGoal;
pub(crate) use use_item_goal::UseItemGoal;
pub(crate) use water_avoiding_random_stroll::WaterAvoidingRandomStrollGoal;

pub(super) const fn reduced_tick_delay(ticks: i32) -> i32 {
    (ticks + 1) / 2
}
