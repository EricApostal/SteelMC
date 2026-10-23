//! Vanilla `TadpoleAi` + `Tadpole.BRAIN_PROVIDER`.
//!
//! Vanilla `Tadpole.makeBrain` is a `Brain.Provider` over
//! `{NEAREST_LIVING_ENTITIES, NEAREST_PLAYERS, HURT_BY, FROG_TEMPTATIONS}` and the
//! two activities from `TadpoleAi.getActivities`; `Tadpole.customServerAiStep` ticks
//! the brain and then runs `TadpoleAi.updateActivity`. Steel keeps that split:
//! [`make_brain`] is the provider and [`tick`] is the brain tick plus the activity
//! update.
//!
//! ## Deviations
//!
//! None: every `TadpoleAi` entry maps onto an existing shared behavior.

use steel_registry::vanilla_entities;
use steel_utils::value_providers::UniformIntProvider;

use crate::entity::SharedEntity;
use crate::entity::ai::behavior::animal_panic::AnimalPanic;
use crate::entity::ai::behavior::count_down_cooldown_ticks::CountDownCooldownTicks;
use crate::entity::ai::behavior::follow_temptation::FollowTemptation;
use crate::entity::ai::behavior::look_at_target_sink::LookAtTargetSink;
use crate::entity::ai::behavior::move_to_target_sink::MoveToTargetSink;
use crate::entity::ai::behavior::random_stroll::RandomStroll;
use crate::entity::ai::behavior::set_entity_look_target_sometimes::SetEntityLookTargetSometimes;
use crate::entity::ai::behavior::set_walk_target_from_look_target::SetWalkTargetFromLookTarget;
use crate::entity::ai::brain::SensorType;
use crate::entity::ai::brain::activity::Activity;
use crate::entity::ai::brain::behavior::builder::BehaviorBuilder;
use crate::entity::ai::brain::behavior::gate::{GateBehavior, OrderPolicy, RunningPolicy};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStatus};
use crate::entity::ai::brain::{ActivityData, Brain};
use crate::entity::ai::sensing::tempting;
use crate::entity::ai::sensing::villager as villager_sensors;
use crate::entity::entities::mobs::passive::tadpole::TadpoleEntity;
use crate::world::World;

/// Vanilla `TadpoleAi.SPEED_MULTIPLIER_WHEN_PANICKING`.
const SPEED_MULTIPLIER_WHEN_PANICKING: f32 = 2.0;
/// Vanilla `TadpoleAi.SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER`.
const SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER: f32 = 0.5;
/// Vanilla `TadpoleAi.SPEED_MULTIPLIER_WHEN_TEMPTED`.
const SPEED_MULTIPLIER_WHEN_TEMPTED: f32 = 1.25;
/// Vanilla `SetEntityLookTargetSometimes.create(PLAYER, 6.0F, UniformInt.of(30, 60))`.
const LOOK_DISTANCE: f32 = 6.0;
const LOOK_INTERVAL: UniformIntProvider = UniformIntProvider {
    min_inclusive: 30,
    max_inclusive: 60,
};
/// Vanilla `SetWalkTargetFromLookTarget.create(0.5F, 3)`.
const SET_WALK_TARGET_SPEED: f32 = 0.5;
const SET_WALK_TARGET_CLOSE_ENOUGH: i32 = 3;

/// Vanilla `Tadpole.BRAIN_PROVIDER` sensor order.
static SENSOR_TYPES: [&SensorType; 4] = [
    &villager_sensors::NEAREST_LIVING_ENTITIES,
    &villager_sensors::NEAREST_PLAYERS,
    &villager_sensors::HURT_BY,
    &tempting::FROG_TEMPTATIONS,
];

/// Vanilla `Tadpole.makeBrain`.
///
/// `Brain::new` registers each sensor's `requires()` set and `Brain::add_activity`
/// registers every behavior's required memories, which is exactly vanilla's
/// "union of sensor requirements and behavior entry conditions".
#[must_use]
pub fn make_brain(_tadpole: &TadpoleEntity) -> Brain {
    Brain::new(&[], &SENSOR_TYPES, activities())
}

/// Vanilla `TadpoleAi.getActivities`.
#[must_use]
pub fn activities() -> Vec<ActivityData> {
    vec![init_core_activity(), init_idle_activity()]
}

/// Vanilla `TadpoleAi.initCoreActivity`.
fn init_core_activity() -> ActivityData {
    ActivityData::create_priority(
        Activity::Core,
        0,
        vec![
            BehaviorBuilder::boxed(AnimalPanic::new(SPEED_MULTIPLIER_WHEN_PANICKING)),
            BehaviorBuilder::boxed(LookAtTargetSink::new(45, 90)),
            BehaviorBuilder::boxed(MoveToTargetSink::new()),
            BehaviorBuilder::boxed(CountDownCooldownTicks::new(&mem::TEMPTATION_COOLDOWN_TICKS)),
        ],
    )
}

/// Vanilla `TadpoleAi.initIdleActivity`.
fn init_idle_activity() -> ActivityData {
    let gate = GateBehavior::new(
        vec![(
            &mem::WALK_TARGET as MemoryModuleTypeRef,
            MemoryStatus::ValueAbsent,
        )],
        Vec::new(),
        OrderPolicy::Ordered,
        RunningPolicy::TryAll,
        vec![
            (
                BehaviorBuilder::boxed(RandomStroll::swim(SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER)),
                2,
            ),
            (
                BehaviorBuilder::boxed(SetWalkTargetFromLookTarget::create(
                    SET_WALK_TARGET_SPEED,
                    SET_WALK_TARGET_CLOSE_ENOUGH,
                )),
                3,
            ),
            (
                BehaviorBuilder::boxed(BehaviorBuilder::create(BehaviorBuilder::trigger_if(
                    |ctx| ctx.body.is_in_water(),
                ))),
                5,
            ),
        ],
    );

    ActivityData::create(
        Activity::Idle,
        vec![
            (
                0,
                BehaviorBuilder::boxed(SetEntityLookTargetSometimes::for_type(
                    &vanilla_entities::PLAYER,
                    LOOK_DISTANCE,
                    LOOK_INTERVAL,
                )),
            ),
            (
                1,
                BehaviorBuilder::boxed(FollowTemptation::new(SPEED_MULTIPLIER_WHEN_TEMPTED)),
            ),
            (2, BehaviorBuilder::boxed(gate)),
        ],
    )
}

/// Vanilla `Tadpole.customServerAiStep`'s brain half: tick the brain, then run
/// `TadpoleAi.updateActivity`.
pub fn tick(brain: &mut Brain, level: &World, body: &SharedEntity) {
    brain.tick(level, body);
    // Vanilla `TadpoleAi.updateActivity`.
    brain.set_active_activity_to_first_valid(&[Activity::Idle]);
}
