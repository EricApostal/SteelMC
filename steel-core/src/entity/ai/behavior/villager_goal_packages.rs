//! Vanilla `VillagerGoalPackages`: the per-activity behavior packages a villager
//! brain registers.
//!
//! Every package mirrors the vanilla list (order, priorities and `RunOne`
//! weights). The raid-gated entries are the one place where Steel cannot yet
//! match vanilla: Steel has no raid manager (`World::get_raid_at` does not
//! exist), so [`raid_exists_and_active`] / [`raid_exists_and_not_victory`] both
//! report no raid and the gated behaviors never start.

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::entity_type::MobCategory;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_entities;
use steel_registry::vanilla_poi_types;
use steel_registry::villager_profession::VillagerProfessionRef;
use steel_utils::BlockPos;

use crate::entity::ai::brain::behavior::builder::BehaviorBuilder;
use crate::entity::ai::brain::behavior::gate::{GateBehavior, OrderPolicy, RunOne, RunningPolicy};
use crate::entity::ai::brain::behavior::{BehaviorContext, BehaviorControl, Trigger};
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::world::World;

use super::acquire_poi::AcquirePoi;
use super::assign_profession_from_job_site::AssignProfessionFromJobSite;
use super::celebrate_villagers_survived_raid::CelebrateVillagersSurvivedRaid;
use super::do_nothing::DoNothing;
use super::give_gift_to_hero::GiveGiftToHero;
use super::go_to_closest_village::GoToClosestVillage;
use super::go_to_potential_job_site::GoToPotentialJobSite;
use super::go_to_wanted_item::GoToWantedItem;
use super::harvest_farmland::HarvestFarmland;
use super::inside_brownian_walk::InsideBrownianWalk;
use super::interact_with::InteractWith;
use super::interact_with_door::InteractWithDoor;
use super::jump_on_bed::JumpOnBed;
use super::locate_hiding_place::LocateHidingPlace;
use super::look_and_follow_trading_player::LookAndFollowTradingPlayerSink;
use super::look_at_target_sink::LookAtTargetSink;
use super::move_to_sky_seeing_spot::MoveToSkySeeingSpot;
use super::move_to_target_sink::MoveToTargetSink;
use super::play_tag_with_other_kids::PlayTagWithOtherKids;
use super::poi::{self, PoiPredicate};
use super::poi_competitor_scan::PoiCompetitorScan;
use super::react_to_bell;
use super::reset_profession::ResetProfession;
use super::reset_raid_status;
use super::ring_bell::RingBell;
use super::set_closest_home_as_walk_target::SetClosestHomeAsWalkTarget;
use super::set_entity_look_target::SetEntityLookTarget;
use super::set_hidden_state::SetHiddenState;
use super::set_look_and_interact::SetLookAndInteract;
use super::set_raid_status;
use super::set_walk_target_away_from::SetWalkTargetAwayFrom;
use super::set_walk_target_from_block_memory::SetWalkTargetFromBlockMemory;
use super::set_walk_target_from_look_target::SetWalkTargetFromLookTarget;
use super::show_trades_to_player::ShowTradesToPlayer;
use super::sleep_in_bed::SleepInBed;
use super::socialize_at_bell::SocializeAtBell;
use super::stroll_around_poi::StrollAroundPoi;
use super::stroll_to_poi::StrollToPoi;
use super::stroll_to_poi_list::StrollToPoiList;
use super::swim::Swim;
use super::trade_with_villager::TradeWithVillager;
use super::update_activity_from_schedule;
use super::use_bonemeal::UseBonemeal;
use super::validate_nearby_poi::ValidateNearbyPoi;
use super::village_bound_random_stroll::VillageBoundRandomStroll;
use super::villager_calm_down::VillagerCalmDown;
use super::villager_make_love::VillagerMakeLove;
use super::villager_panic_trigger::VillagerPanicTrigger;
use super::wake_up;
use super::work_at_composter::WorkAtComposter;
use super::work_at_poi::WorkAtPoi;
use super::yield_job_site::YieldJobSite;

/// A weighted behavior list in the shape the brain's activity registry expects.
pub type Package = Vec<(i32, Box<dyn BehaviorControl>)>;

/// Vanilla `VillagerGoalPackages.STROLL_SPEED_MODIFIER`.
const STROLL_SPEED_MODIFIER: f32 = 0.4;
/// Vanilla `VillagerGoalPackages.INTERACT_DIST_SQR`.
pub const INTERACT_DIST_SQR: i32 = 5;
/// Vanilla `VillagerGoalPackages.INTERACT_WALKUP_DIST`.
pub const INTERACT_WALKUP_DIST: i32 = 2;
/// Vanilla `VillagerGoalPackages.INTERACT_SPEED_MODIFIER`.
pub const INTERACT_SPEED_MODIFIER: f32 = 0.5;

/// Vanilla `VillagerGoalPackages.getCorePackage`.
#[must_use]
pub fn get_core_package(profession: VillagerProfessionRef, speed_modifier: f32) -> Package {
    vec![
        (0, BehaviorBuilder::boxed(Swim::new(0.8))),
        (0, BehaviorBuilder::boxed(InteractWithDoor::create())),
        (0, BehaviorBuilder::boxed(LookAtTargetSink::new(45, 90))),
        (0, BehaviorBuilder::boxed(VillagerPanicTrigger::new())),
        (0, BehaviorBuilder::boxed(wake_up::create())),
        (0, BehaviorBuilder::boxed(react_to_bell::create())),
        (0, BehaviorBuilder::boxed(set_raid_status::create())),
        (
            0,
            BehaviorBuilder::boxed(ValidateNearbyPoi::create(
                poi::profession_held_job_site(profession),
                &mem::JOB_SITE,
            )),
        ),
        (
            0,
            BehaviorBuilder::boxed(ValidateNearbyPoi::create(
                poi::profession_acquirable_job_site(profession),
                &mem::POTENTIAL_JOB_SITE,
            )),
        ),
        (1, BehaviorBuilder::boxed(MoveToTargetSink::new())),
        (2, BehaviorBuilder::boxed(PoiCompetitorScan::create())),
        (
            3,
            BehaviorBuilder::boxed(LookAndFollowTradingPlayerSink::new(speed_modifier)),
        ),
        (
            5,
            BehaviorBuilder::boxed(GoToWantedItem::create(speed_modifier, false, 4)),
        ),
        (
            6,
            BehaviorBuilder::boxed(AcquirePoi::create(
                poi::profession_acquirable_job_site(profession),
                &mem::JOB_SITE,
                Some(&mem::POTENTIAL_JOB_SITE),
                true,
                None,
                |_, _| true,
            )),
        ),
        (
            7,
            BehaviorBuilder::boxed(GoToPotentialJobSite::new(speed_modifier)),
        ),
        (
            8,
            BehaviorBuilder::boxed(YieldJobSite::create(speed_modifier)),
        ),
        (
            10,
            BehaviorBuilder::boxed(AcquirePoi::create(
                PoiPredicate::Type(&vanilla_poi_types::HOME),
                &mem::HOME,
                None,
                false,
                Some(14),
                validate_bed_poi,
            )),
        ),
        (
            10,
            BehaviorBuilder::boxed(AcquirePoi::create(
                PoiPredicate::Type(&vanilla_poi_types::MEETING),
                &mem::MEETING_POINT,
                None,
                true,
                Some(14),
                |_, _| true,
            )),
        ),
        (
            10,
            BehaviorBuilder::boxed(AssignProfessionFromJobSite::create()),
        ),
        (10, BehaviorBuilder::boxed(ResetProfession::create())),
    ]
}

/// Vanilla `VillagerGoalPackages.validateBedPoi`.
fn validate_bed_poi(level: &World, pos: BlockPos) -> bool {
    let state = level.get_block_state(pos);
    state
        .get_block()
        .has_tag(&BlockTag::VILLAGERS_CAN_SLEEP_ON_BED)
        && !state.get_value(&BlockStateProperties::OCCUPIED)
}

/// Vanilla `VillagerGoalPackages.getWorkPackage`.
#[must_use]
pub fn get_work_package(profession: VillagerProfessionRef, speed_modifier: f32) -> Package {
    let is_farmer = profession.key.path == "farmer";

    vec![
        get_minimal_look_behavior(),
        (
            5,
            // Vanilla weights: work 7, stroll-around 2, stroll-to 5, stroll-to-list 5,
            // harvest 2 (farmer) / 5, bonemeal 4 (farmer) / 7.
            BehaviorBuilder::boxed(RunOne::new(vec![
                (work_at_poi(profession), 7),
                (
                    BehaviorBuilder::boxed(StrollAroundPoi::create(
                        &mem::JOB_SITE,
                        STROLL_SPEED_MODIFIER,
                        4,
                    )),
                    2,
                ),
                (
                    BehaviorBuilder::boxed(StrollToPoi::create(&mem::JOB_SITE, 0.4, 1, 10)),
                    5,
                ),
                (
                    BehaviorBuilder::boxed(StrollToPoiList::create(
                        &mem::SECONDARY_JOB_SITE,
                        speed_modifier,
                        1,
                        6,
                        &mem::JOB_SITE,
                    )),
                    5,
                ),
                (
                    BehaviorBuilder::boxed(HarvestFarmland::new()),
                    if is_farmer { 2 } else { 5 },
                ),
                (
                    BehaviorBuilder::boxed(UseBonemeal::new()),
                    if is_farmer { 4 } else { 7 },
                ),
            ])),
        ),
        (
            10,
            BehaviorBuilder::boxed(ShowTradesToPlayer::new(400, 1600)),
        ),
        (
            10,
            BehaviorBuilder::boxed(SetLookAndInteract::create(&vanilla_entities::PLAYER, 4)),
        ),
        (
            2,
            BehaviorBuilder::boxed(SetWalkTargetFromBlockMemory::create(
                &mem::JOB_SITE,
                speed_modifier,
                9,
                100,
                1200,
            )),
        ),
        (3, BehaviorBuilder::boxed(GiveGiftToHero::new(100))),
        (
            99,
            BehaviorBuilder::boxed(update_activity_from_schedule::create()),
        ),
    ]
}

/// Vanilla `VillagerGoalPackages.getWorkPackage`'s farmer/non-farmer work behavior.
fn work_at_poi(profession: VillagerProfessionRef) -> Box<dyn BehaviorControl> {
    if profession.key.path == "farmer" {
        BehaviorBuilder::boxed(WorkAtComposter::new())
    } else {
        BehaviorBuilder::boxed(WorkAtPoi::new())
    }
}

/// Vanilla `VillagerGoalPackages.getPlayPackage`.
#[must_use]
pub fn get_play_package(speed_modifier: f32) -> Package {
    vec![
        (
            0,
            BehaviorBuilder::boxed(MoveToTargetSink::with_range(80, 120)),
        ),
        get_full_look_behavior(),
        (5, BehaviorBuilder::boxed(PlayTagWithOtherKids::create())),
        (
            5,
            // Vanilla weights: villager 2, cat 1, stroll 1, look-target 1, jump-on-bed 2, idle 2.
            BehaviorBuilder::boxed(RunOne::with_entry_condition(
                vec![(
                    &mem::VISIBLE_VILLAGER_BABIES as MemoryModuleTypeRef,
                    MemoryStatus::ValueAbsent,
                )],
                vec![
                    (
                        BehaviorBuilder::boxed(InteractWith::of(
                            &vanilla_entities::VILLAGER,
                            8,
                            &mem::INTERACTION_TARGET,
                            speed_modifier,
                            2,
                        )),
                        2,
                    ),
                    (
                        BehaviorBuilder::boxed(InteractWith::of(
                            &vanilla_entities::CAT,
                            8,
                            &mem::INTERACTION_TARGET,
                            speed_modifier,
                            2,
                        )),
                        1,
                    ),
                    (
                        BehaviorBuilder::boxed(VillageBoundRandomStroll::create(speed_modifier)),
                        1,
                    ),
                    (
                        BehaviorBuilder::boxed(SetWalkTargetFromLookTarget::create(
                            speed_modifier,
                            2,
                        )),
                        1,
                    ),
                    (BehaviorBuilder::boxed(JumpOnBed::new(speed_modifier)), 2),
                    (BehaviorBuilder::boxed(DoNothing::new(20, 40)), 2),
                ],
            )),
        ),
        (
            99,
            BehaviorBuilder::boxed(update_activity_from_schedule::create()),
        ),
    ]
}

/// Vanilla `VillagerGoalPackages.getRestPackage`.
#[must_use]
pub fn get_rest_package(speed_modifier: f32) -> Package {
    vec![
        (
            2,
            BehaviorBuilder::boxed(SetWalkTargetFromBlockMemory::create(
                &mem::HOME,
                speed_modifier,
                1,
                150,
                1200,
            )),
        ),
        (
            3,
            BehaviorBuilder::boxed(ValidateNearbyPoi::create(
                PoiPredicate::Type(&vanilla_poi_types::HOME),
                &mem::HOME,
            )),
        ),
        (3, BehaviorBuilder::boxed(SleepInBed::new())),
        (
            5,
            // Vanilla weights: home 1, brownian 4, village 2, idle 2.
            BehaviorBuilder::boxed(RunOne::with_entry_condition(
                vec![(&mem::HOME as MemoryModuleTypeRef, MemoryStatus::ValueAbsent)],
                vec![
                    (
                        BehaviorBuilder::boxed(SetClosestHomeAsWalkTarget::create(speed_modifier)),
                        1,
                    ),
                    (
                        BehaviorBuilder::boxed(InsideBrownianWalk::create(speed_modifier)),
                        4,
                    ),
                    (
                        BehaviorBuilder::boxed(GoToClosestVillage::create(speed_modifier, 4)),
                        2,
                    ),
                    (BehaviorBuilder::boxed(DoNothing::new(20, 40)), 2),
                ],
            )),
        ),
        get_minimal_look_behavior(),
        (
            99,
            BehaviorBuilder::boxed(update_activity_from_schedule::create()),
        ),
    ]
}

/// Vanilla `VillagerGoalPackages.getMeetPackage`.
#[must_use]
pub fn get_meet_package(speed_modifier: f32) -> Package {
    vec![
        (
            2,
            BehaviorBuilder::boxed(BehaviorBuilder::create(
                BehaviorBuilder::trigger_one_shuffled(vec![
                    (
                        control(StrollAroundPoi::create(&mem::MEETING_POINT, 0.4, 40)),
                        2,
                    ),
                    (control(SocializeAtBell::create()), 2),
                ]),
            )),
        ),
        (
            10,
            BehaviorBuilder::boxed(ShowTradesToPlayer::new(400, 1600)),
        ),
        (
            10,
            BehaviorBuilder::boxed(SetLookAndInteract::create(&vanilla_entities::PLAYER, 4)),
        ),
        (
            2,
            BehaviorBuilder::boxed(SetWalkTargetFromBlockMemory::create(
                &mem::MEETING_POINT,
                speed_modifier,
                6,
                100,
                200,
            )),
        ),
        (3, BehaviorBuilder::boxed(GiveGiftToHero::new(100))),
        (
            3,
            BehaviorBuilder::boxed(ValidateNearbyPoi::create(
                PoiPredicate::Type(&vanilla_poi_types::MEETING),
                &mem::MEETING_POINT,
            )),
        ),
        (
            3,
            BehaviorBuilder::boxed(GateBehavior::new(
                Vec::new(),
                vec![&mem::INTERACTION_TARGET as MemoryModuleTypeRef],
                OrderPolicy::Ordered,
                RunningPolicy::RunOne,
                vec![(BehaviorBuilder::boxed(TradeWithVillager::new()), 1)],
            )),
        ),
        get_full_look_behavior(),
        (
            99,
            BehaviorBuilder::boxed(update_activity_from_schedule::create()),
        ),
    ]
}

/// Vanilla `VillagerGoalPackages.getIdlePackage`.
#[must_use]
pub fn get_idle_package(speed_modifier: f32) -> Package {
    vec![
        (
            2,
            // Vanilla weights: villager 2, breed 1, cat 1, stroll 1, look-target 1,
            // jump-on-bed 1, idle 1.
            BehaviorBuilder::boxed(RunOne::new(vec![
                (
                    BehaviorBuilder::boxed(InteractWith::of(
                        &vanilla_entities::VILLAGER,
                        8,
                        &mem::INTERACTION_TARGET,
                        speed_modifier,
                        2,
                    )),
                    2,
                ),
                (
                    BehaviorBuilder::boxed(InteractWith::of_breeding(
                        &vanilla_entities::VILLAGER,
                        8,
                        &mem::BREED_TARGET,
                        speed_modifier,
                        2,
                    )),
                    1,
                ),
                (
                    BehaviorBuilder::boxed(InteractWith::of(
                        &vanilla_entities::CAT,
                        8,
                        &mem::INTERACTION_TARGET,
                        speed_modifier,
                        2,
                    )),
                    1,
                ),
                (
                    BehaviorBuilder::boxed(VillageBoundRandomStroll::create(speed_modifier)),
                    1,
                ),
                (
                    BehaviorBuilder::boxed(SetWalkTargetFromLookTarget::create(speed_modifier, 2)),
                    1,
                ),
                (BehaviorBuilder::boxed(JumpOnBed::new(speed_modifier)), 1),
                (BehaviorBuilder::boxed(DoNothing::new(30, 60)), 1),
            ])),
        ),
        (3, BehaviorBuilder::boxed(GiveGiftToHero::new(100))),
        (
            3,
            BehaviorBuilder::boxed(SetLookAndInteract::create(&vanilla_entities::PLAYER, 4)),
        ),
        (
            3,
            BehaviorBuilder::boxed(ShowTradesToPlayer::new(400, 1600)),
        ),
        (
            3,
            BehaviorBuilder::boxed(GateBehavior::new(
                Vec::new(),
                vec![&mem::INTERACTION_TARGET as MemoryModuleTypeRef],
                OrderPolicy::Ordered,
                RunningPolicy::RunOne,
                vec![(BehaviorBuilder::boxed(TradeWithVillager::new()), 1)],
            )),
        ),
        (
            3,
            BehaviorBuilder::boxed(GateBehavior::new(
                Vec::new(),
                vec![&mem::BREED_TARGET as MemoryModuleTypeRef],
                OrderPolicy::Ordered,
                RunningPolicy::RunOne,
                vec![(BehaviorBuilder::boxed(VillagerMakeLove::new()), 1)],
            )),
        ),
        get_full_look_behavior(),
        (
            99,
            BehaviorBuilder::boxed(update_activity_from_schedule::create()),
        ),
    ]
}

/// Vanilla `VillagerGoalPackages.getPanicPackage`.
#[must_use]
pub fn get_panic_package(speed_modifier: f32) -> Package {
    let runaway_speed = speed_modifier * 1.5;
    vec![
        (0, BehaviorBuilder::boxed(VillagerCalmDown::create())),
        (
            1,
            BehaviorBuilder::boxed(SetWalkTargetAwayFrom::entity(
                &mem::NEAREST_HOSTILE,
                runaway_speed,
                6,
                false,
            )),
        ),
        (
            1,
            BehaviorBuilder::boxed(SetWalkTargetAwayFrom::entity(
                &mem::HURT_BY_ENTITY,
                runaway_speed,
                6,
                false,
            )),
        ),
        (
            3,
            BehaviorBuilder::boxed(VillageBoundRandomStroll::create_with_params(
                runaway_speed,
                2,
                2,
            )),
        ),
        get_minimal_look_behavior(),
    ]
}

/// Vanilla `VillagerGoalPackages.getPreRaidPackage`.
#[must_use]
pub fn get_pre_raid_package(speed_modifier: f32) -> Package {
    vec![
        (0, BehaviorBuilder::boxed(RingBell::create())),
        (
            0,
            BehaviorBuilder::boxed(BehaviorBuilder::create(
                BehaviorBuilder::trigger_one_shuffled(vec![
                    (
                        control(SetWalkTargetFromBlockMemory::create(
                            &mem::MEETING_POINT,
                            speed_modifier * 1.5,
                            2,
                            150,
                            200,
                        )),
                        6,
                    ),
                    (
                        control(VillageBoundRandomStroll::create(speed_modifier * 1.5)),
                        2,
                    ),
                ]),
            )),
        ),
        get_minimal_look_behavior(),
        (99, BehaviorBuilder::boxed(reset_raid_status::create())),
    ]
}

/// Vanilla `VillagerGoalPackages.getRaidPackage`.
#[must_use]
pub fn get_raid_package(speed_modifier: f32) -> Package {
    vec![
        (
            0,
            BehaviorBuilder::boxed(BehaviorBuilder::create(BehaviorBuilder::sequence(
                trigger(BehaviorBuilder::trigger_if(raid_exists_and_not_victory)),
                trigger(BehaviorBuilder::trigger_one_shuffled(vec![
                    (control(MoveToSkySeeingSpot::create(speed_modifier)), 5),
                    (
                        control(VillageBoundRandomStroll::create(speed_modifier * 1.1)),
                        2,
                    ),
                ])),
            ))),
        ),
        (
            0,
            BehaviorBuilder::boxed(CelebrateVillagersSurvivedRaid::new(600, 600)),
        ),
        (
            2,
            BehaviorBuilder::boxed(BehaviorBuilder::create(BehaviorBuilder::sequence(
                trigger(BehaviorBuilder::trigger_if(raid_exists_and_active)),
                trigger(LocateHidingPlace::create(24, speed_modifier * 1.4, 1)),
            ))),
        ),
        get_minimal_look_behavior(),
        (99, BehaviorBuilder::boxed(reset_raid_status::create())),
    ]
}

/// Vanilla `VillagerGoalPackages.getHidePackage`.
#[must_use]
pub fn get_hide_package(speed_modifier: f32) -> Package {
    vec![
        (0, BehaviorBuilder::boxed(SetHiddenState::create(15, 3))),
        (
            1,
            BehaviorBuilder::boxed(LocateHidingPlace::create(32, speed_modifier * 1.25, 2)),
        ),
        get_minimal_look_behavior(),
    ]
}

/// Vanilla `VillagerGoalPackages.getFullLookBehavior`.
///
/// Vanilla weights: cat 8, villager 2, player 2, creature 1, water-creature 1,
/// axolotls 1, underground-water-creature 1, water-ambient 1, monster 1, idle 2.
fn get_full_look_behavior() -> (i32, Box<dyn BehaviorControl>) {
    (
        5,
        BehaviorBuilder::boxed(RunOne::new(vec![
            (
                BehaviorBuilder::boxed(SetEntityLookTarget::for_type(&vanilla_entities::CAT, 8.0)),
                8,
            ),
            (
                BehaviorBuilder::boxed(SetEntityLookTarget::for_type(
                    &vanilla_entities::VILLAGER,
                    8.0,
                )),
                2,
            ),
            (
                BehaviorBuilder::boxed(SetEntityLookTarget::for_type(
                    &vanilla_entities::PLAYER,
                    8.0,
                )),
                2,
            ),
            (
                BehaviorBuilder::boxed(SetEntityLookTarget::for_category(
                    MobCategory::Creature,
                    8.0,
                )),
                1,
            ),
            (
                BehaviorBuilder::boxed(SetEntityLookTarget::for_category(
                    MobCategory::WaterCreature,
                    8.0,
                )),
                1,
            ),
            (
                BehaviorBuilder::boxed(SetEntityLookTarget::for_category(
                    MobCategory::Axolotls,
                    8.0,
                )),
                1,
            ),
            (
                BehaviorBuilder::boxed(SetEntityLookTarget::for_category(
                    MobCategory::UndergroundWaterCreature,
                    8.0,
                )),
                1,
            ),
            (
                BehaviorBuilder::boxed(SetEntityLookTarget::for_category(
                    MobCategory::WaterAmbient,
                    8.0,
                )),
                1,
            ),
            (
                BehaviorBuilder::boxed(SetEntityLookTarget::for_category(
                    MobCategory::Monster,
                    8.0,
                )),
                1,
            ),
            (BehaviorBuilder::boxed(DoNothing::new(30, 60)), 2),
        ])),
    )
}

/// Vanilla `VillagerGoalPackages.getMinimalLookBehavior`.
///
/// Vanilla weights: villager 2, player 2, idle 8.
fn get_minimal_look_behavior() -> (i32, Box<dyn BehaviorControl>) {
    (
        5,
        BehaviorBuilder::boxed(RunOne::new(vec![
            (
                BehaviorBuilder::boxed(SetEntityLookTarget::for_type(
                    &vanilla_entities::VILLAGER,
                    8.0,
                )),
                2,
            ),
            (
                BehaviorBuilder::boxed(SetEntityLookTarget::for_type(
                    &vanilla_entities::PLAYER,
                    8.0,
                )),
                2,
            ),
            (BehaviorBuilder::boxed(DoNothing::new(30, 60)), 8),
        ])),
    )
}

/// Vanilla `VillagerGoalPackages.raidExistsAndActive`.
///
/// Steel has no raid manager, so no raid can be active.
const fn raid_exists_and_active(_ctx: &mut BehaviorContext<'_>) -> bool {
    false
}

/// Vanilla `VillagerGoalPackages.raidExistsAndNotVictory`.
///
/// Steel has no raid manager, so no raid can gate this package.
const fn raid_exists_and_not_victory(_ctx: &mut BehaviorContext<'_>) -> bool {
    false
}

/// Boxes a trigger for the declarative compositions.
fn trigger(value: impl Trigger + 'static) -> Box<dyn Trigger> {
    Box::new(value)
}

/// Vanilla `BehaviorControl extends Trigger`: a behavior used as a trigger
/// attempt-starts itself. Steel keeps the two traits separate, so the
/// `triggerOneShuffled`/`sequence` compositions wrap controls explicitly.
struct ControlAsTrigger(Box<dyn BehaviorControl>);

impl Trigger for ControlAsTrigger {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        self.0.try_start(ctx)
    }
}

/// Wraps a behavior for the declarative compositions.
fn control(behavior: impl BehaviorControl + 'static) -> Box<dyn Trigger> {
    Box::new(ControlAsTrigger(Box::new(behavior)))
}

#[cfg(test)]
mod tests {
    use steel_registry::vanilla_villager_professions;

    use super::*;

    fn priorities(package: Package) -> Vec<i32> {
        package.into_iter().map(|(priority, _)| priority).collect()
    }

    /// Vanilla priority tables from `VillagerGoalPackages`. The `RunOne`/`GateBehavior`
    /// weights are not introspectable, so the priority sequence is what pins each
    /// package to its vanilla shape.
    #[test]
    fn packages_keep_vanilla_priorities() {
        let farmer: VillagerProfessionRef = &vanilla_villager_professions::FARMER;
        assert_eq!(
            priorities(get_core_package(farmer, 0.5)),
            [
                0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 5, 6, 7, 8, 10, 10, 10, 10
            ]
        );
        assert_eq!(
            priorities(get_work_package(farmer, 0.5)),
            [5, 5, 10, 10, 2, 3, 99]
        );
        assert_eq!(priorities(get_play_package(0.5)), [0, 5, 5, 5, 99]);
        assert_eq!(priorities(get_rest_package(0.5)), [2, 3, 3, 5, 5, 99]);
        assert_eq!(
            priorities(get_meet_package(0.5)),
            [2, 10, 10, 2, 3, 3, 3, 5, 99]
        );
        assert_eq!(priorities(get_idle_package(0.5)), [2, 3, 3, 3, 3, 3, 5, 99]);
        assert_eq!(priorities(get_panic_package(0.5)), [0, 1, 1, 3, 5]);
        assert_eq!(priorities(get_pre_raid_package(0.5)), [0, 0, 5, 99]);
        assert_eq!(priorities(get_raid_package(0.5)), [0, 0, 2, 5, 99]);
        assert_eq!(priorities(get_hide_package(0.5)), [0, 1, 5]);
    }

    /// The work package keeps the same shape for every profession; only the work
    /// behavior itself (and the two harvest weights) differ.
    #[test]
    fn work_package_selects_work_behavior_by_profession() {
        let farmer: VillagerProfessionRef = &vanilla_villager_professions::FARMER;
        let librarian: VillagerProfessionRef = &vanilla_villager_professions::LIBRARIAN;

        assert_eq!(
            priorities(get_work_package(farmer, 0.5)),
            priorities(get_work_package(librarian, 0.5))
        );
        assert!(
            work_at_poi(farmer)
                .debug_string()
                .contains("WorkAtComposter")
        );
        assert!(work_at_poi(librarian).debug_string().contains("WorkAtPoi"));
    }
}
