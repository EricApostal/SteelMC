//! Vanilla `CamelAi` + `Camel.BRAIN_PROVIDER`.
//!
//! Vanilla drives camels from a `Brain`: `Camel.registerGoals()` is empty and
//! `Camel.customServerAiStep` ticks `CamelAi`'s two activity packages (`CORE`,
//! `IDLE`) and then calls `CamelAi.updateActivity`. Steel keeps that split:
//! [`make_brain`] is the provider, [`tick`] the brain half and [`update_activity`]
//! the activity switch.
//!
//! `CamelAi.initMemories` is empty, so nothing is seeded at spawn.
//!
//! ## Camel-specific behaviors
//!
//! `CamelAi.CamelPanic` and `CamelAi.RandomSitting` are inner classes of the AI in
//! vanilla, not shared `net.minecraft.world.entity.ai.behavior` classes, so they are
//! implemented here as private structs next to the packages that use them. Every
//! other behavior comes from the shared behavior library.
//!
//! `// GAP:` `CamelAi.CamelPanic` checks `!body.isMobControlled()`; Steel has no
//! `AbstractHorse.isMobControlled` on the mob surface, so the same test is written
//! against `controlling_passenger().is_mob()` here.

use steel_registry::vanilla_entities;
use steel_utils::Downcast as _;
use steel_utils::value_providers::{IntProvider, UniformIntProvider};

use crate::entity::SharedEntity;
use crate::entity::ai::behavior::animal_make_love::AnimalMakeLove;
use crate::entity::ai::behavior::animal_panic::AnimalPanic;
use crate::entity::ai::behavior::baby_follow_adult::BabyFollowAdult;
use crate::entity::ai::behavior::count_down_cooldown_ticks::CountDownCooldownTicks;
use crate::entity::ai::behavior::do_nothing::DoNothing;
use crate::entity::ai::behavior::follow_temptation::FollowTemptation;
use crate::entity::ai::behavior::look_at_target_sink::LookAtTargetSink;
use crate::entity::ai::behavior::move_to_target_sink::MoveToTargetSink;
use crate::entity::ai::behavior::random_look_around::RandomLookAround;
use crate::entity::ai::behavior::random_stroll::RandomStroll;
use crate::entity::ai::behavior::set_entity_look_target_sometimes::SetEntityLookTargetSometimes;
use crate::entity::ai::behavior::set_walk_target_from_look_target::SetWalkTargetFromLookTarget;
use crate::entity::ai::behavior::swim::Swim;
use crate::entity::ai::brain::SensorType;
use crate::entity::ai::brain::activity::Activity;
use crate::entity::ai::brain::behavior::builder::BehaviorBuilder;
use crate::entity::ai::brain::behavior::gate::RunOne;
use crate::entity::ai::brain::behavior::{
    Behavior, BehaviorContext, BehaviorControl, BehaviorState, BehaviorStatus,
};
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::{ActivityData, Brain};
use crate::entity::ai::sensing;
use crate::entity::ai::sensing::villager as villager_sensors;
use crate::entity::entities::mobs::passive::camel::CamelEntity;
use crate::world::World;

/// Vanilla `CamelAi.SPEED_MULTIPLIER_WHEN_PANICKING`.
const SPEED_MULTIPLIER_WHEN_PANICKING: f32 = 4.0;
/// Vanilla `CamelAi.SPEED_MULTIPLIER_WHEN_IDLING`.
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 2.0;
/// Vanilla `CamelAi.SPEED_MULTIPLIER_WHEN_TEMPTED`.
const SPEED_MULTIPLIER_WHEN_TEMPTED: f32 = 2.5;
/// Vanilla `CamelAi.SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT`.
const SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT: f32 = 2.5;
/// Vanilla `CamelAi.ADULT_FOLLOW_RANGE`, `UniformInt.of(5, 16)`.
const ADULT_FOLLOW_RANGE: UniformIntProvider = UniformIntProvider {
    min_inclusive: 5,
    max_inclusive: 16,
};
/// Vanilla `SetEntityLookTargetSometimes.create(EntityTypes.PLAYER, 6.0F,
/// UniformInt.of(30, 60))` distance.
const LOOK_DISTANCE: f32 = 6.0;
/// Vanilla `SetEntityLookTargetSometimes.create(..., UniformInt.of(30, 60))`.
const LOOK_INTERVAL: UniformIntProvider = UniformIntProvider {
    min_inclusive: 30,
    max_inclusive: 60,
};
/// Vanilla `new RandomLookAround(UniformInt.of(150, 250), 30.0F, 0.0F, 0.0F)`.
const RANDOM_LOOK_INTERVAL: IntProvider = IntProvider::Uniform {
    min_inclusive: 150,
    max_inclusive: 250,
};
/// Vanilla `new RandomLookAround(..., 30.0F, ...)` max yaw.
const RANDOM_LOOK_MAX_YAW: f32 = 30.0;
/// Vanilla `new CamelAi.RandomSitting(20)` minimum seconds between pose changes.
const RANDOM_SITTING_MINIMAL_POSE_TIME_SEC: i32 = 20;

/// Vanilla `Camel.BRAIN_PROVIDER` sensor list.
static SENSOR_TYPES: [&SensorType; 4] = [
    &villager_sensors::NEAREST_LIVING_ENTITIES,
    &villager_sensors::HURT_BY,
    &sensing::tempting::FOOD_TEMPTATIONS,
    &sensing::adult::NEAREST_ADULT,
];

/// Vanilla `Camel.BRAIN_PROVIDER.makeBrain`.
///
/// The provider reads no camel state (its sensor and activity lists are static),
/// so the parameter only fixes the call shape the camel's own `make_brain` uses.
#[must_use]
pub(crate) fn make_brain(_camel: &CamelEntity) -> Brain {
    Brain::new(mem::ALL, &SENSOR_TYPES, activities())
}

/// Vanilla `CamelAi.getActivities`.
#[must_use]
fn activities() -> Vec<ActivityData> {
    vec![core_activity(), idle_activity()]
}

/// Vanilla `CamelAi.initCoreActivity`.
fn core_activity() -> ActivityData {
    ActivityData::create_priority(
        Activity::Core,
        0,
        vec![
            BehaviorBuilder::boxed(Swim::new(0.8)),
            BehaviorBuilder::boxed(CamelPanic::new(SPEED_MULTIPLIER_WHEN_PANICKING)),
            BehaviorBuilder::boxed(LookAtTargetSink::new(45, 90)),
            BehaviorBuilder::boxed(MoveToTargetSink::new()),
            BehaviorBuilder::boxed(CountDownCooldownTicks::new(&mem::TEMPTATION_COOLDOWN_TICKS)),
            BehaviorBuilder::boxed(CountDownCooldownTicks::new(&mem::GAZE_COOLDOWN_TICKS)),
        ],
    )
}

/// Vanilla `CamelAi.initIdleActivity`.
fn idle_activity() -> ActivityData {
    ActivityData::create(
        Activity::Idle,
        vec![
            (0, look_at_players()),
            (
                1,
                BehaviorBuilder::boxed(AnimalMakeLove::new(&vanilla_entities::CAMEL)),
            ),
            (
                2,
                BehaviorBuilder::boxed(RunOne::new(vec![
                    (BehaviorBuilder::boxed(follow_temptation()), 1),
                    (
                        BehaviorBuilder::boxed(BehaviorBuilder::create(
                            BehaviorBuilder::trigger_if_behavior(
                                |ctx: &mut BehaviorContext<'_>| !is_refusing_to_move(ctx.body),
                                Box::new(BabyFollowAdult::create(
                                    ADULT_FOLLOW_RANGE,
                                    SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT,
                                )),
                            ),
                        )),
                        1,
                    ),
                ])),
            ),
            (
                3,
                BehaviorBuilder::boxed(RandomLookAround::new(
                    RANDOM_LOOK_INTERVAL,
                    RANDOM_LOOK_MAX_YAW,
                    0.0,
                    0.0,
                )),
            ),
            (
                4,
                BehaviorBuilder::boxed(RunOne::with_entry_condition(
                    vec![(&mem::WALK_TARGET, MemoryStatus::ValueAbsent)],
                    vec![
                        (
                            BehaviorBuilder::boxed(BehaviorBuilder::create(
                                BehaviorBuilder::trigger_if_behavior(
                                    |ctx: &mut BehaviorContext<'_>| !is_refusing_to_move(ctx.body),
                                    Box::new(RandomStroll::stroll(SPEED_MULTIPLIER_WHEN_IDLING)),
                                ),
                            )),
                            1,
                        ),
                        (
                            BehaviorBuilder::boxed(BehaviorBuilder::create(
                                BehaviorBuilder::trigger_if_behavior(
                                    |ctx: &mut BehaviorContext<'_>| !is_refusing_to_move(ctx.body),
                                    Box::new(SetWalkTargetFromLookTarget::create(
                                        SPEED_MULTIPLIER_WHEN_IDLING,
                                        3,
                                    )),
                                ),
                            )),
                            1,
                        ),
                        (
                            BehaviorBuilder::boxed(RandomSitting::new(
                                RANDOM_SITTING_MINIMAL_POSE_TIME_SEC,
                            )),
                            1,
                        ),
                        (BehaviorBuilder::boxed(DoNothing::new(30, 60)), 1),
                    ],
                )),
            ),
        ],
    )
}

/// Vanilla `Camel.customServerAiStep` brain half.
pub(crate) fn tick(brain: &mut Brain, level: &World, body: &SharedEntity) {
    brain.tick(level, body);
    update_activity(brain, body);
}

/// Vanilla `CamelAi.updateActivity`: camels only ever select `IDLE`.
pub(crate) fn update_activity(brain: &mut Brain, _body: &SharedEntity) {
    brain.set_active_activity_to_first_valid(&[Activity::Idle]);
}

/// Vanilla `SetEntityLookTargetSometimes.create(EntityTypes.PLAYER, 6.0F,
/// UniformInt.of(30, 60))`.
fn look_at_players() -> Box<dyn BehaviorControl> {
    BehaviorBuilder::boxed(SetEntityLookTargetSometimes::for_type(
        &vanilla_entities::PLAYER,
        LOOK_DISTANCE,
        LOOK_INTERVAL,
    ))
}

/// Vanilla `new FollowTemptation(camel -> 2.5F, camel -> camel.isBaby() ? 2.5 : 3.5)`.
fn follow_temptation() -> FollowTemptation {
    FollowTemptation::create_full(
        |_body: &SharedEntity| SPEED_MULTIPLIER_WHEN_TEMPTED,
        |body: &SharedEntity| {
            if body
                .as_ageable_mob()
                .is_some_and(crate::entity::AgeableMob::is_baby)
            {
                2.5
            } else {
                3.5
            }
        },
        false,
    )
}

/// Vanilla `Camel::refuseToMove`, resolved from an erased body.
///
/// The camel is the only body these packages run on, so a non-camel body (a test
/// double) refuses nothing.
fn is_refusing_to_move(body: &SharedEntity) -> bool {
    body.downcast_ref::<CamelEntity>()
        .is_some_and(CamelEntity::refuse_to_move)
}

/// Vanilla `CamelAi.CamelPanic extends AnimalPanic<Camel>`: panics like any animal,
/// but stands up first and never while a mob is steering it.
///
/// `AnimalPanic` keeps its state private, so the decorator forwards the
/// [`BehaviorControl`] calls it does not change.
struct CamelPanic {
    inner: AnimalPanic,
}

impl CamelPanic {
    fn new(speed_multiplier: f32) -> Self {
        Self {
            inner: AnimalPanic::new(speed_multiplier),
        }
    }
}

impl BehaviorControl for CamelPanic {
    fn status(&self) -> BehaviorStatus {
        self.inner.status()
    }

    fn required_memories(&self) -> Vec<MemoryModuleTypeRef> {
        self.inner.required_memories()
    }

    fn try_start(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        // Vanilla `checkExtraStartConditions`: `super && !body.isMobControlled()`.
        if ctx
            .body
            .controlling_passenger()
            .is_some_and(|passenger| passenger.is_mob())
        {
            return false;
        }

        if !self.inner.try_start(ctx) {
            return false;
        }

        // Vanilla `start`: `standUpInstantly()` then `super.start(...)`. Steel's
        // `try_start` runs `start` first, so the stand-up lands right after the
        // panic walk target is set; the two are independent.
        if let Some(camel) = ctx.body.downcast_ref::<CamelEntity>() {
            camel.stand_up_instantly();
        }
        true
    }

    fn tick_or_stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        self.inner.tick_or_stop(ctx);
    }

    fn do_stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        self.inner.do_stop(ctx);
    }

    fn debug_string(&self) -> String {
        self.inner.debug_string()
    }
}

/// Vanilla `CamelAi.RandomSitting`: sit down or stand up on the idle clock.
struct RandomSitting {
    state: BehaviorState,
    minimal_pose_ticks: i32,
}

impl RandomSitting {
    /// Vanilla `new CamelAi.RandomSitting(int minimalPoseTimeSec)`.
    fn new(minimal_pose_time_sec: i32) -> Self {
        Self {
            state: BehaviorState::with_entry_condition(Vec::new()),
            minimal_pose_ticks: minimal_pose_time_sec * 20,
        }
    }
}

impl Behavior for RandomSitting {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        let Some(camel) = ctx.body.downcast_ref::<CamelEntity>() else {
            return false;
        };
        let body = ctx.body;

        !body.is_in_water()
            && camel.pose_time() >= i64::from(self.minimal_pose_ticks)
            && !body.as_mob().is_some_and(|mob| mob.is_leashed())
            && body.on_ground()
            && body.controlling_passenger().is_none()
            && camel.can_camel_change_pose()
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        let Some(camel) = ctx.body.downcast_ref::<CamelEntity>() else {
            return;
        };

        if camel.is_camel_sitting() {
            camel.stand_up();
        } else if ctx.get_memory(&mem::IS_PANICKING) != Some(true) {
            camel.sit_down();
        }
    }
}
