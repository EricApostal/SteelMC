//! Vanilla `FrogAi` + `Frog.BRAIN_PROVIDER`.
//!
//! `Frog.makeBrain` is a `Brain.Provider` over the five-sensor vanilla list and
//! the six activities from `FrogAi.getActivities`; `Frog.customServerAiStep`
//! ticks the brain and then calls `FrogAi.updateActivity`. Steel keeps that
//! split: [`make_brain`] is the provider, [`tick`] the brain half and
//! [`update_activity`] the activity switch.
//!
//! `FrogAi.initMemories` seeds `LONG_JUMP_COOLDOWN_TICKS` from
//! `UniformInt.of(100, 140)` at spawn (`Frog.create`/`finalizeSpawn`), not from
//! `makeBrain`; [`init_memories`] is exposed for those call sites.

use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_blocks;
use steel_registry::vanilla_entities;
use steel_registry::vanilla_fluid_tags::FluidTag;
use steel_utils::BlockPos;
use steel_utils::value_providers::UniformIntProvider;

use crate::entity::SharedEntity;
use crate::entity::ai::behavior::animal_make_love::AnimalMakeLove;
use crate::entity::ai::behavior::animal_panic::AnimalPanic;
use crate::entity::ai::behavior::count_down_cooldown_ticks::CountDownCooldownTicks;
use crate::entity::ai::behavior::croak::Croak;
use crate::entity::ai::behavior::follow_temptation::FollowTemptation;
use crate::entity::ai::behavior::long_jump_mid_jump::LongJumpMidJump;
use crate::entity::ai::behavior::long_jump_to_preferred_block::LongJumpToPreferredBlock;
use crate::entity::ai::behavior::look_at_target_sink::LookAtTargetSink;
use crate::entity::ai::behavior::move_to_target_sink::MoveToTargetSink;
use crate::entity::ai::behavior::random_stroll::RandomStroll;
use crate::entity::ai::behavior::set_entity_look_target_sometimes::SetEntityLookTargetSometimes;
use crate::entity::ai::behavior::set_walk_target_from_look_target::SetWalkTargetFromLookTarget;
use crate::entity::ai::behavior::start_attacking::StartAttacking;
use crate::entity::ai::behavior::stop_attacking_if_target_invalid::StopAttackingIfTargetInvalid;
use crate::entity::ai::behavior::try_find_land::TryFindLand;
use crate::entity::ai::behavior::try_find_land_near_liquid::TryFindLandNearLiquid;
use crate::entity::ai::behavior::try_lay_spawn_on_fluid_near_land::TryLaySpawnOnFluidNearLand;
use crate::entity::ai::brain::SensorType;
use crate::entity::ai::brain::activity::Activity;
use crate::entity::ai::brain::behavior::builder::BehaviorBuilder;
use crate::entity::ai::brain::behavior::gate::{GateBehavior, OrderPolicy, RunOne, RunningPolicy};
use crate::entity::ai::brain::behavior::{BehaviorContext, BehaviorControl, TriggerIf};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::{ActivityData, Brain};
use crate::entity::ai::sensing;
use crate::entity::ai::sensing::villager as villager_sensors;
use crate::entity::entities::mobs::passive::frog::{FrogEntity, ShootTongue};
use crate::world::World;

/// Vanilla `FrogAi.SPEED_MULTIPLIER_WHEN_PANICKING`.
const SPEED_MULTIPLIER_WHEN_PANICKING: f32 = 2.0;
/// Vanilla `FrogAi.SPEED_MULTIPLIER_WHEN_IDLING`.
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 1.0;
/// Vanilla `FrogAi.SPEED_MULTIPLIER_ON_LAND`.
const SPEED_MULTIPLIER_ON_LAND: f32 = 1.0;
/// Vanilla `FrogAi.SPEED_MULTIPLIER_IN_WATER`.
const SPEED_MULTIPLIER_IN_WATER: f32 = 0.75;
/// Vanilla `FrogAi.SPEED_MULTIPLIER_WHEN_TEMPTED`.
const SPEED_MULTIPLIER_WHEN_TEMPTED: f32 = 1.25;
/// Vanilla `FrogAi.TIME_BETWEEN_LONG_JUMPS`.
const TIME_BETWEEN_LONG_JUMPS: UniformIntProvider = UniformIntProvider {
    min_inclusive: 100,
    max_inclusive: 140,
};
/// Vanilla `FrogAi.MAX_LONG_JUMP_HEIGHT`.
const MAX_LONG_JUMP_HEIGHT: i32 = 2;
/// Vanilla `FrogAi.MAX_LONG_JUMP_WIDTH`.
const MAX_LONG_JUMP_WIDTH: i32 = 4;
/// Vanilla `FrogAi.MAX_JUMP_VELOCITY_MULTIPLIER`.
const MAX_JUMP_VELOCITY_MULTIPLIER: f32 = 3.571_428_8;
/// `BlockTags.FROG_PREFER_JUMP_TO` as a `&'static` tag key.
///
/// The registry exposes it as a `const`, whose reference is a temporary; the
/// long-jump behavior stores the key for its whole lifetime.
static FROG_PREFER_JUMP_TO_TAG: steel_utils::Identifier = BlockTag::FROG_PREFER_JUMP_TO;
/// Vanilla `FrogAi` `SetEntityLookTargetSometimes.create(EntityTypes.PLAYER,
/// 6.0F, UniformInt.of(30, 60))` arguments.
const LOOK_DISTANCE: f32 = 6.0;
/// Vanilla `FrogAi` look interval, `UniformInt.of(30, 60)`.
const LOOK_INTERVAL: UniformIntProvider = UniformIntProvider {
    min_inclusive: 30,
    max_inclusive: 60,
};

/// Vanilla `Frog.BRAIN_PROVIDER` sensor list.
static SENSOR_TYPES: [&SensorType; 5] = [
    &villager_sensors::NEAREST_LIVING_ENTITIES,
    &villager_sensors::HURT_BY,
    &sensing::frog_attackables::FROG_ATTACKABLES,
    &sensing::tempting::FROG_TEMPTATIONS,
    &sensing::is_in_water::IS_IN_WATER,
];

/// Vanilla `Frog.BRAIN_PROVIDER.makeBrain`.
///
/// The provider reads no entity state (its sensor and activity lists are
/// static), so the parameter only fixes the call shape the frog's own
/// `make_brain` uses.
#[must_use]
pub(crate) fn make_brain(_frog: &FrogEntity) -> Brain {
    Brain::new(&[], &SENSOR_TYPES, activities())
}

/// Vanilla `FrogAi.initMemories`: seed the long-jump cooldown at spawn.
///
/// Vanilla `UniformInt.sample` is `min + random.nextInt(max - min)`, i.e. the
/// upper bound is exclusive (100..140); [`UniformIntProvider`] is inclusive, so
/// the same range is sampled with an exclusive `rand` range.
pub(crate) fn init_memories(brain: &mut Brain) {
    let cooldown = rand::random_range(
        TIME_BETWEEN_LONG_JUMPS.min_inclusive..TIME_BETWEEN_LONG_JUMPS.max_inclusive,
    );
    brain.set_memory(&mem::LONG_JUMP_COOLDOWN_TICKS, Some(cooldown));
}

/// Vanilla `FrogAi.getActivities`.
#[must_use]
fn activities() -> Vec<ActivityData> {
    vec![
        core_activity(),
        idle_activity(),
        swim_activity(),
        lay_spawn_activity(),
        tongue_activity(),
        jump_activity(),
    ]
}

/// Vanilla `FrogAi.initCoreActivity`.
fn core_activity() -> ActivityData {
    ActivityData::create_priority(
        Activity::Core,
        0,
        vec![
            BehaviorBuilder::boxed(AnimalPanic::new(SPEED_MULTIPLIER_WHEN_PANICKING)),
            BehaviorBuilder::boxed(LookAtTargetSink::new(45, 90)),
            BehaviorBuilder::boxed(MoveToTargetSink::new()),
            BehaviorBuilder::boxed(CountDownCooldownTicks::new(&mem::TEMPTATION_COOLDOWN_TICKS)),
            BehaviorBuilder::boxed(CountDownCooldownTicks::new(&mem::LONG_JUMP_COOLDOWN_TICKS)),
        ],
    )
}

/// Vanilla `FrogAi.initIdleActivity`.
fn idle_activity() -> ActivityData {
    ActivityData::create_full(
        Activity::Idle,
        vec![
            (0, look_at_players()),
            (
                0,
                BehaviorBuilder::boxed(AnimalMakeLove::new(&vanilla_entities::FROG)),
            ),
            (1, follow_temptation()),
            (2, start_attacking()),
            (
                3,
                BehaviorBuilder::boxed(TryFindLand::create(6, SPEED_MULTIPLIER_ON_LAND)),
            ),
            (
                4,
                BehaviorBuilder::boxed(RunOne::with_entry_condition(
                    vec![(&mem::WALK_TARGET, MemoryStatus::ValueAbsent)],
                    vec![
                        (
                            BehaviorBuilder::boxed(RandomStroll::stroll(
                                SPEED_MULTIPLIER_WHEN_IDLING,
                            )),
                            1,
                        ),
                        (
                            BehaviorBuilder::boxed(SetWalkTargetFromLookTarget::create(
                                SPEED_MULTIPLIER_WHEN_IDLING,
                                3,
                            )),
                            1,
                        ),
                        (BehaviorBuilder::boxed(Croak::new()), 3),
                        (
                            BehaviorBuilder::boxed(BehaviorBuilder::create(TriggerIf::new(
                                |ctx: &mut BehaviorContext<'_>| ctx.body.on_ground(),
                            ))),
                            2,
                        ),
                    ],
                )),
            ),
        ],
        vec![
            (&mem::LONG_JUMP_MID_JUMP, MemoryStatus::ValueAbsent),
            (&mem::IS_IN_WATER, MemoryStatus::ValueAbsent),
        ],
        Vec::new(),
    )
}

/// Vanilla `FrogAi.initSwimActivity`.
fn swim_activity() -> ActivityData {
    ActivityData::create_full(
        Activity::Swim,
        vec![
            (0, look_at_players()),
            (1, follow_temptation()),
            (2, start_attacking()),
            (3, BehaviorBuilder::boxed(TryFindLand::create(8, 1.5))),
            (
                5,
                BehaviorBuilder::boxed(GateBehavior::new(
                    vec![(&mem::WALK_TARGET, MemoryStatus::ValueAbsent)],
                    Vec::new(),
                    OrderPolicy::Ordered,
                    RunningPolicy::TryAll,
                    vec![
                        (
                            BehaviorBuilder::boxed(RandomStroll::swim(SPEED_MULTIPLIER_IN_WATER)),
                            1,
                        ),
                        (
                            BehaviorBuilder::boxed(RandomStroll::stroll_with_restriction(
                                SPEED_MULTIPLIER_ON_LAND,
                                true,
                            )),
                            1,
                        ),
                        (
                            BehaviorBuilder::boxed(SetWalkTargetFromLookTarget::create(
                                SPEED_MULTIPLIER_ON_LAND,
                                3,
                            )),
                            1,
                        ),
                        (
                            BehaviorBuilder::boxed(BehaviorBuilder::create(TriggerIf::new(
                                |ctx: &mut BehaviorContext<'_>| ctx.body.is_in_water(),
                            ))),
                            5,
                        ),
                    ],
                )),
            ),
        ],
        vec![
            (&mem::LONG_JUMP_MID_JUMP, MemoryStatus::ValueAbsent),
            (&mem::IS_IN_WATER, MemoryStatus::ValuePresent),
        ],
        Vec::new(),
    )
}

/// Vanilla `FrogAi.initLaySpawnActivity`.
fn lay_spawn_activity() -> ActivityData {
    ActivityData::create_full(
        Activity::LaySpawn,
        vec![
            (0, look_at_players()),
            (1, start_attacking()),
            (
                2,
                BehaviorBuilder::boxed(TryFindLandNearLiquid::create(
                    8,
                    SPEED_MULTIPLIER_ON_LAND,
                    FluidTag::FROG_TRIES_TO_FIND_LAND_NEAR,
                )),
            ),
            (
                3,
                BehaviorBuilder::boxed(TryLaySpawnOnFluidNearLand::create(
                    vanilla_blocks::FROGSPAWN.default_state(),
                )),
            ),
            (
                4,
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
                        1,
                    ),
                    (BehaviorBuilder::boxed(Croak::new()), 2),
                    (
                        BehaviorBuilder::boxed(BehaviorBuilder::create(TriggerIf::new(
                            |ctx: &mut BehaviorContext<'_>| ctx.body.on_ground(),
                        ))),
                        1,
                    ),
                ])),
            ),
        ],
        vec![
            (&mem::LONG_JUMP_MID_JUMP, MemoryStatus::ValueAbsent),
            (&mem::IS_PREGNANT, MemoryStatus::ValuePresent),
        ],
        Vec::new(),
    )
}

/// Vanilla `FrogAi.initTongueActivity`: entering it requires `ATTACK_TARGET`,
/// and leaving it erases that memory.
fn tongue_activity() -> ActivityData {
    ActivityData::create_with_erase(
        Activity::Tongue,
        0,
        vec![
            BehaviorBuilder::boxed(StopAttackingIfTargetInvalid::create()),
            BehaviorBuilder::boxed(ShootTongue::new()),
        ],
        &mem::ATTACK_TARGET,
    )
}

/// Vanilla `FrogAi.initJumpActivity`.
fn jump_activity() -> ActivityData {
    ActivityData::create_full(
        Activity::LongJump,
        vec![
            (
                0,
                BehaviorBuilder::boxed(LongJumpMidJump::new(TIME_BETWEEN_LONG_JUMPS)),
            ),
            (
                1,
                BehaviorBuilder::boxed(
                    LongJumpToPreferredBlock::new(
                        TIME_BETWEEN_LONG_JUMPS,
                        MAX_LONG_JUMP_HEIGHT,
                        MAX_LONG_JUMP_WIDTH,
                        MAX_JUMP_VELOCITY_MULTIPLIER,
                        &FROG_PREFER_JUMP_TO_TAG,
                        0.5,
                    )
                    .with_acceptable_landing_spot(is_acceptable_landing_spot),
                ),
            ),
        ],
        vec![
            (&mem::TEMPTING_PLAYER, MemoryStatus::ValueAbsent),
            (&mem::BREED_TARGET, MemoryStatus::ValueAbsent),
            (&mem::LONG_JUMP_COOLDOWN_TICKS, MemoryStatus::ValueAbsent),
            (&mem::IS_IN_WATER, MemoryStatus::ValueAbsent),
        ],
        Vec::new(),
    )
}

/// Vanilla `Frog.customServerAiStep` brain half.
pub(crate) fn tick(brain: &mut Brain, level: &World, body: &SharedEntity) {
    brain.tick(level, body);
    update_activity(brain, body);
}

/// Vanilla `FrogAi.updateActivity`: tongue, spawn, long jump, swim, then idle.
pub(crate) fn update_activity(brain: &mut Brain, _body: &SharedEntity) {
    brain.set_active_activity_to_first_valid(&[
        Activity::Tongue,
        Activity::LaySpawn,
        Activity::LongJump,
        Activity::Swim,
        Activity::Idle,
    ]);
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

/// Vanilla `new FollowTemptation(s -> 1.25F)`.
fn follow_temptation() -> Box<dyn BehaviorControl> {
    BehaviorBuilder::boxed(FollowTemptation::new(SPEED_MULTIPLIER_WHEN_TEMPTED))
}

/// Vanilla `StartAttacking.create(FrogAi::canAttack, brain.getMemory(NEAREST_ATTACKABLE))`.
fn start_attacking() -> Box<dyn BehaviorControl> {
    BehaviorBuilder::boxed(StartAttacking::create_if(
        can_attack,
        nearest_attackable_target,
    ))
}

/// Vanilla `FrogAi.canAttack`: `!BehaviorUtils.isBreeding(mob)`, i.e. the body
/// has no `BREED_TARGET`.
fn can_attack(ctx: &BehaviorContext<'_>, _body: &SharedEntity) -> bool {
    !ctx.has_memory_value(&mem::BREED_TARGET)
}

/// Vanilla `StartAttacking` finder: the `FROG_ATTACKABLES` sensor's
/// `NEAREST_ATTACKABLE`.
fn nearest_attackable_target(
    ctx: &BehaviorContext<'_>,
    _body: &SharedEntity,
) -> Option<SharedEntity> {
    ctx.get_memory(&mem::NEAREST_ATTACKABLE)
}

/// Vanilla `FrogAi.isAcceptableLandingSpot`: reject landings in fluid, accept
/// the `FROG_PREFER_JUMP_TO` blocks outright, then mirror the trapdoor guard and
/// `LongJumpToRandomPos.defaultAcceptableLandingSpot`.
fn is_acceptable_landing_spot(body: &SharedEntity, target_pos: BlockPos) -> bool {
    use steel_registry::blocks::block_state_ext::BlockStateExt as _;
    use steel_registry::vanilla_block_tags::BlockTag;

    use crate::entity::ai::behavior::long_jump_to_random_pos::LongJumpToRandomPos;
    use crate::entity::ai::path::{PathType, PathfindingContext};
    use crate::world::LevelReader as _;

    let below = BlockPos::new(target_pos.x(), target_pos.y() - 1, target_pos.z());
    let above = BlockPos::new(target_pos.x(), target_pos.y() + 1, target_pos.z());
    let Some(world) = body.level() else {
        return false;
    };

    let state = world.get_block_state(target_pos);
    let state_below = world.get_block_state(below);
    if !state.get_fluid_state().is_empty()
        || !state_below.get_fluid_state().is_empty()
        || !world.get_block_state(above).get_fluid_state().is_empty()
    {
        return false;
    }

    if state.get_block().has_tag(&BlockTag::FROG_PREFER_JUMP_TO)
        || state_below
            .get_block()
            .has_tag(&BlockTag::FROG_PREFER_JUMP_TO)
    {
        return true;
    }

    let mut context = PathfindingContext::new(world.as_ref(), body.block_position());
    let path_type =
        context.get_path_type_from_state(target_pos.x(), target_pos.y(), target_pos.z());
    let path_type_below = context.get_path_type_from_state(below.x(), below.y(), below.z());

    // Vanilla: `pathType != TRAPDOOR && (!state.isAir() || pathTypeBelow !=
    // TRAPDOOR) ? defaultAcceptableLandingSpot : true`.
    if path_type != PathType::Trapdoor && (!state.is_air() || path_type_below != PathType::Trapdoor)
    {
        LongJumpToRandomPos::default_acceptable_landing_spot(body, target_pos)
    } else {
        true
    }
}
