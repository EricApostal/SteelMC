//! Vanilla `HappyGhastAi` + `HappyGhast.BRAIN_PROVIDER`.
//!
//! `HappyGhast.makeBrain` is a `Brain.Provider` over the five vanilla sensors and
//! the three activities from `HappyGhastAi.getActivities`;
//! `HappyGhast.customServerAiStep` only ticks that brain for babies and then calls
//! `HappyGhastAi.updateActivity`. Steel keeps the same split: [`make_brain`] is the
//! provider, [`tick`] the brain + activity half.
//!
//! ## Deviations
//!
//! - `HappyGhastAi.initCoreActivity` registers `new AnimalPanic(2.0F, 0)`, the
//!   flying variant that flees through `AirAndWaterRandomPos`; Steel's
//!   [`AnimalPanic`] only implements the `LandRandomPos` panic position, so panic
//!   flight reuses the land/water getter.
//! - `Swim(0.8F)`/`AnimalPanic`/`LookAtTargetSink`/`MoveToTargetSink` are the shared
//!   Steel behaviours with the same vanilla arguments.

use steel_utils::value_providers::UniformIntProvider;

use crate::entity::SharedEntity;
use crate::entity::ai::behavior::animal_panic::AnimalPanic;
use crate::entity::ai::behavior::baby_follow_adult::BabyFollowAdult;
use crate::entity::ai::behavior::count_down_cooldown_ticks::CountDownCooldownTicks;
use crate::entity::ai::behavior::follow_temptation::FollowTemptation;
use crate::entity::ai::behavior::look_at_target_sink::LookAtTargetSink;
use crate::entity::ai::behavior::move_to_target_sink::MoveToTargetSink;
use crate::entity::ai::behavior::random_stroll::RandomStroll;
use crate::entity::ai::behavior::set_walk_target_from_look_target::SetWalkTargetFromLookTarget;
use crate::entity::ai::behavior::swim::Swim;
use crate::entity::ai::brain::SensorType;
use crate::entity::ai::brain::activity::Activity;
use crate::entity::ai::brain::behavior::builder::BehaviorBuilder;
use crate::entity::ai::brain::behavior::gate::RunOne;
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::{ActivityData, Brain};
use crate::entity::ai::sensing;
use crate::entity::ai::sensing::villager as villager_sensors;
use crate::entity::entities::mobs::passive::happy_ghast::HappyGhastEntity;
use crate::world::World;

/// Vanilla `HappyGhastAi.SPEED_MULTIPLIER_WHEN_IDLING`.
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 1.0;
/// Vanilla `HappyGhastAi.SPEED_MULTIPLIER_WHEN_TEMPTED`.
const SPEED_MULTIPLIER_WHEN_TEMPTED: f32 = 1.25;
/// Vanilla `HappyGhastAi.SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT`.
const SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT: f32 = 1.1;
/// Vanilla `HappyGhastAi.BABY_GHAST_CLOSE_ENOUGH_DIST`.
const BABY_GHAST_CLOSE_ENOUGH_DIST: f64 = 3.0;
/// Vanilla `HappyGhastAi.ADULT_FOLLOW_RANGE`, `UniformInt.of(3, 16)`.
const ADULT_FOLLOW_RANGE: UniformIntProvider = UniformIntProvider {
    min_inclusive: 3,
    max_inclusive: 16,
};
/// Vanilla `HappyGhastAi.SPEED_MULTIPLIER_WHEN_PANICKING`.
const SPEED_MULTIPLIER_WHEN_PANICKING: f32 = 2.0;
/// Vanilla `Swim(0.8F)` chance argument.
const SWIM_CHANCE: f32 = 0.8;
/// Vanilla `SetWalkTargetFromLookTarget.create(1.0F, 3)` close-enough distance.
const LOOK_TARGET_CLOSE_ENOUGH: i32 = 3;

/// Vanilla `HappyGhast.BRAIN_PROVIDER` sensor list.
static SENSOR_TYPES: [&SensorType; 5] = [
    &villager_sensors::NEAREST_LIVING_ENTITIES,
    &villager_sensors::HURT_BY,
    &sensing::tempting::FOOD_TEMPTATIONS,
    &sensing::adult_any_type::NEAREST_ADULT_ANY_TYPE,
    &villager_sensors::NEAREST_PLAYERS,
];

/// Vanilla `HappyGhast.BRAIN_PROVIDER.makeBrain`.
///
/// The provider derives its memory list from the sensor list, so the memory slice
/// is empty exactly like the vanilla `Brain.provider(sensors, activities)` call.
#[must_use]
pub(crate) fn make_brain(_ghast: &HappyGhastEntity) -> Brain {
    Brain::new(&[], &SENSOR_TYPES, activities())
}

/// Vanilla `HappyGhastAi.getActivities`.
#[must_use]
fn activities() -> Vec<ActivityData> {
    vec![core_activity(), idle_activity(), panic_activity()]
}

/// Vanilla `HappyGhastAi.initCoreActivity`.
fn core_activity() -> ActivityData {
    ActivityData::create_priority(
        Activity::Core,
        0,
        vec![
            BehaviorBuilder::boxed(Swim::new(SWIM_CHANCE)),
            BehaviorBuilder::boxed(AnimalPanic::new(SPEED_MULTIPLIER_WHEN_PANICKING)),
            BehaviorBuilder::boxed(LookAtTargetSink::new(45, 90)),
            BehaviorBuilder::boxed(MoveToTargetSink::new()),
            BehaviorBuilder::boxed(CountDownCooldownTicks::new(&mem::TEMPTATION_COOLDOWN_TICKS)),
        ],
    )
}

/// Vanilla `HappyGhastAi.initIdleActivity`.
fn idle_activity() -> ActivityData {
    ActivityData::create(
        Activity::Idle,
        vec![
            (
                1,
                BehaviorBuilder::boxed(FollowTemptation::create_full(
                    |_| SPEED_MULTIPLIER_WHEN_TEMPTED,
                    |_| BABY_GHAST_CLOSE_ENOUGH_DIST,
                    true,
                )),
            ),
            (
                2,
                BehaviorBuilder::boxed(BabyFollowAdult::create_with_speed_fn(
                    ADULT_FOLLOW_RANGE,
                    |_| SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT,
                    &mem::NEAREST_VISIBLE_PLAYER,
                    true,
                )),
            ),
            (
                3,
                BehaviorBuilder::boxed(BabyFollowAdult::create_with_speed_fn(
                    ADULT_FOLLOW_RANGE,
                    |_| SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT,
                    &mem::NEAREST_VISIBLE_ADULT,
                    true,
                )),
            ),
            (
                4,
                BehaviorBuilder::boxed(RunOne::new(vec![
                    (
                        BehaviorBuilder::boxed(RandomStroll::fly(SPEED_MULTIPLIER_WHEN_IDLING)),
                        1,
                    ),
                    (
                        BehaviorBuilder::boxed(SetWalkTargetFromLookTarget::create(
                            SPEED_MULTIPLIER_WHEN_IDLING,
                            LOOK_TARGET_CLOSE_ENOUGH,
                        )),
                        1,
                    ),
                ])),
            ),
        ],
    )
}

/// Vanilla `HappyGhastAi.initPanicActivity`: entering it requires `IS_PANICKING`.
fn panic_activity() -> ActivityData {
    ActivityData::create_full(
        Activity::Panic,
        Vec::new(),
        vec![(&mem::IS_PANICKING, MemoryStatus::ValuePresent)],
        Vec::new(),
    )
}

/// Vanilla `HappyGhast.customServerAiStep` brain half plus `HappyGhastAi.updateActivity`.
pub(crate) fn tick(brain: &mut Brain, level: &World, body: &SharedEntity) {
    brain.tick(level, body);
    update_activity(brain);
}

/// Vanilla `HappyGhastAi.updateActivity`: the first valid of `PANIC`, `IDLE` wins.
pub(crate) fn update_activity(brain: &mut Brain) {
    brain.set_active_activity_to_first_valid(&[Activity::Panic, Activity::Idle]);
}
