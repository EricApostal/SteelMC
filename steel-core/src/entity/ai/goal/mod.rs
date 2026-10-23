//! Vanilla-shaped goal selector and movement goals.

mod avoid_entity;
mod beg_goal;
mod break_door;
mod breath_air;
mod breed_goal;
mod capabilities;
mod cat_lie_on_block;
mod cat_sit_on_block;
mod climb_on_top_of_powder_snow;
mod door_interact;
mod eat_block_goal;
mod flee_sun;
mod float_goal;
mod follow_flock_leader;
mod follow_mob;
mod follow_owner;
mod follow_parent;
mod follow_player_ridden_entity;
mod golem_random_stroll_in_village;
mod hold_ground_attack;
mod hurt_by_target;
mod illager_spells;
mod interact;
mod jump_goal;
mod land_on_owners_shoulder;
mod leap_at_target;
mod llama_follow_caravan;
mod look_at_player;
mod look_at_trading_player;
mod melee_attack;
mod move_back_to_village;
mod move_through_village;
mod move_to_block;
mod move_towards_restriction;
mod move_towards_target;
mod nearest_attackable_target;
mod non_tame_random_target;
mod ocelot_attack;
mod offer_flower;
mod open_door;
mod owner_targets;
mod panic_goal;
mod pathfind_to_raid;
mod raid_raider_goals;
mod random_look_around;
mod random_pos;
mod random_stand;
mod random_stroll;
mod random_swimming;
mod ranged_attack;
mod ranged_bow_attack;
mod ranged_crossbow_attack;
mod remove_block_goal;
mod restrict_sun;
mod run_around_like_crazy;
mod selector;
mod sit_when_ordered_to;
mod spear_use_goal;
mod stroll_through_village;
mod swell_goal;
mod target;
mod target_goal;
mod tempt_goal;
mod try_find_water;
mod use_item_goal;
mod water_avoiding_random_flying;
mod water_avoiding_random_stroll;
mod witch_targets;
mod zombie_attack_goal;

pub(crate) use avoid_entity::AvoidEntityGoal;
pub(crate) use beg_goal::BegGoal;
pub(crate) use break_door::{BreakDoorGoal, VindicatorBreakDoorGoal};
pub(crate) use breath_air::BreathAirGoal;
pub(crate) use breed_goal::BreedGoal;
pub(crate) use capabilities::{
    AbstractHorse, CaravanLlama, CreeperLike, Feline, RangedAttackMob, SchoolingFish,
    ShoulderRiding, Tameable,
};
pub(crate) use cat_lie_on_block::CatLieOnBlockGoal;
pub(crate) use cat_sit_on_block::CatSitOnBlockGoal;
pub(crate) use climb_on_top_of_powder_snow::ClimbOnTopOfPowderSnowGoal;
pub(crate) use eat_block_goal::EatBlockGoal;
pub(crate) use flee_sun::FleeSunGoal;
pub(crate) use float_goal::FloatGoal;
pub(crate) use follow_flock_leader::FollowFlockLeaderGoal;
pub(crate) use follow_mob::FollowMobGoal;
pub(crate) use follow_owner::FollowOwnerGoal;
pub(crate) use follow_parent::FollowParentGoal;
pub(crate) use follow_player_ridden_entity::FollowPlayerRiddenEntityGoal;
pub(crate) use golem_random_stroll_in_village::GolemRandomStrollInVillageGoal;
pub(crate) use hold_ground_attack::HoldGroundAttackGoal;
pub(crate) use hurt_by_target::HurtByTargetGoal;
pub(crate) use illager_spells::{
    IllusionerBlindnessSpell, IllusionerMirrorSpell, Spellcaster, SpellcasterCastingSpellGoal,
    SpellcasterSpell, SpellcasterUseSpellGoal,
};
pub(crate) use interact::InteractGoal;
pub(crate) use jump_goal::{DolphinJumpGoal, JumpGoal};
pub(crate) use land_on_owners_shoulder::LandOnOwnersShoulderGoal;
pub(crate) use leap_at_target::LeapAtTargetGoal;
pub(crate) use llama_follow_caravan::LlamaFollowCaravanGoal;
pub(crate) use look_at_player::LookAtPlayerGoal;
pub(crate) use look_at_trading_player::LookAtTradingPlayerGoal;
pub(crate) use melee_attack::MeleeAttackGoal;
pub(crate) use move_back_to_village::MoveBackToVillageGoal;
pub(crate) use move_through_village::MoveThroughVillageGoal;
pub(crate) use move_to_block::MoveToBlockGoal;
pub(crate) use move_towards_restriction::MoveTowardsRestrictionGoal;
pub(crate) use move_towards_target::MoveTowardsTargetGoal;
pub(crate) use nearest_attackable_target::NearestAttackableTargetGoal;
pub(crate) use non_tame_random_target::NonTameRandomTargetGoal;
pub(crate) use ocelot_attack::OcelotAttackGoal;
pub(crate) use offer_flower::OfferFlowerGoal;
pub(crate) use open_door::OpenDoorGoal;
pub(crate) use owner_targets::{OwnerHurtByTargetGoal, OwnerHurtTargetGoal};
pub(crate) use panic_goal::PanicGoal;
pub(crate) use pathfind_to_raid::PathfindToRaidGoal;
pub(crate) use raid_raider_goals::{
    RaiderCelebrationGoal, RaiderMoveThroughVillageGoal, RaiderObtainLeaderBannerGoal,
    RaiderOpenDoorGoal,
};
pub(crate) use random_look_around::RandomLookAroundGoal;
pub(crate) use random_pos::default_random_pos_away;
pub(crate) use random_stand::RandomStandGoal;
pub(crate) use random_stroll::RandomStrollGoal;
pub(crate) use random_swimming::RandomSwimmingGoal;
pub(crate) use ranged_attack::RangedAttackGoal;
pub(crate) use ranged_bow_attack::RangedBowAttackGoal;
pub(crate) use ranged_crossbow_attack::RangedCrossbowAttackGoal;
pub(crate) use remove_block_goal::RemoveBlockGoal;
pub(crate) use restrict_sun::RestrictSunGoal;
pub(crate) use run_around_like_crazy::RunAroundLikeCrazyGoal;
pub(crate) use selector::{Goal, GoalControl, GoalControls, GoalSelector, is_goal};
pub(crate) use sit_when_ordered_to::SitWhenOrderedToGoal;
pub(crate) use spear_use_goal::SpearUseGoal;
pub(crate) use stroll_through_village::StrollThroughVillageGoal;
pub(crate) use swell_goal::SwellGoal;
pub(crate) use target::{
    AngryAtPlayerTargetGoal, DefendVillageTargetGoal, ResetUniversalAngerTargetGoal,
};
pub(crate) use tempt_goal::TemptGoal;
pub(crate) use try_find_water::TryFindWaterGoal;
pub(crate) use use_item_goal::UseItemGoal;
pub(crate) use water_avoiding_random_flying::WaterAvoidingRandomFlyingGoal;
pub(crate) use water_avoiding_random_stroll::WaterAvoidingRandomStrollGoal;
pub(crate) use witch_targets::{
    NearestAttackableWitchTargetGoal, NearestHealableRaiderTargetGoal, WitchTargetState,
};
pub(crate) use zombie_attack_goal::ZombieAttackGoal;

pub(super) const fn reduced_tick_delay(ticks: i32) -> i32 {
    (ticks + 1) / 2
}
