//! Vanilla `WardenAi`.
//!
//! `Warden.makeBrain` is `BRAIN_PROVIDER` (four memories, two sensors) plus
//! `WardenAi.getActivities`; `Warden.customServerAiStep` ticks the brain and then
//! calls `WardenAi.updateActivity`. Steel keeps the same shape as free functions:
//! [`make_brain`] builds the provider, [`tick`] is the `customServerAiStep` brain
//! half and [`update_activity`] is `WardenAi.updateActivity`.
//!
//! ## Vanilla state with no brain-agnostic counterpart
//!
//! Vanilla reads the warden's anger through `Warden.getEntityAngryAt()`,
//! `getAngerLevel()` and `canTargetEntity()`, all *entity* accessors, and its
//! `FIGHT` predicates close over the warden instance. Steel brains only see
//! `&SharedEntity` plus the memory store, so the same three tests are expressed
//! over memories and the shared targeting conditions:
//!
//! - angry-at target / "is angry": the `ANGRY_AT` (and `UNIVERSAL_ANGER`) brain
//!   memories, which the warden entity mirrors its anger into.
//! - `canTargetEntity`: [`is_entity_attackable`], the same
//!   `TargetingConditions.forCombat()` test the sensors use.
//!
//! `set_disturbance_location` drops vanilla's world-border guard: it needs the
//! level, which this brain-only signature does not have, so the vibration
//! callback that owns the level applies it before calling.

use std::sync::Arc;

use steel_utils::BlockPos;

use crate::entity::SharedEntity;
use crate::entity::ai::behavior::do_nothing::DoNothing;
use crate::entity::ai::behavior::go_to_target_location::GoToTargetLocation;
use crate::entity::ai::behavior::look_at_target_sink::LookAtTargetSink;
use crate::entity::ai::behavior::melee_attack::MeleeAttack;
use crate::entity::ai::behavior::move_to_target_sink::MoveToTargetSink;
use crate::entity::ai::behavior::random_stroll::RandomStroll;
use crate::entity::ai::behavior::set_walk_target_from_attack_target_if_target_out_of_reach::SetWalkTargetFromAttackTargetIfTargetOutOfReach;
use crate::entity::ai::behavior::stop_attacking_if_target_invalid::StopAttackingIfTargetInvalid;
use crate::entity::ai::behavior::swim::Swim;
use crate::entity::ai::behavior::warden;
use crate::entity::ai::brain::SensorType;
use crate::entity::ai::brain::activity::Activity;
use crate::entity::ai::brain::behavior::builder::BehaviorBuilder;
use crate::entity::ai::brain::behavior::gate::RunOne;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStatus, module_type as mem};
use crate::entity::ai::brain::tracker::{BlockPosTracker, EntityTracker, PositionTracker};
use crate::entity::ai::brain::{ActivityData, Brain};
use crate::entity::ai::sensing::villager::NEAREST_PLAYERS;
use crate::entity::ai::sensing::villager::{follow_range, is_entity_attackable};
use crate::entity::ai::sensing::warden_entity::WARDEN_ENTITY_SENSOR;
use crate::world::World;

/// Vanilla `WardenAi.SPEED_MULTIPLIER_WHEN_IDLING`.
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 0.5;
/// Vanilla `WardenAi.SPEED_MULTIPLIER_WHEN_INVESTIGATING`.
const SPEED_MULTIPLIER_WHEN_INVESTIGATING: f32 = 0.7;
/// Vanilla `WardenAi.SPEED_MULTIPLIER_WHEN_FIGHTING`.
const SPEED_MULTIPLIER_WHEN_FIGHTING: f32 = 1.2;
/// Vanilla `WardenAi.MELEE_ATTACK_COOLDOWN`.
const MELEE_ATTACK_COOLDOWN: i32 = 18;

/// Vanilla `Warden` `Attributes.FOLLOW_RANGE` base value, used by the `FIGHT`
/// look-target entry.
///
/// Vanilla reads `getAttributeValue(FOLLOW_RANGE)` once, while
/// `BRAIN_PROVIDER.makeBrain` builds the activities. [`make_brain`] takes no body,
/// so the vanilla base value stands in for that construction-time read.
const FOLLOW_RANGE: f32 = 24.0;

/// Vanilla `Warden.BRAIN_PROVIDER` memory list.
static MEMORY_TYPES: [MemoryModuleTypeRef; 6] = [
    &mem::NEAREST_VISIBLE_NEMESIS,
    &mem::RECENT_PROJECTILE,
    &mem::TOUCH_COOLDOWN,
    &mem::VIBRATION_COOLDOWN,
    // Vanilla keeps anger on the entity (`Warden.getEntityAngryAt`); Steel mirrors
    // it into the brain, so the two keys the warden's anger predicates read must be
    // registered here or `MemoryStore::set` silently drops them.
    &mem::ANGRY_AT,
    &mem::UNIVERSAL_ANGER,
];

/// Vanilla `Warden.BRAIN_PROVIDER` sensor list.
///
/// `WARDEN_ENTITY_SENSOR` is a `NearestLivingEntitySensor`, narrowed by its
/// `requires()` to `NEAREST_VISIBLE_NEMESIS`; `NEAREST_PLAYERS` supplies
/// `NEAREST_VISIBLE_ATTACKABLE_PLAYER`.
static SENSOR_TYPES: [&SensorType; 2] = [&NEAREST_PLAYERS, &WARDEN_ENTITY_SENSOR];

/// Vanilla `Warden.makeBrain`.
#[must_use]
pub(crate) fn make_brain() -> Brain {
    Brain::new(&MEMORY_TYPES, &SENSOR_TYPES, activities())
}

/// Vanilla `WardenAi.getActivities`.
#[must_use]
fn activities() -> Vec<ActivityData> {
    vec![
        core_activity(),
        emerge_activity(),
        digging_activity(),
        idle_activity(),
        roar_activity(),
        fight_activity(),
        investigate_activity(),
        sniffing_activity(),
    ]
}

/// Vanilla `Warden.customServerAiStep` brain half.
pub(crate) fn tick(brain: &mut Brain, level: &World, body: &SharedEntity) {
    brain.tick(level, body);
}

/// Vanilla `WardenAi.updateActivity`.
pub(crate) fn update_activity(brain: &mut Brain) {
    brain.set_active_activity_to_first_valid(&[
        Activity::Emerge,
        Activity::Dig,
        Activity::Roar,
        Activity::Fight,
        Activity::Investigate,
        Activity::Sniff,
        Activity::Idle,
    ]);
}

/// Vanilla `WardenAi.initCoreActivity`.
fn core_activity() -> ActivityData {
    ActivityData::create_priority(
        Activity::Core,
        0,
        vec![
            BehaviorBuilder::boxed(Swim::new(0.8)),
            BehaviorBuilder::boxed(warden::set_warden_look_target::SetWardenLookTarget::create()),
            BehaviorBuilder::boxed(LookAtTargetSink::new(45, 90)),
            BehaviorBuilder::boxed(MoveToTargetSink::new()),
        ],
    )
}

/// Vanilla `WardenAi.initEmergeActivity`.
fn emerge_activity() -> ActivityData {
    ActivityData::create_with_erase(
        Activity::Emerge,
        5,
        vec![BehaviorBuilder::boxed(warden::emerging::Emerging::new(
            warden::EMERGE_DURATION,
        ))],
        &mem::IS_EMERGING,
    )
}

/// Vanilla `WardenAi.initDiggingActivity`.
fn digging_activity() -> ActivityData {
    ActivityData::create_full(
        Activity::Dig,
        vec![
            (
                0,
                BehaviorBuilder::boxed(warden::force_unmount::ForceUnmount::new()),
            ),
            (
                1,
                BehaviorBuilder::boxed(warden::digging::Digging::new(warden::DIGGING_DURATION)),
            ),
        ],
        vec![
            (&mem::ROAR_TARGET, MemoryStatus::ValueAbsent),
            (&mem::DIG_COOLDOWN, MemoryStatus::ValueAbsent),
        ],
        Vec::new(),
    )
}

/// Vanilla `WardenAi.initIdleActivity`.
fn idle_activity() -> ActivityData {
    ActivityData::create_priority(
        Activity::Idle,
        10,
        vec![
            BehaviorBuilder::boxed(warden::set_roar_target::SetRoarTarget::create(angry_at)),
            BehaviorBuilder::boxed(warden::try_to_sniff::TryToSniff::create()),
            BehaviorBuilder::boxed(RunOne::with_entry_condition(
                vec![(&mem::IS_SNIFFING, MemoryStatus::ValueAbsent)],
                vec![
                    (
                        BehaviorBuilder::boxed(RandomStroll::stroll(SPEED_MULTIPLIER_WHEN_IDLING)),
                        2,
                    ),
                    (BehaviorBuilder::boxed(DoNothing::new(30, 60)), 1),
                ],
            )),
        ],
    )
}

/// Vanilla `WardenAi.initRoarActivity`.
fn roar_activity() -> ActivityData {
    ActivityData::create_with_erase(
        Activity::Roar,
        10,
        vec![BehaviorBuilder::boxed(warden::roar::Roar::new())],
        &mem::ROAR_TARGET,
    )
}

/// Vanilla `WardenAi.initFightActivity`.
fn fight_activity() -> ActivityData {
    ActivityData::create_full(
        Activity::Fight,
        vec![
            (0, BehaviorBuilder::boxed(dig_cooldown_setter())),
            (
                1,
                BehaviorBuilder::boxed(StopAttackingIfTargetInvalid::create_full(
                    stop_attacking_when,
                    on_target_invalid,
                    false,
                )),
            ),
            (2, BehaviorBuilder::boxed(set_attack_target_look_target())),
            (
                3,
                BehaviorBuilder::boxed(SetWalkTargetFromAttackTargetIfTargetOutOfReach::create(
                    SPEED_MULTIPLIER_WHEN_FIGHTING,
                )),
            ),
            (
                4,
                BehaviorBuilder::boxed(warden::sonic_boom::SonicBoom::new()),
            ),
            (
                5,
                BehaviorBuilder::boxed(MeleeAttack::create(MELEE_ATTACK_COOLDOWN)),
            ),
        ],
        vec![(&mem::ATTACK_TARGET, MemoryStatus::ValuePresent)],
        Vec::new(),
    )
}

/// Vanilla `WardenAi.initInvestigateActivity`.
fn investigate_activity() -> ActivityData {
    ActivityData::create_with_erase(
        Activity::Investigate,
        5,
        vec![
            BehaviorBuilder::boxed(warden::set_roar_target::SetRoarTarget::create(angry_at)),
            BehaviorBuilder::boxed(GoToTargetLocation::create(
                &mem::DISTURBANCE_LOCATION,
                2,
                SPEED_MULTIPLIER_WHEN_INVESTIGATING,
            )),
        ],
        &mem::DISTURBANCE_LOCATION,
    )
}

/// Vanilla `WardenAi.initSniffingActivity`.
fn sniffing_activity() -> ActivityData {
    ActivityData::create_with_erase(
        Activity::Sniff,
        5,
        vec![
            BehaviorBuilder::boxed(warden::set_roar_target::SetRoarTarget::create(angry_at)),
            BehaviorBuilder::boxed(warden::sniffing::Sniffing::new(warden::SNIFFING_DURATION)),
        ],
        &mem::IS_SNIFFING,
    )
}

/// Vanilla `WardenAi.setDigCooldown`.
pub(crate) fn set_dig_cooldown(brain: &mut Brain) {
    if brain.has_memory_value(&mem::DIG_COOLDOWN) {
        brain.set_memory_with_expiry(&mem::DIG_COOLDOWN, (), warden::DIGGING_COOLDOWN);
    }
}

/// Vanilla `WardenAi.setDisturbanceLocation`.
///
/// The caller owns the level, so vanilla's
/// `level().getWorldBorder().isWithinBounds(position)` guard stays at the call
/// site; `entity_angry_at` is `Warden.getEntityAngryAt().isPresent()`.
pub(crate) fn set_disturbance_location(
    brain: &mut Brain,
    position: BlockPos,
    entity_angry_at: bool,
) {
    if entity_angry_at || brain.has_memory_value(&mem::ATTACK_TARGET) {
        return;
    }

    set_dig_cooldown(brain);
    brain.set_memory_with_expiry(
        &mem::SNIFF_COOLDOWN,
        (),
        warden::DISTURBANCE_LOCATION_EXPIRY,
    );
    brain.set_memory_with_expiry(
        &mem::LOOK_TARGET,
        Arc::new(BlockPosTracker::from_block_pos(position)) as Arc<dyn PositionTracker>,
        warden::DISTURBANCE_LOCATION_EXPIRY,
    );
    brain.set_memory_with_expiry(
        &mem::DISTURBANCE_LOCATION,
        position,
        warden::DISTURBANCE_LOCATION_EXPIRY,
    );
    brain.erase_memory(&mem::WALK_TARGET);
}

/// Vanilla `WardenAi.DIG_COOLDOWN_SETTER`.
///
/// Vanilla builds it with `BehaviorBuilder`; Steel expresses the same
/// "refresh the cooldown if the memory is registered" rule as a [`Trigger`].
struct DigCooldownSetter;

impl Trigger for DigCooldownSetter {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if ctx.has_memory_value(&mem::DIG_COOLDOWN) {
            ctx.set_memory_with_expiry(&mem::DIG_COOLDOWN, (), warden::DIGGING_COOLDOWN);
        }
        true
    }
}

/// Vanilla `WardenAi.DIG_COOLDOWN_SETTER`.
fn dig_cooldown_setter() -> OneShot<DigCooldownSetter> {
    OneShot::new(DigCooldownSetter).with_required_memories(vec![&mem::DIG_COOLDOWN])
}

/// Vanilla `SetEntityLookTarget.create(entity -> isTarget(body, entity), followRange)`.
///
/// Vanilla's predicate reads the body's brain; Steel behaviors may not re-enter the
/// brain lock, so the same rule is expressed as a [`Trigger`] over the memories:
/// look at the current `ATTACK_TARGET` while no look target is set.
struct SetAttackTargetLookTarget;

impl Trigger for SetAttackTargetLookTarget {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if ctx.has_memory_value(&mem::LOOK_TARGET) {
            return false;
        }

        let Some(target) = ctx.get_memory(&mem::ATTACK_TARGET) else {
            return false;
        };

        if target.distance_to_sqr(ctx.body.position()) > f64::from(FOLLOW_RANGE).powi(2) {
            return false;
        }

        crate::entity::ai::behavior::utils::set_look_target(
            ctx,
            Arc::new(EntityTracker::new(target, true)),
        );
        true
    }
}

/// Vanilla `SetEntityLookTarget.create(entity -> isTarget(body, entity), followRange)`.
fn set_attack_target_look_target() -> OneShot<SetAttackTargetLookTarget> {
    OneShot::new(SetAttackTargetLookTarget)
        .with_required_memories(vec![&mem::LOOK_TARGET, &mem::ATTACK_TARGET])
}

/// Vanilla `Warden::getEntityAngryAt`: the `ANGRY_AT` UUID resolved to a living
/// entity. Used as vanilla's `SetRoarTarget` finder.
///
/// Reads `ANGRY_AT` from the borrowed memory store rather than the body's brain:
/// the brain lock is held for the whole tick, so re-locking it here would
/// self-deadlock (`parking_lot::Mutex` is not reentrant).
fn angry_at(ctx: &BehaviorContext<'_>, _body: &SharedEntity) -> Option<SharedEntity> {
    let uuid = ctx.get_memory(&mem::ANGRY_AT)?;
    let target = ctx.level.get_entity_by_uuid(&uuid)?;
    target.as_living_entity()?;
    Some(target)
}

/// Vanilla `Warden.getAngerLevel().isAngry()` over the brain memories the warden
/// mirrors its anger into.
fn is_angry(ctx: &BehaviorContext<'_>) -> bool {
    ctx.has_memory_value(&mem::ANGRY_AT) || ctx.has_memory_value(&mem::UNIVERSAL_ANGER)
}

/// Vanilla `WardenAi`: `!body.getAngerLevel().isAngry() || !body.canTargetEntity(target)`.
fn stop_attacking_when(ctx: &BehaviorContext<'_>, target: &SharedEntity) -> bool {
    !is_angry(ctx) || !can_target_entity(ctx, target)
}

/// Vanilla `WardenAi.onTargetInvalid`: `clearAnger(target)` plus `setDigCooldown`.
///
/// Dropping the anger itself (the entity's `AngerManagement`) stays with the
/// entity; the cooldown is the brain half.
fn on_target_invalid(ctx: &mut BehaviorContext<'_>, _target: &SharedEntity) {
    if ctx.has_memory_value(&mem::DIG_COOLDOWN) {
        ctx.set_memory_with_expiry(&mem::DIG_COOLDOWN, (), warden::DIGGING_COOLDOWN);
    }
}

/// Vanilla `Warden.canTargetEntity`, narrowed to what a brain can see.
fn can_target_entity(ctx: &BehaviorContext<'_>, target: &SharedEntity) -> bool {
    let Some(follow_range) = follow_range(ctx.body) else {
        return false;
    };
    let attack_target = ctx.get_memory(&mem::ATTACK_TARGET);
    is_entity_attackable(
        ctx.level,
        ctx.body,
        target,
        follow_range,
        attack_target.as_ref(),
    )
}
