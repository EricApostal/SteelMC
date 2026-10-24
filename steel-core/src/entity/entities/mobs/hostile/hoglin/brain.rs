//! Vanilla `HoglinAi` and `Zoglin.getActivities`.
//!
//! Both mobs build their brain from a sensor list plus their activity packages and
//! switch activities from `customServerAiStep` after the brain tick; Steel keeps
//! that split in free functions, exactly like `WardenAi`:
//!
//! - [`make_hoglin_brain`] / [`make_zoglin_brain`] are the `BRAIN_PROVIDER`s,
//! - [`tick_hoglin`] / [`tick_zoglin`] are the `customServerAiStep` brain halves,
//! - [`update_hoglin_activity`] / [`update_zoglin_activity`] are the activity
//!   switches.
//!
//! ## Vanilla state with no brain-agnostic counterpart
//!
//! Vanilla's finders and predicates close over the mob
//! (`Hoglin::findNearestValidAttackTarget`, `Zoglin::isAdult`, …) and read the
//! brain behind it. The brain functions here stay entity-agnostic
//! (`&SharedEntity`/`&World`), so the same tests read the memories through
//! [`BehaviorContext`] and the cross-cutting `is_baby()` capability; the one
//! entity-side effect vanilla performs while switching activities
//! (`makeSound(getSoundForCurrentActivity)` / `playAngrySound`) stays with the
//! entity's own `customServerAiStep`.

use steel_registry::vanilla_entities;
use steel_utils::value_providers::UniformIntProvider;

use crate::entity::LivingEntity;
use crate::entity::SharedEntity;
use crate::entity::ai::behavior::animal_make_love::AnimalMakeLove;
use crate::entity::ai::behavior::baby_follow_adult::BabyFollowAdult;
use crate::entity::ai::behavior::become_passive_if_memory_present::BecomePassiveIfMemoryPresent;
use crate::entity::ai::behavior::do_nothing::DoNothing;
use crate::entity::ai::behavior::erase_memory_if::EraseMemoryIf;
use crate::entity::ai::behavior::look_at_target_sink::LookAtTargetSink;
use crate::entity::ai::behavior::melee_attack::MeleeAttack;
use crate::entity::ai::behavior::move_to_target_sink::MoveToTargetSink;
use crate::entity::ai::behavior::random_stroll::RandomStroll;
use crate::entity::ai::behavior::set_entity_look_target_sometimes::SetEntityLookTargetSometimes;
use crate::entity::ai::behavior::set_walk_target_away_from::SetWalkTargetAwayFrom;
use crate::entity::ai::behavior::set_walk_target_from_attack_target_if_target_out_of_reach::SetWalkTargetFromAttackTargetIfTargetOutOfReach;
use crate::entity::ai::behavior::set_walk_target_from_look_target::SetWalkTargetFromLookTarget;
use crate::entity::ai::behavior::start_attacking::StartAttacking;
use crate::entity::ai::behavior::stop_attacking_if_target_invalid::StopAttackingIfTargetInvalid;
use crate::entity::ai::brain::SensorType;
use crate::entity::ai::brain::activity::Activity;
use crate::entity::ai::brain::behavior::builder::BehaviorBuilder;
use crate::entity::ai::brain::behavior::gate::RunOne;
use crate::entity::ai::brain::behavior::{BehaviorContext, BehaviorControl, OneShot, Trigger};
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStatus, module_type as mem};
use crate::entity::ai::brain::{ActivityData, Brain};
use crate::entity::ai::sensing::adult::NEAREST_ADULT;
use crate::entity::ai::sensing::hoglin_specific::HOGLIN_SPECIFIC_SENSOR;
use crate::entity::ai::sensing::villager::{
    NEAREST_LIVING_ENTITIES, NEAREST_PLAYERS, follow_range, is_entity_attackable,
};
use crate::world::World;

/// Vanilla `HoglinAi.ADULT_FOLLOW_RANGE`, `UniformInt.of(5, 16)`.
const ADULT_FOLLOW_RANGE: UniformIntProvider = UniformIntProvider {
    min_inclusive: 5,
    max_inclusive: 16,
};
/// Vanilla `HoglinAi.ATTACK_INTERVAL`.
const ATTACK_INTERVAL: i32 = 40;
/// Vanilla `HoglinAi.BABY_ATTACK_INTERVAL`.
const BABY_ATTACK_INTERVAL: i32 = 15;
/// Vanilla `HoglinAi.REPELLENT_PACIFY_TIME`.
const REPELLENT_PACIFY_TIME: i32 = 200;
/// Vanilla `HoglinAi.SPEED_MULTIPLIER_WHEN_IDLING`.
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 0.4;
/// Vanilla `HoglinAi.SPEED_MULTIPLIER_WHEN_RETREATING`.
const SPEED_MULTIPLIER_WHEN_RETREATING: f32 = 1.3;
/// Vanilla `HoglinAi.SPEED_MULTIPLIER_WHEN_MAKING_LOVE`.
const SPEED_MULTIPLIER_WHEN_MAKING_LOVE: f32 = 0.6;
/// Vanilla `HoglinAi.SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT`.
const SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT: f32 = 0.6;
/// Vanilla `HoglinAi.SPEED_MULTIPLIER_WHEN_AVOIDING_REPELLENT`.
const SPEED_MULTIPLIER_WHEN_AVOIDING_REPELLENT: f32 = 1.0;
/// Vanilla `SetEntityLookTargetSometimes` interval, `UniformInt.of(30, 60)`.
const LOOK_INTERVAL: UniformIntProvider = UniformIntProvider {
    min_inclusive: 30,
    max_inclusive: 60,
};
/// Vanilla `Zoglin` `SPEED_MULTIPLIER_WHEN_IDLING`.
const ZOGLIN_SPEED_MULTIPLIER_WHEN_IDLING: f32 = 0.4;
/// Vanilla `Zoglin` `SPEED_MULTIPLIER_WHEN_FIGHTING`.
const ZOGLIN_SPEED_MULTIPLIER_WHEN_FIGHTING: f32 = 1.0;
/// Vanilla `Zoglin` attack intervals for adults and babies.
const ZOGLIN_ATTACK_INTERVAL: i32 = 40;
/// Vanilla `Zoglin` baby attack interval.
const ZOGLIN_BABY_ATTACK_INTERVAL: i32 = 15;

/// Vanilla `Hoglin.BRAIN_PROVIDER` sensor list.
static HOGLIN_SENSOR_TYPES: [&SensorType; 4] = [
    &NEAREST_LIVING_ENTITIES,
    &NEAREST_PLAYERS,
    &NEAREST_ADULT,
    &HOGLIN_SPECIFIC_SENSOR,
];

/// Vanilla `Zoglin.BRAIN_PROVIDER` sensor list.
static ZOGLIN_SENSOR_TYPES: [&SensorType; 2] = [&NEAREST_LIVING_ENTITIES, &NEAREST_PLAYERS];

/// Vanilla `Hoglin.BRAIN_PROVIDER` memory list (none beyond the sensor sets).
static HOGLIN_MEMORY_TYPES: [MemoryModuleTypeRef; 0] = [];

/// Vanilla `Zoglin.BRAIN_PROVIDER` memory list (none beyond the sensor sets).
static ZOGLIN_MEMORY_TYPES: [MemoryModuleTypeRef; 0] = [];

/// Vanilla `Hoglin.makeBrain`.
#[must_use]
pub(crate) fn make_hoglin_brain() -> Brain {
    Brain::new(
        &HOGLIN_MEMORY_TYPES,
        &HOGLIN_SENSOR_TYPES,
        hoglin_activities(),
    )
}

/// Vanilla `Zoglin.makeBrain`.
#[must_use]
pub(crate) fn make_zoglin_brain() -> Brain {
    Brain::new(
        &ZOGLIN_MEMORY_TYPES,
        &ZOGLIN_SENSOR_TYPES,
        zoglin_activities(),
    )
}

/// Vanilla `Hoglin.customServerAiStep` brain half.
pub(crate) fn tick_hoglin(brain: &mut Brain, level: &World, body: &SharedEntity) {
    brain.tick(level, body);
}

/// Vanilla `Zoglin.customServerAiStep` brain half.
pub(crate) fn tick_zoglin(brain: &mut Brain, level: &World, body: &SharedEntity) {
    brain.tick(level, body);
}

/// Vanilla `HoglinAi.updateActivity`.
pub(crate) fn update_hoglin_activity(brain: &mut Brain) {
    brain.set_active_activity_to_first_valid(&[Activity::Fight, Activity::Avoid, Activity::Idle]);
}

/// Vanilla `Zoglin.updateActivity`.
pub(crate) fn update_zoglin_activity(brain: &mut Brain) {
    brain.set_active_activity_to_first_valid(&[Activity::Fight, Activity::Idle]);
}

/// Vanilla `BehaviorBuilder.triggerIf(predicate, behavior)` over Steel's
/// [`BehaviorControl`] tree: runs `behavior` once when `predicate` accepts the body.
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

/// Vanilla `HoglinAi.getActivities`.
#[must_use]
fn hoglin_activities() -> Vec<ActivityData> {
    vec![
        hoglin_core_activity(),
        hoglin_idle_activity(),
        hoglin_fight_activity(),
        hoglin_retreat_activity(),
    ]
}

/// Vanilla `Zoglin.getActivities`.
#[must_use]
fn zoglin_activities() -> Vec<ActivityData> {
    vec![
        zoglin_core_activity(),
        zoglin_idle_activity(),
        zoglin_fight_activity(),
    ]
}

/// Vanilla `HoglinAi.initCoreActivity`.
fn hoglin_core_activity() -> ActivityData {
    ActivityData::create_priority(
        Activity::Core,
        0,
        vec![
            BehaviorBuilder::boxed(LookAtTargetSink::new(45, 90)),
            BehaviorBuilder::boxed(MoveToTargetSink::new()),
        ],
    )
}

/// Vanilla `HoglinAi.initIdleActivity`.
fn hoglin_idle_activity() -> ActivityData {
    ActivityData::create_priority(
        Activity::Idle,
        10,
        vec![
            BehaviorBuilder::boxed(BecomePassiveIfMemoryPresent::create(
                &mem::NEAREST_REPELLENT,
                REPELLENT_PACIFY_TIME,
            )),
            BehaviorBuilder::boxed(AnimalMakeLove::with_speed(
                &vanilla_entities::HOGLIN,
                SPEED_MULTIPLIER_WHEN_MAKING_LOVE,
                2,
            )),
            BehaviorBuilder::boxed(SetWalkTargetAwayFrom::pos(
                &mem::NEAREST_REPELLENT,
                SPEED_MULTIPLIER_WHEN_AVOIDING_REPELLENT,
                8,
                true,
            )),
            BehaviorBuilder::boxed(StartAttacking::create(hoglin_nearest_valid_attack_target)),
            BehaviorBuilder::boxed(trigger_if_behavior(
                is_adult,
                SetWalkTargetAwayFrom::entity(
                    &mem::NEAREST_VISIBLE_ADULT_PIGLIN,
                    SPEED_MULTIPLIER_WHEN_IDLING,
                    8,
                    false,
                ),
            )),
            BehaviorBuilder::boxed(SetEntityLookTargetSometimes::create(8.0, LOOK_INTERVAL)),
            BehaviorBuilder::boxed(BabyFollowAdult::create(
                ADULT_FOLLOW_RANGE,
                SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT,
            )),
            BehaviorBuilder::boxed(hoglin_idle_movement_behaviors()),
        ],
    )
}

/// Vanilla `HoglinAi.initFightActivity`.
fn hoglin_fight_activity() -> ActivityData {
    ActivityData::create_full(
        Activity::Fight,
        vec![
            (
                10,
                BehaviorBuilder::boxed(BecomePassiveIfMemoryPresent::create(
                    &mem::NEAREST_REPELLENT,
                    REPELLENT_PACIFY_TIME,
                )),
            ),
            (
                11,
                BehaviorBuilder::boxed(AnimalMakeLove::with_speed(
                    &vanilla_entities::HOGLIN,
                    SPEED_MULTIPLIER_WHEN_MAKING_LOVE,
                    2,
                )),
            ),
            (
                12,
                BehaviorBuilder::boxed(SetWalkTargetFromAttackTargetIfTargetOutOfReach::create(
                    1.0,
                )),
            ),
            (
                13,
                BehaviorBuilder::boxed(trigger_if_behavior(
                    is_adult,
                    MeleeAttack::create(ATTACK_INTERVAL),
                )),
            ),
            (
                14,
                BehaviorBuilder::boxed(trigger_if_behavior(
                    is_baby,
                    MeleeAttack::create(BABY_ATTACK_INTERVAL),
                )),
            ),
            (
                15,
                BehaviorBuilder::boxed(StopAttackingIfTargetInvalid::create()),
            ),
            (
                16,
                BehaviorBuilder::boxed(EraseMemoryIf::create(is_breeding, &mem::ATTACK_TARGET)),
            ),
        ],
        vec![(&mem::ATTACK_TARGET, MemoryStatus::ValuePresent)],
        Vec::new(),
    )
}

/// Vanilla `HoglinAi.initRetreatActivity`.
fn hoglin_retreat_activity() -> ActivityData {
    ActivityData::create_full(
        Activity::Avoid,
        vec![
            (
                10,
                BehaviorBuilder::boxed(SetWalkTargetAwayFrom::entity(
                    &mem::AVOID_TARGET,
                    SPEED_MULTIPLIER_WHEN_RETREATING,
                    15,
                    false,
                )),
            ),
            (11, BehaviorBuilder::boxed(hoglin_idle_movement_behaviors())),
            (
                12,
                BehaviorBuilder::boxed(SetEntityLookTargetSometimes::create(8.0, LOOK_INTERVAL)),
            ),
            (
                13,
                BehaviorBuilder::boxed(EraseMemoryIf::create(
                    hoglin_wants_to_stop_fleeing,
                    &mem::AVOID_TARGET,
                )),
            ),
        ],
        vec![(&mem::AVOID_TARGET, MemoryStatus::ValuePresent)],
        Vec::new(),
    )
}

/// Vanilla `HoglinAi.createIdleMovementBehaviors`.
fn hoglin_idle_movement_behaviors() -> RunOne {
    RunOne::new(vec![
        (
            BehaviorBuilder::boxed(RandomStroll::stroll(SPEED_MULTIPLIER_WHEN_IDLING)),
            2,
        ),
        (
            BehaviorBuilder::boxed(SetWalkTargetFromLookTarget::create(
                SPEED_MULTIPLIER_WHEN_IDLING,
                3,
            )),
            2,
        ),
        (BehaviorBuilder::boxed(DoNothing::new(30, 60)), 1),
    ])
}

/// Vanilla `Zoglin.initCoreActivity`.
fn zoglin_core_activity() -> ActivityData {
    ActivityData::create_priority(
        Activity::Core,
        0,
        vec![
            BehaviorBuilder::boxed(LookAtTargetSink::new(45, 90)),
            BehaviorBuilder::boxed(MoveToTargetSink::new()),
        ],
    )
}

/// Vanilla `Zoglin.initIdleActivity`.
fn zoglin_idle_activity() -> ActivityData {
    ActivityData::create_priority(
        Activity::Idle,
        10,
        vec![
            BehaviorBuilder::boxed(StartAttacking::create(zoglin_nearest_valid_attack_target)),
            BehaviorBuilder::boxed(SetEntityLookTargetSometimes::create(8.0, LOOK_INTERVAL)),
            BehaviorBuilder::boxed(RunOne::new(vec![
                (
                    BehaviorBuilder::boxed(RandomStroll::stroll(
                        ZOGLIN_SPEED_MULTIPLIER_WHEN_IDLING,
                    )),
                    2,
                ),
                (
                    BehaviorBuilder::boxed(SetWalkTargetFromLookTarget::create(
                        ZOGLIN_SPEED_MULTIPLIER_WHEN_IDLING,
                        3,
                    )),
                    2,
                ),
                (BehaviorBuilder::boxed(DoNothing::new(30, 60)), 1),
            ])),
        ],
    )
}

/// Vanilla `Zoglin.initFightActivity`.
fn zoglin_fight_activity() -> ActivityData {
    ActivityData::create_full(
        Activity::Fight,
        vec![
            (
                10,
                BehaviorBuilder::boxed(SetWalkTargetFromAttackTargetIfTargetOutOfReach::create(
                    ZOGLIN_SPEED_MULTIPLIER_WHEN_FIGHTING,
                )),
            ),
            (
                11,
                BehaviorBuilder::boxed(trigger_if_behavior(
                    is_adult,
                    MeleeAttack::create(ZOGLIN_ATTACK_INTERVAL),
                )),
            ),
            (
                12,
                BehaviorBuilder::boxed(trigger_if_behavior(
                    is_baby,
                    MeleeAttack::create(ZOGLIN_BABY_ATTACK_INTERVAL),
                )),
            ),
            (
                13,
                BehaviorBuilder::boxed(StopAttackingIfTargetInvalid::create()),
            ),
        ],
        vec![(&mem::ATTACK_TARGET, MemoryStatus::ValuePresent)],
        Vec::new(),
    )
}

/// Vanilla `HoglinAi.findNearestValidAttackTarget`: only an un-pacified,
/// non-breeding hoglin attacks, and only the nearest visible attackable player.
fn hoglin_nearest_valid_attack_target(
    ctx: &BehaviorContext<'_>,
    _body: &SharedEntity,
) -> Option<SharedEntity> {
    if is_pacified(ctx) || ctx.has_memory_value(&mem::BREED_TARGET) {
        return None;
    }
    ctx.get_memory(&mem::NEAREST_VISIBLE_ATTACKABLE_PLAYER)
}

/// Vanilla `Zoglin.findNearestValidAttackTarget`: the closest visible living
/// entity that is neither a zoglin nor a creeper and is attackable.
fn zoglin_nearest_valid_attack_target(
    ctx: &BehaviorContext<'_>,
    body: &SharedEntity,
) -> Option<SharedEntity> {
    let follow_range = follow_range(body)?;
    let attack_target = ctx.get_memory(&mem::ATTACK_TARGET);
    let visible = ctx.get_memory(&mem::NEAREST_VISIBLE_LIVING_ENTITIES)?;
    visible.find_closest(|entity| {
        entity.entity_type() != &vanilla_entities::ZOGLIN
            && entity.entity_type() != &vanilla_entities::CREEPER
            && is_entity_attackable(
                ctx.level,
                body,
                entity,
                follow_range,
                attack_target.as_ref(),
            )
    })
}

/// Vanilla `HoglinAi.isPacified`.
fn is_pacified(ctx: &BehaviorContext<'_>) -> bool {
    ctx.has_memory_value(&mem::PACIFIED)
}

/// Vanilla `HoglinAi.isBreeding`.
fn is_breeding(ctx: &mut BehaviorContext<'_>) -> bool {
    ctx.has_memory_value(&mem::BREED_TARGET)
}

/// Vanilla `AgeableMob.isAdult`.
fn is_adult(ctx: &mut BehaviorContext<'_>) -> bool {
    !is_baby(ctx)
}

/// Vanilla `AgeableMob.isBaby`.
fn is_baby(ctx: &mut BehaviorContext<'_>) -> bool {
    ctx.body
        .as_living_entity()
        .is_some_and(LivingEntity::is_baby)
}

/// Vanilla `HoglinAi.wantsToStopFleeing`: an adult that no longer faces more
/// piglins than hoglins.
fn hoglin_wants_to_stop_fleeing(ctx: &mut BehaviorContext<'_>) -> bool {
    !is_baby(ctx) && !piglins_outnumber_hoglins(ctx)
}

/// Vanilla `HoglinAi.piglinsOutnumberHoglins`.
fn piglins_outnumber_hoglins(ctx: &mut BehaviorContext<'_>) -> bool {
    if is_baby(ctx) {
        return false;
    }

    let piglin_count = ctx
        .get_memory(&mem::VISIBLE_ADULT_PIGLIN_COUNT)
        .unwrap_or(0);
    let hoglin_count = ctx
        .get_memory(&mem::VISIBLE_ADULT_HOGLIN_COUNT)
        .unwrap_or(0)
        + 1;
    piglin_count > hoglin_count
}
