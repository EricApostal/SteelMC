//! Vanilla `PiglinAi`/`PiglinBruteAi` activity packages.
//!
//! The statics the packages call live in `entity::ai::behavior::piglin::ai`; this
//! module mirrors vanilla's `initCoreActivity`/`initIdleActivity`/… structure and the
//! two `customServerAiStep` brain halves, exactly like `hoglin::brain`.
//!
//! Documented deviation: vanilla plays `getSoundForCurrentActivity` inside
//! `PiglinAi.updateActivity`; Steel's entity plays it after the brain tick, when the
//! activity changed (same transition, outside the brain lock).

use steel_registry::vanilla_entities;
use steel_utils::GlobalPos;

use crate::entity::ai::behavior::back_up_if_too_close::BackUpIfTooClose;
use crate::entity::ai::behavior::copy_memory_with_expiry::CopyMemoryWithExpiry;
use crate::entity::ai::behavior::crossbow_attack::CrossbowAttack;
use crate::entity::ai::behavior::dismount_or_skip_mounting::DismountOrSkipMounting;
use crate::entity::ai::behavior::do_nothing::DoNothing;
use crate::entity::ai::behavior::erase_memory_if::EraseMemoryIf;
use crate::entity::ai::behavior::go_to_target_location::GoToTargetLocation;
use crate::entity::ai::behavior::go_to_wanted_item::GoToWantedItem;
use crate::entity::ai::behavior::interact_with::InteractWith;
use crate::entity::ai::behavior::interact_with_door::InteractWithDoor;
use crate::entity::ai::behavior::look_at_target_sink::LookAtTargetSink;
use crate::entity::ai::behavior::melee_attack::MeleeAttack;
use crate::entity::ai::behavior::mount::Mount;
use crate::entity::ai::behavior::move_to_target_sink::MoveToTargetSink;
use crate::entity::ai::behavior::piglin::ai;
use crate::entity::ai::behavior::piglin::remember_if_hoglin_was_killed::RememberIfHoglinWasKilled;
use crate::entity::ai::behavior::piglin::start_admiring_item_if_seen::StartAdmiringItemIfSeen;
use crate::entity::ai::behavior::piglin::start_hunting_hoglin::StartHuntingHoglin;
use crate::entity::ai::behavior::piglin::stop_admiring_if_item_too_far_away::StopAdmiringIfItemTooFarAway;
use crate::entity::ai::behavior::piglin::stop_admiring_if_tired_of_trying_to_reach_item::StopAdmiringIfTiredOfTryingToReachItem;
use crate::entity::ai::behavior::piglin::stop_holding_item_if_no_longer_admiring::StopHoldingItemIfNoLongerAdmiring;
use crate::entity::ai::behavior::random_stroll::RandomStroll;
use crate::entity::ai::behavior::set_entity_look_target::SetEntityLookTarget;
use crate::entity::ai::behavior::set_look_and_interact::SetLookAndInteract;
use crate::entity::ai::behavior::set_walk_target_away_from::SetWalkTargetAwayFrom;
use crate::entity::ai::behavior::set_walk_target_from_attack_target_if_target_out_of_reach::SetWalkTargetFromAttackTargetIfTargetOutOfReach;
use crate::entity::ai::behavior::set_walk_target_from_look_target::SetWalkTargetFromLookTarget;
use crate::entity::ai::behavior::spear_approach::SpearApproach;
use crate::entity::ai::behavior::spear_attack::SpearAttack;
use crate::entity::ai::behavior::spear_retreat::SpearRetreat;
use crate::entity::ai::behavior::start_attacking::StartAttacking;
use crate::entity::ai::behavior::start_celebrating_if_target_dead::StartCelebratingIfTargetDead;
use crate::entity::ai::behavior::stop_attacking_if_target_invalid::StopAttackingIfTargetInvalid;
use crate::entity::ai::behavior::stop_being_angry_if_target_dead::StopBeingAngryIfTargetDead;
use crate::entity::ai::behavior::stroll_around_poi::StrollAroundPoi;
use crate::entity::ai::behavior::stroll_to_poi::StrollToPoi;
use crate::entity::ai::brain::SensorType;
use crate::entity::ai::brain::activity::Activity;
use crate::entity::ai::brain::behavior::builder::BehaviorBuilder;
use crate::entity::ai::brain::behavior::gate::RunOne;
use crate::entity::ai::brain::behavior::{BehaviorContext, BehaviorControl, OneShot, Trigger};
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, module_type as mem};
use crate::entity::ai::brain::{ActivityData, Brain};
use crate::entity::ai::sensing::piglin_brute_specific::PIGLIN_BRUTE_SPECIFIC_SENSOR;
use crate::entity::ai::sensing::piglin_specific::PIGLIN_SPECIFIC_SENSOR;
use crate::entity::ai::sensing::villager::{
    HURT_BY, NEAREST_ITEMS, NEAREST_LIVING_ENTITIES, NEAREST_PLAYERS,
};
use crate::entity::{Entity as _, LivingEntity as _, SharedEntity};
use crate::world::World;

/// Vanilla `PiglinAi.initMemories`.
pub(crate) fn init_memories(brain: &mut Brain) {
    let duration = i64::from(ai::sample(ai::TIME_BETWEEN_HUNTS));
    brain.set_memory_with_expiry(&mem::HUNTED_RECENTLY, true, duration);
}

/// Vanilla `PiglinBruteAi.initMemories`: the spawn position becomes `HOME`.
pub(crate) fn init_brute_memories(brain: &mut Brain, level: &World, body: &SharedEntity) {
    brain.set_memory(
        &mem::HOME,
        Some(GlobalPos::new(level.key.clone(), body.block_position())),
    );
}

/// Vanilla `Piglin.BRAIN_PROVIDER` memory list.
static PIGLIN_MEMORY_TYPES: [MemoryModuleTypeRef; 6] = [
    &mem::UNIVERSAL_ANGER,
    &mem::ATE_RECENTLY,
    &mem::SPEAR_FLEEING_TIME,
    &mem::SPEAR_FLEEING_POSITION,
    &mem::SPEAR_CHARGE_POSITION,
    &mem::SPEAR_ENGAGE_TIME,
];

/// Vanilla `Piglin.BRAIN_PROVIDER` sensor list.
static PIGLIN_SENSOR_TYPES: [&SensorType; 5] = [
    &NEAREST_LIVING_ENTITIES,
    &NEAREST_PLAYERS,
    &NEAREST_ITEMS,
    &HURT_BY,
    &PIGLIN_SPECIFIC_SENSOR,
];

/// Vanilla `PiglinBrute.BRAIN_PROVIDER` memory list.
static PIGLIN_BRUTE_MEMORY_TYPES: [MemoryModuleTypeRef; 1] = [&mem::NEAREST_VISIBLE_ADULT_PIGLINS];

/// Vanilla `PiglinBrute.BRAIN_PROVIDER` sensor list.
static PIGLIN_BRUTE_SENSOR_TYPES: [&SensorType; 5] = [
    &NEAREST_LIVING_ENTITIES,
    &NEAREST_PLAYERS,
    &NEAREST_ITEMS,
    &HURT_BY,
    &PIGLIN_BRUTE_SPECIFIC_SENSOR,
];

/// Vanilla `Piglin.makeBrain`.
#[must_use]
pub(crate) fn make_piglin_brain() -> Brain {
    Brain::new(
        &PIGLIN_MEMORY_TYPES,
        &PIGLIN_SENSOR_TYPES,
        piglin_activities(),
    )
}

/// Vanilla `PiglinBrute.makeBrain`.
#[must_use]
pub(crate) fn make_piglin_brute_brain() -> Brain {
    Brain::new(
        &PIGLIN_BRUTE_MEMORY_TYPES,
        &PIGLIN_BRUTE_SENSOR_TYPES,
        piglin_brute_activities(),
    )
}

/// Vanilla `Piglin.customServerAiStep` brain half.
pub(crate) fn tick_piglin(brain: &mut Brain, level: &World, body: &SharedEntity) {
    brain.tick(level, body);
}

/// Vanilla `PiglinBrute.customServerAiStep` brain half.
pub(crate) fn tick_piglin_brute(brain: &mut Brain, level: &World, body: &SharedEntity) {
    brain.tick(level, body);
}

/// The state `PiglinAi.updateActivity` mirrors back onto the entity.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PiglinActivityUpdate {
    /// The active non-core activity changed this tick.
    pub activity_changed: bool,
    /// Vanilla `body.setAggressive(brain.hasMemoryValue(ATTACK_TARGET))`.
    pub aggressive: bool,
    /// Vanilla `body.setDancing(brain.hasMemoryValue(DANCING))`.
    pub dancing: bool,
    /// Vanilla `brain.hasMemoryValue(RIDE_TARGET)`.
    pub has_ride_target: bool,
}

/// Vanilla `PiglinAi.updateActivity`.
pub(crate) fn update_piglin_activity(brain: &mut Brain) -> PiglinActivityUpdate {
    let old_activity = brain.get_active_non_core_activity();
    brain.set_active_activity_to_first_valid(&[
        Activity::AdmireItem,
        Activity::Fight,
        Activity::Avoid,
        Activity::Celebrate,
        Activity::Ride,
        Activity::Idle,
    ]);
    let activity_changed = old_activity != brain.get_active_non_core_activity();

    if !brain.has_memory_value(&mem::CELEBRATE_LOCATION) {
        brain.erase_memory(&mem::DANCING);
    }

    PiglinActivityUpdate {
        activity_changed,
        aggressive: brain.has_memory_value(&mem::ATTACK_TARGET),
        dancing: brain.has_memory_value(&mem::DANCING),
        has_ride_target: brain.has_memory_value(&mem::RIDE_TARGET),
    }
}

/// Vanilla `PiglinBruteAi.updateActivity`.
pub(crate) fn update_piglin_brute_activity(brain: &mut Brain) -> PiglinActivityUpdate {
    let old_activity = brain.get_active_non_core_activity();
    brain.set_active_activity_to_first_valid(&[Activity::Fight, Activity::Idle]);
    PiglinActivityUpdate {
        activity_changed: old_activity != brain.get_active_non_core_activity(),
        aggressive: brain.has_memory_value(&mem::ATTACK_TARGET),
        dancing: false,
        has_ride_target: false,
    }
}

/// Vanilla `BehaviorBuilder.triggerIf(predicate, behavior)` over Steel's
/// [`BehaviorControl`] tree, matching `hoglin::brain`'s helper.
fn trigger_if_behavior(
    predicate: fn(&mut BehaviorContext<'_>) -> bool,
    behavior: impl BehaviorControl + 'static,
) -> OneShot<TriggerIfBehavior> {
    OneShot::new(TriggerIfBehavior {
        predicate,
        behavior: Box::new(behavior),
    })
}

/// Vanilla `BehaviorBuilder.triggerIf(predicate, behavior)`.
struct TriggerIfBehavior {
    predicate: fn(&mut BehaviorContext<'_>) -> bool,
    behavior: Box<dyn BehaviorControl>,
}

impl Trigger for TriggerIfBehavior {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        (self.predicate)(ctx) && self.behavior.try_start(ctx)
    }
}

/// Vanilla `PiglinAi.getActivities`.
#[must_use]
fn piglin_activities() -> Vec<ActivityData> {
    vec![
        piglin_core_activity(),
        piglin_idle_activity(),
        piglin_admire_item_activity(),
        piglin_fight_activity(),
        piglin_celebrate_activity(),
        piglin_retreat_activity(),
        piglin_ride_hoglin_activity(),
    ]
}

/// Vanilla `PiglinAi.initCoreActivity`.
fn piglin_core_activity() -> ActivityData {
    ActivityData::create_priority(
        Activity::Core,
        0,
        vec![
            BehaviorBuilder::boxed(LookAtTargetSink::new(45, 90)),
            BehaviorBuilder::boxed(MoveToTargetSink::new()),
            BehaviorBuilder::boxed(InteractWithDoor::create()),
            BehaviorBuilder::boxed(baby_avoid_nemesis()),
            BehaviorBuilder::boxed(avoid_zombified()),
            BehaviorBuilder::boxed(StopHoldingItemIfNoLongerAdmiring::create()),
            BehaviorBuilder::boxed(StartAdmiringItemIfSeen::create(ai::ADMIRE_DURATION as i32)),
            BehaviorBuilder::boxed(StartCelebratingIfTargetDead::create(
                ai::CELEBRATION_TIME,
                ai::wants_to_dance,
            )),
            BehaviorBuilder::boxed(StopBeingAngryIfTargetDead::create()),
        ],
    )
}

/// Vanilla `PiglinAi.initIdleActivity`.
fn piglin_idle_activity() -> ActivityData {
    ActivityData::create_priority(
        Activity::Idle,
        10,
        vec![
            BehaviorBuilder::boxed(SetEntityLookTarget::create_with_predicate(
                ai::is_player_holding_loved_item,
                ai::MAX_LOOK_DIST_FOR_PLAYER_HOLDING_LOVED_ITEM,
            )),
            BehaviorBuilder::boxed(StartAttacking::create_if(
                is_adult_for_attack,
                ai::find_nearest_valid_attack_target,
            )),
            BehaviorBuilder::boxed(trigger_if_behavior(
                |ctx| ai::can_hunt(ctx.body),
                StartHuntingHoglin::create(),
            )),
            BehaviorBuilder::boxed(avoid_repellent()),
            BehaviorBuilder::boxed(baby_sometimes_ride_baby_hoglin()),
            BehaviorBuilder::boxed(idle_look_behaviors()),
            BehaviorBuilder::boxed(idle_movement_behaviors()),
            BehaviorBuilder::boxed(SetLookAndInteract::create(&vanilla_entities::PLAYER, 4)),
        ],
    )
}

/// Vanilla `PiglinAi.initAdmireItemActivity`.
fn piglin_admire_item_activity() -> ActivityData {
    ActivityData::create_with_erase(
        Activity::AdmireItem,
        10,
        vec![
            BehaviorBuilder::boxed(GoToWantedItem::create(
                ai::SPEED_MULTIPLIER_WHEN_GOING_TO_WANTED_ITEM,
                true,
                ai::MAX_DISTANCE_TO_WALK_TO_ITEM,
            )),
            BehaviorBuilder::boxed(StopAdmiringIfItemTooFarAway::create(
                ai::MAX_DISTANCE_TO_WALK_TO_ITEM,
            )),
            BehaviorBuilder::boxed(StopAdmiringIfTiredOfTryingToReachItem::create(
                ai::MAX_TIME_TO_WALK_TO_ITEM,
                ai::HOW_LONG_TIME_TO_DISABLE_ADMIRE_WALKING_IF_CANT_REACH_ITEM,
            )),
        ],
        &mem::ADMIRING_ITEM,
    )
}

/// Vanilla `PiglinAi.initFightActivity`.
fn piglin_fight_activity() -> ActivityData {
    ActivityData::create_with_erase(
        Activity::Fight,
        10,
        vec![
            BehaviorBuilder::boxed(StopAttackingIfTargetInvalid::create_with(
                ai::is_nearest_valid_attack_target,
            )),
            BehaviorBuilder::boxed(trigger_if_behavior(
                |ctx| ai::has_crossbow(ctx.body),
                BackUpIfTooClose::create(
                    ai::MIN_DESIRED_DIST_FROM_TARGET_WHEN_HOLDING_CROSSBOW,
                    ai::SPEED_WHEN_STRAFING_BACK_FROM_TARGET,
                ),
            )),
            BehaviorBuilder::boxed(SetWalkTargetFromAttackTargetIfTargetOutOfReach::create(1.0)),
            BehaviorBuilder::boxed(SpearApproach::new(1.0, 10.0)),
            BehaviorBuilder::boxed(SpearAttack::new(1.0, 1.0, 2.0)),
            BehaviorBuilder::boxed(SpearRetreat::new(1.0)),
            BehaviorBuilder::boxed(MeleeAttack::create(ai::MELEE_ATTACK_COOLDOWN)),
            BehaviorBuilder::boxed(CrossbowAttack::new()),
            BehaviorBuilder::boxed(RememberIfHoglinWasKilled::create()),
            BehaviorBuilder::boxed(EraseMemoryIf::create(
                is_near_zombified,
                &mem::ATTACK_TARGET,
            )),
        ],
        &mem::ATTACK_TARGET,
    )
}

/// Vanilla `PiglinAi.initCelebrateActivity`.
fn piglin_celebrate_activity() -> ActivityData {
    ActivityData::create_with_erase(
        Activity::Celebrate,
        10,
        vec![
            BehaviorBuilder::boxed(avoid_repellent()),
            BehaviorBuilder::boxed(SetEntityLookTarget::create_with_predicate(
                ai::is_player_holding_loved_item,
                ai::MAX_LOOK_DIST_FOR_PLAYER_HOLDING_LOVED_ITEM,
            )),
            BehaviorBuilder::boxed(StartAttacking::create_if(
                is_adult_for_attack,
                ai::find_nearest_valid_attack_target,
            )),
            BehaviorBuilder::boxed(trigger_if_behavior(
                |ctx| !ctx.has_memory_value(&mem::DANCING),
                GoToTargetLocation::create(
                    &mem::CELEBRATE_LOCATION,
                    2,
                    ai::SPEED_MULTIPLIER_WHEN_GOING_TO_CELEBRATE_LOCATION,
                ),
            )),
            BehaviorBuilder::boxed(trigger_if_behavior(
                |ctx| ctx.has_memory_value(&mem::DANCING),
                GoToTargetLocation::create(
                    &mem::CELEBRATE_LOCATION,
                    4,
                    ai::SPEED_MULTIPLIER_WHEN_DANCING,
                ),
            )),
            BehaviorBuilder::boxed(RunOne::new(vec![
                (
                    BehaviorBuilder::boxed(SetEntityLookTarget::for_type(
                        &vanilla_entities::PIGLIN,
                        ai::MAX_LOOK_DIST,
                    )),
                    1,
                ),
                (
                    BehaviorBuilder::boxed(RandomStroll::stroll_with_distances(
                        ai::SPEED_MULTIPLIER_WHEN_IDLING,
                        2,
                        1,
                    )),
                    1,
                ),
                (BehaviorBuilder::boxed(DoNothing::new(10, 20)), 1),
            ])),
        ],
        &mem::CELEBRATE_LOCATION,
    )
}

/// Vanilla `PiglinAi.initRetreatActivity`.
fn piglin_retreat_activity() -> ActivityData {
    ActivityData::create_with_erase(
        Activity::Avoid,
        10,
        vec![
            BehaviorBuilder::boxed(SetWalkTargetAwayFrom::entity(
                &mem::AVOID_TARGET,
                ai::SPEED_MULTIPLIER_WHEN_RETREATING,
                ai::DESIRED_DISTANCE_FROM_ENTITY_WHEN_AVOIDING,
                true,
            )),
            BehaviorBuilder::boxed(idle_look_behaviors()),
            BehaviorBuilder::boxed(idle_movement_behaviors()),
            BehaviorBuilder::boxed(EraseMemoryIf::create(
                wants_to_stop_fleeing,
                &mem::AVOID_TARGET,
            )),
        ],
        &mem::AVOID_TARGET,
    )
}

/// Vanilla `PiglinAi.initRideHoglinActivity`.
fn piglin_ride_hoglin_activity() -> ActivityData {
    let mut shuffled = create_look_behaviors();
    // Vanilla appends `Pair.of(BehaviorBuilder.triggerIf(e -> true), 1)` as a no-op
    // fallback so the shuffled gate always succeeds.
    shuffled.push((
        Box::new(BehaviorBuilder::trigger_if(
            |_: &mut BehaviorContext<'_>| true,
        )),
        1,
    ));

    ActivityData::create_with_erase(
        Activity::Ride,
        10,
        vec![
            BehaviorBuilder::boxed(Mount::create(ai::SPEED_MULTIPLIER_WHEN_MOUNTING)),
            BehaviorBuilder::boxed(SetEntityLookTarget::create_with_predicate(
                ai::is_player_holding_loved_item,
                ai::MAX_LOOK_DIST,
            )),
            BehaviorBuilder::boxed(BehaviorBuilder::create(BehaviorBuilder::sequence(
                Box::new(BehaviorBuilder::trigger_if(
                    |ctx: &mut BehaviorContext<'_>| ctx.body.is_passenger(),
                )),
                Box::new(BehaviorBuilder::trigger_one_shuffled(shuffled)),
            ))),
            BehaviorBuilder::boxed(DismountOrSkipMounting::create(
                ai::MAX_WALK_DISTANCE_TO_START_RIDING,
                ai::wants_to_stop_riding,
            )),
        ],
        &mem::RIDE_TARGET,
    )
}

/// Vanilla `PiglinAi.createLookBehaviors`.
fn create_look_behaviors() -> Vec<(Box<dyn Trigger>, i32)> {
    vec![
        (
            Box::new(SetEntityLookTarget::for_type(
                &vanilla_entities::PLAYER,
                ai::MAX_LOOK_DIST,
            )),
            1,
        ),
        (
            Box::new(SetEntityLookTarget::for_type(
                &vanilla_entities::PIGLIN,
                ai::MAX_LOOK_DIST,
            )),
            1,
        ),
        (Box::new(SetEntityLookTarget::any(ai::MAX_LOOK_DIST)), 1),
    ]
}

/// Vanilla `PiglinAi.createIdleLookBehaviors`.
fn idle_look_behaviors() -> RunOne {
    RunOne::new(vec![
        (
            BehaviorBuilder::boxed(SetEntityLookTarget::for_type(
                &vanilla_entities::PLAYER,
                ai::MAX_LOOK_DIST,
            )),
            1,
        ),
        (
            BehaviorBuilder::boxed(SetEntityLookTarget::for_type(
                &vanilla_entities::PIGLIN,
                ai::MAX_LOOK_DIST,
            )),
            1,
        ),
        (
            BehaviorBuilder::boxed(SetEntityLookTarget::any(ai::MAX_LOOK_DIST)),
            1,
        ),
        (BehaviorBuilder::boxed(DoNothing::new(30, 60)), 1),
    ])
}

/// Vanilla `PiglinAi.createIdleMovementBehaviors`.
fn idle_movement_behaviors() -> RunOne {
    RunOne::new(vec![
        (
            BehaviorBuilder::boxed(RandomStroll::stroll(ai::SPEED_MULTIPLIER_WHEN_IDLING)),
            2,
        ),
        (
            BehaviorBuilder::boxed(InteractWith::of(
                &vanilla_entities::PIGLIN,
                ai::INTERACTION_RANGE,
                &mem::INTERACTION_TARGET,
                ai::SPEED_MULTIPLIER_WHEN_IDLING,
                2,
            )),
            2,
        ),
        (
            BehaviorBuilder::boxed(trigger_if_behavior(
                doesnt_see_any_player_holding_loved_item,
                SetWalkTargetFromLookTarget::create(ai::SPEED_MULTIPLIER_WHEN_IDLING, 3),
            )),
            2,
        ),
        (BehaviorBuilder::boxed(DoNothing::new(30, 60)), 1),
    ])
}

/// Vanilla `PiglinAi.avoidRepellent`.
fn avoid_repellent() -> OneShot<SetWalkTargetAwayFrom> {
    SetWalkTargetAwayFrom::pos(
        &mem::NEAREST_REPELLENT,
        ai::SPEED_MULTIPLIER_WHEN_AVOIDING,
        ai::INTERACTION_RANGE,
        false,
    )
}

/// Vanilla `PiglinAi.babyAvoidNemesis`.
fn baby_avoid_nemesis() -> OneShot<CopyMemoryWithExpiry> {
    CopyMemoryWithExpiry::create(
        is_baby,
        &mem::NEAREST_VISIBLE_NEMESIS,
        &mem::AVOID_TARGET,
        ai::BABY_AVOID_NEMESIS_DURATION,
    )
}

/// Vanilla `PiglinAi.avoidZombified`.
fn avoid_zombified() -> OneShot<CopyMemoryWithExpiry> {
    CopyMemoryWithExpiry::create(
        is_near_zombified_immutable,
        &mem::NEAREST_VISIBLE_ZOMBIFIED,
        &mem::AVOID_TARGET,
        ai::AVOID_ZOMBIFIED_DURATION,
    )
}

/// Vanilla `PiglinAi.babySometimesRideBabyHoglin`.
///
/// Documented deviation: vanilla keeps one `SetEntityLookTargetSometimes.Ticker` per
/// piglin; Steel behaviors are shared, so the interval is sampled per tick with the
/// same `RIDE_START_INTERVAL` distribution instead of counting down.
fn baby_sometimes_ride_baby_hoglin() -> OneShot<CopyMemoryWithExpiry> {
    CopyMemoryWithExpiry::create(
        |ctx| {
            ctx.body
                .as_living_entity()
                .is_some_and(LivingEntity::is_baby)
                && ai::sample(ai::RIDE_START_INTERVAL) == 0
        },
        &mem::NEAREST_VISIBLE_BABY_HOGLIN,
        &mem::RIDE_TARGET,
        ai::RIDE_DURATION,
    )
}

/// `Piglin::isBaby` as a `BehaviorContext` predicate.
fn is_baby(ctx: &BehaviorContext<'_>) -> bool {
    ctx.body
        .as_living_entity()
        .is_some_and(LivingEntity::is_baby)
}

/// `PiglinAi.isNearZombified` as an immutable `BehaviorContext` predicate
/// (`CopyMemoryWithExpiry` takes `&BehaviorContext`).
fn is_near_zombified_immutable(ctx: &BehaviorContext<'_>) -> bool {
    ai::is_near_zombified(ctx.memories(), ctx.body)
}

/// `PiglinAi.isNearZombified` as a mutable `BehaviorContext` predicate (`EraseMemoryIf`).
fn is_near_zombified(ctx: &mut BehaviorContext<'_>) -> bool {
    ai::is_near_zombified(ctx.memories(), ctx.body)
}

/// `PiglinAi.wantsToStopFleeing` as a `BehaviorContext` predicate.
fn wants_to_stop_fleeing(ctx: &mut BehaviorContext<'_>) -> bool {
    ai::wants_to_stop_fleeing(ctx.memories(), ctx.body)
}

/// `PiglinAi.doesntSeeAnyPlayerHoldingLovedItem` as a `BehaviorContext` predicate.
fn doesnt_see_any_player_holding_loved_item(ctx: &mut BehaviorContext<'_>) -> bool {
    ai::doesnt_see_any_player_holding_loved_item(ctx.memories())
}

/// Vanilla `Piglin::isAdult` gate for `StartAttacking`.
fn is_adult_for_attack(_ctx: &BehaviorContext<'_>, body: &SharedEntity) -> bool {
    ai::is_adult(body)
}

/// Vanilla `PiglinBruteAi.getActivities`.
#[must_use]
fn piglin_brute_activities() -> Vec<ActivityData> {
    vec![
        piglin_brute_core_activity(),
        piglin_brute_idle_activity(),
        piglin_brute_fight_activity(),
    ]
}

/// Vanilla `PiglinBruteAi.initCoreActivity`.
fn piglin_brute_core_activity() -> ActivityData {
    ActivityData::create_priority(
        Activity::Core,
        0,
        vec![
            BehaviorBuilder::boxed(LookAtTargetSink::new(45, 90)),
            BehaviorBuilder::boxed(MoveToTargetSink::new()),
            BehaviorBuilder::boxed(InteractWithDoor::create()),
            BehaviorBuilder::boxed(StopBeingAngryIfTargetDead::create()),
        ],
    )
}

/// Vanilla `PiglinBruteAi.initIdleActivity`.
fn piglin_brute_idle_activity() -> ActivityData {
    ActivityData::create_priority(
        Activity::Idle,
        10,
        vec![
            BehaviorBuilder::boxed(StartAttacking::create(
                ai::find_nearest_valid_attack_target_brute,
            )),
            BehaviorBuilder::boxed(brute_idle_look_behaviors()),
            BehaviorBuilder::boxed(brute_idle_movement_behaviors()),
            BehaviorBuilder::boxed(SetLookAndInteract::create(&vanilla_entities::PLAYER, 4)),
        ],
    )
}

/// Vanilla `PiglinBruteAi.initFightActivity`.
fn piglin_brute_fight_activity() -> ActivityData {
    ActivityData::create_with_erase(
        Activity::Fight,
        10,
        vec![
            BehaviorBuilder::boxed(StopAttackingIfTargetInvalid::create_with(
                is_nearest_valid_brute_attack_target,
            )),
            BehaviorBuilder::boxed(SetWalkTargetFromAttackTargetIfTargetOutOfReach::create(1.0)),
            BehaviorBuilder::boxed(MeleeAttack::create(ai::MELEE_ATTACK_COOLDOWN)),
        ],
        &mem::ATTACK_TARGET,
    )
}

/// Vanilla `PiglinBruteAi.isNearestValidAttackTarget`.
fn is_nearest_valid_brute_attack_target(ctx: &BehaviorContext<'_>, target: &SharedEntity) -> bool {
    ai::find_nearest_valid_attack_target_brute(ctx, ctx.body)
        .is_some_and(|nearest| nearest.id() == target.id())
}

/// Vanilla `PiglinBruteAi.createIdleLookBehaviors`.
fn brute_idle_look_behaviors() -> RunOne {
    RunOne::new(vec![
        (
            BehaviorBuilder::boxed(SetEntityLookTarget::for_type(
                &vanilla_entities::PLAYER,
                ai::MAX_LOOK_DIST,
            )),
            1,
        ),
        (
            BehaviorBuilder::boxed(SetEntityLookTarget::for_type(
                &vanilla_entities::PIGLIN,
                ai::MAX_LOOK_DIST,
            )),
            1,
        ),
        (
            BehaviorBuilder::boxed(SetEntityLookTarget::for_type(
                &vanilla_entities::PIGLIN_BRUTE,
                ai::MAX_LOOK_DIST,
            )),
            1,
        ),
        (
            BehaviorBuilder::boxed(SetEntityLookTarget::any(ai::MAX_LOOK_DIST)),
            1,
        ),
        (BehaviorBuilder::boxed(DoNothing::new(30, 60)), 1),
    ])
}

/// Vanilla `PiglinBruteAi.createIdleMovementBehaviors`.
fn brute_idle_movement_behaviors() -> RunOne {
    RunOne::new(vec![
        (
            BehaviorBuilder::boxed(RandomStroll::stroll(ai::SPEED_MULTIPLIER_WHEN_IDLING)),
            2,
        ),
        (
            BehaviorBuilder::boxed(InteractWith::of(
                &vanilla_entities::PIGLIN,
                ai::INTERACTION_RANGE,
                &mem::INTERACTION_TARGET,
                ai::SPEED_MULTIPLIER_WHEN_IDLING,
                2,
            )),
            2,
        ),
        (
            BehaviorBuilder::boxed(InteractWith::of(
                &vanilla_entities::PIGLIN_BRUTE,
                ai::INTERACTION_RANGE,
                &mem::INTERACTION_TARGET,
                ai::SPEED_MULTIPLIER_WHEN_IDLING,
                2,
            )),
            2,
        ),
        (
            BehaviorBuilder::boxed(StrollToPoi::create(
                &mem::HOME,
                ai::SPEED_MULTIPLIER_WHEN_IDLING,
                2,
                100,
            )),
            2,
        ),
        (
            BehaviorBuilder::boxed(StrollAroundPoi::create(
                &mem::HOME,
                ai::SPEED_MULTIPLIER_WHEN_IDLING,
                5,
            )),
            2,
        ),
        (BehaviorBuilder::boxed(DoNothing::new(30, 60)), 1),
    ])
}
