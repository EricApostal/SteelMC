//! Vanilla `GoatAi` + `Goat.BRAIN_PROVIDER`.
//!
//! Vanilla drives goats from a `Brain`: `Goat.registerGoals()` is empty and
//! `Goat.customServerAiStep` ticks `GoatAi`'s four activity packages (`CORE`,
//! `IDLE`, `LONG_JUMP`, `RAM`) and then calls `GoatAi.updateActivity`. Steel keeps
//! that split: [`make_brain`] is the provider and [`tick`] is the brain tick plus
//! the activity switch.
//!
//! `GoatAi.initMemories` seeds `LONG_JUMP_COOLDOWN_TICKS` and `RAM_COOLDOWN_TICKS`
//! at spawn, not from `makeBrain`; [`crate::entity::entities::mobs::passive::goat::GoatEntity::init_ai_memories`]
//! is that half.
//!
//! ## Deviations
//!
//! - `RamTarget`/`PrepareRamNearestTarget` carry no per-body sound or knockback
//!   selectors, so the goat's screaming and baby variants are resolved once when
//!   the brain is built (vanilla re-evaluates on every ram).
//! - `LongJumpToRandomPos`/`LongJumpMidJump` take no sound handles yet, so the
//!   goat's screaming/regular long-jump sounds are not played by the behaviors.

use steel_registry::vanilla_entities;
use steel_registry::vanilla_game_rules::MOB_GRIEFING;
use steel_utils::value_providers::UniformIntProvider;

use crate::entity::AgeableMob;
use crate::entity::SharedEntity;
use crate::entity::ai::behavior::animal_make_love::AnimalMakeLove;
use crate::entity::ai::behavior::animal_panic::AnimalPanic;
use crate::entity::ai::behavior::baby_follow_adult::BabyFollowAdult;
use crate::entity::ai::behavior::count_down_cooldown_ticks::CountDownCooldownTicks;
use crate::entity::ai::behavior::do_nothing::DoNothing;
use crate::entity::ai::behavior::follow_temptation::FollowTemptation;
use crate::entity::ai::behavior::long_jump_mid_jump::LongJumpMidJump;
use crate::entity::ai::behavior::long_jump_to_random_pos::LongJumpToRandomPos;
use crate::entity::ai::behavior::look_at_target_sink::LookAtTargetSink;
use crate::entity::ai::behavior::move_to_target_sink::MoveToTargetSink;
use crate::entity::ai::behavior::prepare_ram_nearest_target::PrepareRamNearestTarget;
use crate::entity::ai::behavior::ram_target::RamTarget;
use crate::entity::ai::behavior::random_stroll::RandomStroll;
use crate::entity::ai::behavior::set_entity_look_target_sometimes::SetEntityLookTargetSometimes;
use crate::entity::ai::behavior::set_walk_target_from_look_target::SetWalkTargetFromLookTarget;
use crate::entity::ai::behavior::swim::Swim;
use crate::entity::ai::brain::SensorType;
use crate::entity::ai::brain::activity::Activity;
use crate::entity::ai::brain::behavior::BehaviorControl;
use crate::entity::ai::brain::behavior::builder::BehaviorBuilder;
use crate::entity::ai::brain::behavior::gate::RunOne;
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::{ActivityData, Brain};
use crate::entity::ai::sensing;
use crate::entity::ai::sensing::villager as villager_sensors;
use crate::entity::ai::targeting::TargetingConditions;
use crate::entity::entities::mobs::passive::goat::GoatEntity;
use crate::world::World;

/// Vanilla `GoatAi.SPEED_MULTIPLIER_WHEN_IDLING`.
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 1.0;
/// Vanilla `GoatAi.SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT`.
const SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT: f32 = 1.25;
/// Vanilla `GoatAi.SPEED_MULTIPLIER_WHEN_TEMPTED`.
const SPEED_MULTIPLIER_WHEN_TEMPTED: f32 = 1.25;
/// Vanilla `GoatAi.SPEED_MULTIPLIER_WHEN_PANICKING`.
const SPEED_MULTIPLIER_WHEN_PANICKING: f32 = 2.0;
/// Vanilla `GoatAi.SPEED_MULTIPLIER_WHEN_PREPARING_TO_RAM`.
const SPEED_MULTIPLIER_WHEN_PREPARING_TO_RAM: f32 = 1.25;
/// Vanilla `GoatAi.SPEED_MULTIPLIER_WHEN_RAMMING`.
const SPEED_MULTIPLIER_WHEN_RAMMING: f32 = 3.0;
/// Vanilla `GoatAi.ADULT_RAM_KNOCKBACK_FORCE`.
const ADULT_RAM_KNOCKBACK_FORCE: f64 = 2.5;
/// Vanilla `GoatAi.BABY_RAM_KNOCKBACK_FORCE`.
const BABY_RAM_KNOCKBACK_FORCE: f64 = 1.0;
/// Vanilla `GoatAi.RAM_MIN_DISTANCE`.
const RAM_MIN_DISTANCE: i32 = 4;
/// Vanilla `GoatAi.RAM_MAX_DISTANCE`.
const RAM_MAX_DISTANCE: i32 = 7;
/// Vanilla `GoatAi.RAM_PREPARE_TIME`.
const RAM_PREPARE_TIME: i32 = 20;
/// Vanilla `GoatAi.MAX_LONG_JUMP_HEIGHT`.
const MAX_LONG_JUMP_HEIGHT: i32 = 5;
/// Vanilla `GoatAi.MAX_LONG_JUMP_WIDTH`.
const MAX_LONG_JUMP_WIDTH: i32 = 5;
/// Vanilla `GoatAi.MAX_JUMP_VELOCITY_MULTIPLIER`.
const MAX_JUMP_VELOCITY_MULTIPLIER: f32 = 3.571_428_8;
/// Vanilla `SetEntityLookTargetSometimes.create(EntityTypes.PLAYER, 6.0F,
/// UniformInt.of(30, 60))` distance.
const LOOK_DISTANCE: f32 = 6.0;
/// Vanilla `SetEntityLookTargetSometimes.create(..., UniformInt.of(30, 60))`.
const LOOK_INTERVAL: UniformIntProvider = UniformIntProvider {
    min_inclusive: 30,
    max_inclusive: 60,
};
/// Vanilla `GoatAi.ADULT_FOLLOW_RANGE`, `UniformInt.of(5, 16)`.
const ADULT_FOLLOW_RANGE: UniformIntProvider = UniformIntProvider {
    min_inclusive: 5,
    max_inclusive: 16,
};
/// Vanilla `GoatAi.TIME_BETWEEN_LONG_JUMPS`, `UniformInt.of(600, 1200)`.
const TIME_BETWEEN_LONG_JUMPS: UniformIntProvider = UniformIntProvider {
    min_inclusive: 600,
    max_inclusive: 1200,
};
/// Lower bound of vanilla `GoatAi.TIME_BETWEEN_LONG_JUMPS`.
pub(crate) const TIME_BETWEEN_LONG_JUMPS_MIN: i32 = 600;
/// Upper bound of vanilla `GoatAi.TIME_BETWEEN_LONG_JUMPS`.
pub(crate) const TIME_BETWEEN_LONG_JUMPS_MAX: i32 = 1200;
/// Vanilla `GoatAi.TIME_BETWEEN_RAMS`, `UniformInt.of(600, 6000)`.
const TIME_BETWEEN_RAMS: UniformIntProvider = UniformIntProvider {
    min_inclusive: 600,
    max_inclusive: 6000,
};
/// Lower bound of vanilla `GoatAi.TIME_BETWEEN_RAMS`.
pub(crate) const TIME_BETWEEN_RAMS_MIN: i32 = 600;
/// Upper bound of vanilla `GoatAi.TIME_BETWEEN_RAMS`.
pub(crate) const TIME_BETWEEN_RAMS_MAX: i32 = 6000;
/// Vanilla `GoatAi.TIME_BETWEEN_RAMS_SCREAMER`, `UniformInt.of(100, 300)`.
const TIME_BETWEEN_RAMS_SCREAMER: UniformIntProvider = UniformIntProvider {
    min_inclusive: 100,
    max_inclusive: 300,
};

/// Vanilla `Goat.BRAIN_PROVIDER` sensor list, in vanilla order.
static SENSOR_TYPES: [&SensorType; 6] = [
    &villager_sensors::NEAREST_LIVING_ENTITIES,
    &villager_sensors::NEAREST_PLAYERS,
    &villager_sensors::NEAREST_ITEMS,
    &sensing::adult::NEAREST_ADULT,
    &villager_sensors::HURT_BY,
    &sensing::tempting::FOOD_TEMPTATIONS,
];

/// Vanilla `Goat.makeBrain` (`BRAIN_PROVIDER.makeBrain`).
///
/// The activity packages resolve the goat's screaming/baby variants once, because
/// the shared `RamTarget`/`PrepareRamNearestTarget` behaviors take plain values.
#[must_use]
pub(crate) fn make_brain(goat: &GoatEntity) -> Brain {
    let is_screaming = goat.is_screaming_goat();
    let is_baby = AgeableMob::is_baby(goat);
    Brain::new(mem::ALL, &SENSOR_TYPES, activities(is_screaming, is_baby))
}

/// Vanilla `GoatAi.getActivities`.
fn activities(is_screaming: bool, is_baby: bool) -> Vec<ActivityData> {
    vec![
        core_activity(),
        idle_activity(),
        long_jump_activity(),
        ram_activity(is_screaming, is_baby),
    ]
}

/// Vanilla `GoatAi.initCoreActivity`.
fn core_activity() -> ActivityData {
    ActivityData::create_priority(
        Activity::Core,
        0,
        vec![
            BehaviorBuilder::boxed(Swim::new(0.8)),
            BehaviorBuilder::boxed(AnimalPanic::new(SPEED_MULTIPLIER_WHEN_PANICKING)),
            BehaviorBuilder::boxed(LookAtTargetSink::new(45, 90)),
            BehaviorBuilder::boxed(MoveToTargetSink::new()),
            BehaviorBuilder::boxed(CountDownCooldownTicks::new(&mem::TEMPTATION_COOLDOWN_TICKS)),
            BehaviorBuilder::boxed(CountDownCooldownTicks::new(&mem::LONG_JUMP_COOLDOWN_TICKS)),
            BehaviorBuilder::boxed(CountDownCooldownTicks::new(&mem::RAM_COOLDOWN_TICKS)),
        ],
    )
}

/// Vanilla `GoatAi.initIdleActivity`.
fn idle_activity() -> ActivityData {
    ActivityData::create_full(
        Activity::Idle,
        vec![
            (0, look_at_players()),
            (
                0,
                BehaviorBuilder::boxed(AnimalMakeLove::new(&vanilla_entities::GOAT)),
            ),
            (
                1,
                BehaviorBuilder::boxed(FollowTemptation::new(SPEED_MULTIPLIER_WHEN_TEMPTED)),
            ),
            (
                2,
                BehaviorBuilder::boxed(BabyFollowAdult::create(
                    ADULT_FOLLOW_RANGE,
                    SPEED_MULTIPLIER_WHEN_FOLLOWING_ADULT,
                )),
            ),
            (
                3,
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
            ),
        ],
        vec![
            (&mem::RAM_COOLDOWN_TICKS, MemoryStatus::ValuePresent),
            (&mem::LONG_JUMP_COOLDOWN_TICKS, MemoryStatus::ValuePresent),
        ],
        Vec::new(),
    )
}

/// Vanilla `GoatAi.initLongJumpActivity`.
fn long_jump_activity() -> ActivityData {
    ActivityData::create_full(
        Activity::LongJump,
        vec![
            (
                0,
                BehaviorBuilder::boxed(LongJumpMidJump::new(TIME_BETWEEN_LONG_JUMPS)),
            ),
            (
                1,
                BehaviorBuilder::boxed(LongJumpToRandomPos::new(
                    TIME_BETWEEN_LONG_JUMPS,
                    MAX_LONG_JUMP_HEIGHT,
                    MAX_LONG_JUMP_WIDTH,
                    MAX_JUMP_VELOCITY_MULTIPLIER,
                )),
            ),
        ],
        vec![
            (&mem::TEMPTING_PLAYER, MemoryStatus::ValueAbsent),
            (&mem::BREED_TARGET, MemoryStatus::ValueAbsent),
            (&mem::WALK_TARGET, MemoryStatus::ValueAbsent),
            (&mem::LONG_JUMP_COOLDOWN_TICKS, MemoryStatus::ValueAbsent),
        ],
        Vec::new(),
    )
}

/// Vanilla `GoatAi.initRamActivity`.
fn ram_activity(is_screaming: bool, is_baby: bool) -> ActivityData {
    let time_between_rams = if is_screaming {
        TIME_BETWEEN_RAMS_SCREAMER
    } else {
        TIME_BETWEEN_RAMS
    };
    let cooldown_on_fail = time_between_rams.min_inclusive;
    let knockback_force = if is_baby {
        BABY_RAM_KNOCKBACK_FORCE
    } else {
        ADULT_RAM_KNOCKBACK_FORCE
    };

    ActivityData::create_full(
        Activity::Ram,
        vec![
            (
                0,
                BehaviorBuilder::boxed(RamTarget::new(
                    time_between_rams,
                    ram_target_conditions(),
                    SPEED_MULTIPLIER_WHEN_RAMMING,
                    knockback_force,
                )),
            ),
            (
                1,
                BehaviorBuilder::boxed(PrepareRamNearestTarget::new(
                    cooldown_on_fail,
                    RAM_MIN_DISTANCE,
                    RAM_MAX_DISTANCE,
                    SPEED_MULTIPLIER_WHEN_PREPARING_TO_RAM,
                    ram_target_conditions(),
                    RAM_PREPARE_TIME,
                )),
            ),
        ],
        vec![
            (&mem::TEMPTING_PLAYER, MemoryStatus::ValueAbsent),
            (&mem::BREED_TARGET, MemoryStatus::ValueAbsent),
            (&mem::RAM_COOLDOWN_TICKS, MemoryStatus::ValueAbsent),
        ],
        Vec::new(),
    )
}

/// Vanilla `GoatAi.RAM_TARGET_CONDITIONS`: combat targeting that skips goats,
/// respects `MOB_GRIEFING` for armor stands, and stays inside the world border.
fn ram_target_conditions() -> TargetingConditions {
    TargetingConditions::for_combat().selector(|target, level| {
        target.entity_type() != &vanilla_entities::GOAT
            && (level.get_game_rule(&MOB_GRIEFING)
                || target.entity_type() != &vanilla_entities::ARMOR_STAND)
            && level
                .world_border_snapshot()
                .is_within_bounds(target.bounding_box())
    })
}

/// Vanilla `Goat.customServerAiStep` brain half: tick the brain, then run
/// `GoatAi.updateActivity`.
pub(crate) fn tick(brain: &mut Brain, level: &World, body: &SharedEntity) {
    brain.tick(level, body);
    update_activity(brain, body);
}

/// Vanilla `GoatAi.updateActivity`: ram, then long jump, then idle.
pub(crate) fn update_activity(brain: &mut Brain, _body: &SharedEntity) {
    brain.set_active_activity_to_first_valid(&[Activity::Ram, Activity::LongJump, Activity::Idle]);
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
