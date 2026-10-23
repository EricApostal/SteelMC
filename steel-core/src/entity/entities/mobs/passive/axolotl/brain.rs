//! Vanilla `AxolotlAi` + `Axolotl.BRAIN_PROVIDER`.
//!
//! `Axolotl.makeBrain` is a `Brain.Provider` over the five-sensor vanilla list
//! and the four activities from `AxolotlAi.getActivities`;
//! `Axolotl.customServerAiStep` ticks the brain, calls
//! `AxolotlAi.updateActivity` and then mirrors `PLAY_DEAD_TICKS` into the
//! `playingDead` synced flag. Steel keeps that split: [`make_brain`] is the
//! provider, [`tick`] the brain + activity half and [`update_playing_dead`] the
//! synced-flag half the entity tick calls.
//!
//! ## Deviations
//!
//! `SetWalkTargetFromLookTarget` in `AxolotlAi.initIdleActivity` is
//! `create(canSetWalkTargetFromLookTarget, getSpeedModifier, 3)`: a water-parity
//! predicate plus a speed function. Steel's behavior only takes a constant speed
//! modifier and has no predicate hook, so the idle entry uses
//! `SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER` and the predicate is left to the
//! water/ground `triggerIf` entries next to it.

use steel_registry::vanilla_entities;
use steel_registry::vanilla_fluid_tags::FluidTag;
use steel_utils::value_providers::UniformIntProvider;

use crate::entity::SharedEntity;
use crate::entity::ai::behavior::animal_make_love::AnimalMakeLove;
use crate::entity::ai::behavior::baby_follow_adult::BabyFollowAdult;
use crate::entity::ai::behavior::count_down_cooldown_ticks::CountDownCooldownTicks;
use crate::entity::ai::behavior::erase_memory_if::EraseMemoryIf;
use crate::entity::ai::behavior::follow_temptation::FollowTemptation;
use crate::entity::ai::behavior::look_at_target_sink::LookAtTargetSink;
use crate::entity::ai::behavior::melee_attack::MeleeAttack;
use crate::entity::ai::behavior::move_to_target_sink::MoveToTargetSink;
use crate::entity::ai::behavior::random_stroll::RandomStroll;
use crate::entity::ai::behavior::set_entity_look_target_sometimes::SetEntityLookTargetSometimes;
use crate::entity::ai::behavior::set_walk_target_from_attack_target_if_target_out_of_reach::SetWalkTargetFromAttackTargetIfTargetOutOfReach;
use crate::entity::ai::behavior::set_walk_target_from_look_target::SetWalkTargetFromLookTarget;
use crate::entity::ai::behavior::start_attacking::StartAttacking;
use crate::entity::ai::behavior::stop_attacking_if_target_invalid::StopAttackingIfTargetInvalid;
use crate::entity::ai::behavior::try_find_liquid::TryFindLiquid;
use crate::entity::ai::brain::SensorType;
use crate::entity::ai::brain::activity::Activity;
use crate::entity::ai::brain::behavior::builder::BehaviorBuilder;
use crate::entity::ai::brain::behavior::gate::{GateBehavior, OrderPolicy, RunOne, RunningPolicy};
use crate::entity::ai::brain::behavior::{BehaviorContext, TriggerIf};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::{ActivityData, Brain};
use crate::entity::ai::sensing;
use crate::entity::ai::sensing::villager as villager_sensors;
use crate::entity::entities::mobs::passive::axolotl::{AxolotlEntity, PlayDead, ValidatePlayDead};
use crate::world::World;

/// Vanilla `AxolotlAi.ADULT_FOLLOW_RANGE`.
const ADULT_FOLLOW_RANGE: UniformIntProvider = UniformIntProvider {
    min_inclusive: 5,
    max_inclusive: 16,
};
/// Vanilla `AxolotlAi.SPEED_MULTIPLIER_WHEN_MAKING_LOVE`.
const SPEED_MULTIPLIER_WHEN_MAKING_LOVE: f32 = 0.2;
/// Vanilla `AxolotlAi.SPEED_MULTIPLIER_ON_LAND`.
const SPEED_MULTIPLIER_ON_LAND: f32 = 0.15;
/// Vanilla `AxolotlAi.SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER`.
const SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER: f32 = 0.5;
/// Vanilla `AxolotlAi.SPEED_MULTIPLIER_WHEN_CHASING_IN_WATER`.
const SPEED_MULTIPLIER_WHEN_CHASING_IN_WATER: f32 = 0.6;
/// Vanilla `AxolotlAi.SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT_IN_WATER`.
const SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT_IN_WATER: f32 = 0.6;
/// Vanilla `MeleeAttack.create(20)` cooldown.
const ATTACK_COOLDOWN: i32 = 20;
/// Vanilla `AxolotlAi.updateActivity` hunting cooldown, `2400L`.
const HAS_HUNTING_COOLDOWN_TICKS: i64 = 2400;
/// Vanilla `SetEntityLookTargetSometimes.create(EntityTypes.PLAYER, 6.0F,
/// UniformInt.of(30, 60))` arguments.
const LOOK_DISTANCE: f32 = 6.0;
/// Vanilla look interval, `UniformInt.of(30, 60)`.
const LOOK_INTERVAL: UniformIntProvider = UniformIntProvider {
    min_inclusive: 30,
    max_inclusive: 60,
};

/// Vanilla `Axolotl.BRAIN_PROVIDER` sensor list.
static SENSOR_TYPES: [&SensorType; 5] = [
    &villager_sensors::NEAREST_LIVING_ENTITIES,
    &sensing::adult::NEAREST_ADULT,
    &villager_sensors::HURT_BY,
    &sensing::axolotl_attackables::AXOLOTL_ATTACKABLES,
    &sensing::tempting::FOOD_TEMPTATIONS,
];

/// Vanilla `Axolotl.BRAIN_PROVIDER.makeBrain`.
///
/// The provider reads no entity state (its sensor and activity lists are
/// static), so the parameter only fixes the call shape the axolotl's own
/// `make_brain` uses.
#[must_use]
pub(crate) fn make_brain(_axolotl: &AxolotlEntity) -> Brain {
    Brain::new(&[], &SENSOR_TYPES, activities())
}

/// Vanilla `AxolotlAi.getActivities`.
#[must_use]
fn activities() -> Vec<ActivityData> {
    vec![
        core_activity(),
        idle_activity(),
        fight_activity(),
        play_dead_activity(),
    ]
}

/// Vanilla `AxolotlAi.initCoreActivity`.
fn core_activity() -> ActivityData {
    ActivityData::create_priority(
        Activity::Core,
        0,
        vec![
            BehaviorBuilder::boxed(LookAtTargetSink::new(45, 90)),
            BehaviorBuilder::boxed(MoveToTargetSink::new()),
            BehaviorBuilder::boxed(ValidatePlayDead::create()),
            BehaviorBuilder::boxed(CountDownCooldownTicks::new(&mem::TEMPTATION_COOLDOWN_TICKS)),
        ],
    )
}

/// Vanilla `AxolotlAi.initIdleActivity`.
fn idle_activity() -> ActivityData {
    ActivityData::create_priority(
        Activity::Idle,
        0,
        vec![
            BehaviorBuilder::boxed(SetEntityLookTargetSometimes::for_type(
                &vanilla_entities::PLAYER,
                LOOK_DISTANCE,
                LOOK_INTERVAL,
            )),
            BehaviorBuilder::boxed(AnimalMakeLove::with_speed(
                &vanilla_entities::AXOLOTL,
                SPEED_MULTIPLIER_WHEN_MAKING_LOVE,
                2,
            )),
            BehaviorBuilder::boxed(RunOne::new(vec![
                (
                    BehaviorBuilder::boxed(FollowTemptation::with_speed_fn(speed_modifier)),
                    1,
                ),
                (
                    BehaviorBuilder::boxed(BabyFollowAdult::create_with_speed_fn(
                        ADULT_FOLLOW_RANGE,
                        speed_modifier_following_adult,
                        &mem::NEAREST_VISIBLE_ADULT,
                        false,
                    )),
                    1,
                ),
            ])),
            BehaviorBuilder::boxed(StartAttacking::create(nearest_valid_attack_target)),
            BehaviorBuilder::boxed(TryFindLiquid::create(
                6,
                SPEED_MULTIPLIER_ON_LAND,
                FluidTag::AXOLOTL_TRIES_TO_FIND,
            )),
            BehaviorBuilder::boxed(GateBehavior::new(
                vec![(&mem::WALK_TARGET, MemoryStatus::ValueAbsent)],
                Vec::new(),
                OrderPolicy::Ordered,
                RunningPolicy::TryAll,
                vec![
                    (
                        BehaviorBuilder::boxed(RandomStroll::swim(
                            SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER,
                        )),
                        2,
                    ),
                    (
                        BehaviorBuilder::boxed(RandomStroll::stroll_with_restriction(
                            SPEED_MULTIPLIER_ON_LAND,
                            false,
                        )),
                        2,
                    ),
                    (
                        BehaviorBuilder::boxed(SetWalkTargetFromLookTarget::create(
                            SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER,
                            3,
                        )),
                        3,
                    ),
                    (
                        BehaviorBuilder::boxed(BehaviorBuilder::create(TriggerIf::new(
                            |ctx: &mut BehaviorContext<'_>| ctx.body.is_in_water(),
                        ))),
                        5,
                    ),
                    (
                        BehaviorBuilder::boxed(BehaviorBuilder::create(TriggerIf::new(
                            |ctx: &mut BehaviorContext<'_>| ctx.body.on_ground(),
                        ))),
                        5,
                    ),
                ],
            )),
        ],
    )
}

/// Vanilla `AxolotlAi.initFightActivity`: entering it requires `ATTACK_TARGET`,
/// and leaving it erases that memory.
fn fight_activity() -> ActivityData {
    ActivityData::create_with_erase(
        Activity::Fight,
        0,
        vec![
            BehaviorBuilder::boxed(StopAttackingIfTargetInvalid::create()),
            BehaviorBuilder::boxed(
                SetWalkTargetFromAttackTargetIfTargetOutOfReach::create_with_speed_fn(
                    speed_modifier_chasing,
                ),
            ),
            BehaviorBuilder::boxed(MeleeAttack::create(ATTACK_COOLDOWN)),
            BehaviorBuilder::boxed(EraseMemoryIf::create(is_breeding_ctx, &mem::ATTACK_TARGET)),
        ],
        &mem::ATTACK_TARGET,
    )
}

/// Vanilla `AxolotlAi.initPlayDeadActivity`: entering it requires
/// `PLAY_DEAD_TICKS`, and leaving it erases that memory.
fn play_dead_activity() -> ActivityData {
    ActivityData::create_full(
        Activity::PlayDead,
        vec![
            (0, BehaviorBuilder::boxed(PlayDead::new())),
            (
                1,
                BehaviorBuilder::boxed(EraseMemoryIf::create(
                    is_breeding_ctx,
                    &mem::PLAY_DEAD_TICKS,
                )),
            ),
        ],
        vec![(&mem::PLAY_DEAD_TICKS, MemoryStatus::ValuePresent)],
        vec![&mem::PLAY_DEAD_TICKS],
    )
}

/// Vanilla `Axolotl.customServerAiStep` brain half.
pub(crate) fn tick(brain: &mut Brain, level: &World, body: &SharedEntity) {
    brain.tick(level, body);
    update_activity(brain, body);
}

/// Vanilla `AxolotlAi.updateActivity`.
///
/// A play-dead axolotl keeps its activity; otherwise the first valid of
/// `PLAY_DEAD`, `FIGHT`, `IDLE` wins, and leaving `FIGHT` arms the
/// `HAS_HUNTING_COOLDOWN` memory that `AxolotlAttackablesSensor` reads.
pub(crate) fn update_activity(brain: &mut Brain, _body: &SharedEntity) {
    let old_activity = brain.get_active_non_core_activity();

    if old_activity == Some(Activity::PlayDead) {
        return;
    }

    brain.set_active_activity_to_first_valid(&[
        Activity::PlayDead,
        Activity::Fight,
        Activity::Idle,
    ]);

    if old_activity == Some(Activity::Fight)
        && brain.get_active_non_core_activity() != Some(Activity::Fight)
    {
        brain.set_memory_with_expiry(&mem::HAS_HUNTING_COOLDOWN, true, HAS_HUNTING_COOLDOWN_TICKS);
    }
}

/// Vanilla `Axolotl.customServerAiStep` tail:
/// `setPlayingDead(playDeadTicks.isPresent() && playDeadTicks > 0)`.
pub(crate) fn update_playing_dead(brain: &Brain, axolotl: &AxolotlEntity) {
    let playing_dead = brain
        .get_memory(&mem::PLAY_DEAD_TICKS)
        .is_some_and(|ticks| ticks > 0);
    axolotl.set_playing_dead(playing_dead);
}

/// Vanilla `AxolotlAi.findNearestValidAttackTarget`: nothing while breeding,
/// otherwise the sensor's `NEAREST_ATTACKABLE`.
fn nearest_valid_attack_target(
    ctx: &BehaviorContext<'_>,
    _body: &SharedEntity,
) -> Option<SharedEntity> {
    if is_breeding(ctx) {
        return None;
    }

    ctx.get_memory(&mem::NEAREST_ATTACKABLE)
}

/// Vanilla `BehaviorUtils.isBreeding`: the body has a `BREED_TARGET`.
fn is_breeding(ctx: &BehaviorContext<'_>) -> bool {
    ctx.has_memory_value(&mem::BREED_TARGET)
}

/// [`is_breeding`] adapted to the mutable context signature of [`EraseMemoryIf`].
fn is_breeding_ctx(ctx: &mut BehaviorContext<'_>) -> bool {
    is_breeding(ctx)
}

/// Vanilla `AxolotlAi.getSpeedModifierChasing`.
fn speed_modifier_chasing(body: &SharedEntity) -> f32 {
    if body.is_in_water() {
        SPEED_MULTIPLIER_WHEN_CHASING_IN_WATER
    } else {
        SPEED_MULTIPLIER_ON_LAND
    }
}

/// Vanilla `AxolotlAi.getSpeedModifierFollowingAdult`.
fn speed_modifier_following_adult(body: &SharedEntity) -> f32 {
    if body.is_in_water() {
        SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT_IN_WATER
    } else {
        SPEED_MULTIPLIER_ON_LAND
    }
}

/// Vanilla `AxolotlAi.getSpeedModifier`.
fn speed_modifier(body: &SharedEntity) -> f32 {
    if body.is_in_water() {
        SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER
    } else {
        SPEED_MULTIPLIER_ON_LAND
    }
}
