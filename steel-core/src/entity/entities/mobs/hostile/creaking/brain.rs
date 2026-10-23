//! Vanilla `CreakingAi`.
//!
//! `Creaking.makeBrain` is a two-sensor provider with no extra memories;
//! `Creaking.customServerAiStep` ticks the brain and then calls
//! `CreakingAi.updateActivity`. Steel keeps that split: [`make_brain`] is the
//! provider, [`tick`] the brain half and [`update_activity`] the activity switch.
//!
//! ## Vanilla state with no brain-agnostic counterpart
//!
//! `Creaking.canMove()` and `Creaking.isActive()` are synced entity data, and the
//! `FIGHT`/`IDLE` packages gate on them. The brain functions stay entity-agnostic
//! (`&SharedEntity`/`&World`) and reach those two flags through
//! [`CreakingEntity`]'s public accessors, exactly like `CrossbowAttack` reaches
//! `PillagerEntity`. The one gate that cannot be expressed this way is
//! `Swim`'s `checkExtraStartConditions`: `Swim` owns its start condition and has
//! no predicate hook, so the `canMove` half of it is left to the creaking's own
//! movement controls, which freeze the body while `canMove` is false.

use steel_utils::Downcast as _;
use steel_utils::value_providers::UniformIntProvider;

use crate::entity::SharedEntity;
use crate::entity::ai::behavior::do_nothing::DoNothing;
use crate::entity::ai::behavior::look_at_target_sink::LookAtTargetSink;
use crate::entity::ai::behavior::melee_attack::MeleeAttack;
use crate::entity::ai::behavior::move_to_target_sink::MoveToTargetSink;
use crate::entity::ai::behavior::random_stroll::RandomStroll;
use crate::entity::ai::behavior::set_entity_look_target_sometimes::SetEntityLookTargetSometimes;
use crate::entity::ai::behavior::set_walk_target_from_attack_target_if_target_out_of_reach::SetWalkTargetFromAttackTargetIfTargetOutOfReach;
use crate::entity::ai::behavior::set_walk_target_from_look_target::SetWalkTargetFromLookTarget;
use crate::entity::ai::behavior::start_attacking::StartAttacking;
use crate::entity::ai::behavior::stop_attacking_if_target_invalid::StopAttackingIfTargetInvalid;
use crate::entity::ai::behavior::swim::Swim;
use crate::entity::ai::brain::SensorType;
use crate::entity::ai::brain::activity::Activity;
use crate::entity::ai::brain::behavior::BehaviorContext;
use crate::entity::ai::brain::behavior::builder::BehaviorBuilder;
use crate::entity::ai::brain::behavior::gate::RunOne;
use crate::entity::ai::brain::memory::{MemoryStatus, module_type as mem};
use crate::entity::ai::brain::{ActivityData, Brain};
use crate::entity::ai::sensing::villager::{NEAREST_LIVING_ENTITIES, NEAREST_PLAYERS};
use crate::entity::entities::mobs::hostile::creaking::CreakingEntity;
use crate::world::World;

/// Vanilla `Creaking.SPEED_MULTIPLIER_WHEN_IDLING`.
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 0.3;
/// Vanilla `Creaking.MOVEMENT_SPEED_WHEN_FIGHTING`.
const SPEED_MULTIPLIER_WHEN_FIGHTING: f32 = 0.4;
/// Vanilla `Creaking.ATTACK_INTERVAL`.
const ATTACK_INTERVAL: i32 = 40;
/// Vanilla `SetEntityLookTargetSometimes` interval in the `IDLE` package,
/// `UniformInt.of(30, 60)`.
const LOOK_INTERVAL: UniformIntProvider = UniformIntProvider {
    min_inclusive: 30,
    max_inclusive: 60,
};

/// Vanilla `Creaking.BRAIN_PROVIDER` sensor list.
static SENSOR_TYPES: [&SensorType; 2] = [&NEAREST_LIVING_ENTITIES, &NEAREST_PLAYERS];

/// Vanilla `Creaking.makeBrain`.
#[must_use]
pub(crate) fn make_brain() -> Brain {
    Brain::new(&[], &SENSOR_TYPES, activities())
}

/// Vanilla `CreakingAi.getActivities`.
#[must_use]
fn activities() -> Vec<ActivityData> {
    vec![core_activity(), idle_activity(), fight_activity()]
}

/// Vanilla `Creaking.customServerAiStep` brain half.
pub(crate) fn tick(brain: &mut Brain, level: &World, body: &SharedEntity) {
    brain.tick(level, body);
}

/// Vanilla `CreakingAi.updateActivity`: a creaking that cannot move uses the
/// default activity instead of fighting or idling.
pub(crate) fn update_activity(brain: &mut Brain, can_move: bool) {
    if can_move {
        brain.set_active_activity_to_first_valid(&[Activity::Fight, Activity::Idle]);
    } else {
        brain.use_default_activity();
    }
}

/// Vanilla `CreakingAi.initCoreActivity`.
fn core_activity() -> ActivityData {
    ActivityData::create_priority(
        Activity::Core,
        0,
        vec![
            BehaviorBuilder::boxed(Swim::new(0.8)),
            BehaviorBuilder::boxed(LookAtTargetSink::new(45, 90)),
            BehaviorBuilder::boxed(MoveToTargetSink::new()),
        ],
    )
}

/// Vanilla `CreakingAi.initIdleActivity`.
fn idle_activity() -> ActivityData {
    ActivityData::create_priority(
        Activity::Idle,
        10,
        vec![
            BehaviorBuilder::boxed(StartAttacking::create_if(
                is_active,
                nearest_visible_attackable_player,
            )),
            BehaviorBuilder::boxed(SetEntityLookTargetSometimes::create(8.0, LOOK_INTERVAL)),
            BehaviorBuilder::boxed(RunOne::new(vec![
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
            ])),
        ],
    )
}

/// Vanilla `CreakingAi.initFightActivity`.
fn fight_activity() -> ActivityData {
    ActivityData::create_full(
        Activity::Fight,
        vec![
            (
                10,
                BehaviorBuilder::boxed(SetWalkTargetFromAttackTargetIfTargetOutOfReach::create(
                    SPEED_MULTIPLIER_WHEN_FIGHTING,
                )),
            ),
            (
                11,
                BehaviorBuilder::boxed(MeleeAttack::create_with_predicate(
                    can_move,
                    ATTACK_INTERVAL,
                )),
            ),
            (
                12,
                BehaviorBuilder::boxed(StopAttackingIfTargetInvalid::create_with(
                    is_attack_target_still_reachable,
                )),
            ),
        ],
        vec![(&mem::ATTACK_TARGET, MemoryStatus::ValuePresent)],
        Vec::new(),
    )
}

/// Vanilla `Creaking::isActive`.
fn is_active(_ctx: &BehaviorContext<'_>, body: &SharedEntity) -> bool {
    body.as_ref()
        .downcast_ref::<CreakingEntity>()
        .is_some_and(CreakingEntity::is_active)
}

/// Vanilla `Creaking::canMove`.
fn can_move(body: &SharedEntity) -> bool {
    body.as_ref()
        .downcast_ref::<CreakingEntity>()
        .is_some_and(CreakingEntity::can_move)
}

/// Vanilla `CreakingAi.initIdleActivity` finder:
/// `brain.getMemory(NEAREST_VISIBLE_ATTACKABLE_PLAYER)`.
fn nearest_visible_attackable_player(
    ctx: &BehaviorContext<'_>,
    _body: &SharedEntity,
) -> Option<SharedEntity> {
    ctx.get_memory(&mem::NEAREST_VISIBLE_ATTACKABLE_PLAYER)
}

/// Vanilla `CreakingAi.isAttackTargetStillReachable`: the target is one of the
/// visible attackable players remembered by the sensor.
fn is_attack_target_still_reachable(ctx: &BehaviorContext<'_>, target: &SharedEntity) -> bool {
    ctx.get_memory(&mem::NEAREST_VISIBLE_ATTACKABLE_PLAYERS)
        .is_some_and(|players| players.iter().any(|player| player.id() == target.id()))
}
