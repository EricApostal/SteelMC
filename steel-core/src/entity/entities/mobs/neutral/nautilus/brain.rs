//! Vanilla `NautilusAi` and `ZombieNautilusAi` plus the two `BRAIN_PROVIDER`s.
//!
//! Both concrete nautili share the sensor set
//! (`NEAREST_LIVING_ENTITIES`, `NEAREST_ADULT`, `NEAREST_PLAYERS`, `HURT_BY`,
//! `NAUTILUS_TEMPTATIONS`) and the `CORE`/`IDLE`/`FIGHT` activity layout; only the
//! speed modifiers, the breeding entry and the charge speed differ, exactly as in
//! vanilla.
//!
//! ## Deviations
//!
//! - `SensorType.NAUTILUS_TEMPTATIONS` has no Steel equivalent, so
//!   [`NAUTILUS_TEMPTATIONS`] is built here from the shared `TemptingSensor` with
//!   `NautilusAi.getTemptations()` (`#NAUTILUS_FOOD`).
//! - Vanilla `ChargeAttack`'s `chargeSound` argument has no Steel behavior-layer
//!   equivalent (see `ai/behavior/charge_attack.rs`), so the dash sound is played
//!   by the entity's own `getDashSound` path instead.
//! - `Sensor.isEntityAttackableIgnoringLineOfSight` is reused from the piglin AI
//!   module, which implements the same `forCombat().range(FOLLOW_RANGE)
//!   .ignoreLineOfSight()` test.

use steel_registry::vanilla_entities;
use steel_registry::vanilla_entity_type_tags::EntityTypeTag;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{REGISTRY, TaggedRegistryExt as _, vanilla_game_rules::MOB_GRIEFING};
use steel_utils::value_providers::UniformIntProvider;

use crate::entity::ai::behavior::animal_make_love::AnimalMakeLove;
use crate::entity::ai::behavior::animal_panic::AnimalPanic;
use crate::entity::ai::behavior::charge_attack::ChargeAttack;
use crate::entity::ai::behavior::count_down_cooldown_ticks::CountDownCooldownTicks;
use crate::entity::ai::behavior::follow_temptation::FollowTemptation;
use crate::entity::ai::behavior::look_at_target_sink::LookAtTargetSink;
use crate::entity::ai::behavior::move_to_target_sink::MoveToTargetSink;
use crate::entity::ai::behavior::piglin::ai::is_entity_attackable_ignoring_line_of_sight;
use crate::entity::ai::behavior::random_stroll::RandomStroll;
use crate::entity::ai::behavior::set_walk_target_from_look_target::SetWalkTargetFromLookTarget;
use crate::entity::ai::brain::SensorType;
use crate::entity::ai::brain::activity::Activity;
use crate::entity::ai::brain::behavior::builder::BehaviorBuilder;
use crate::entity::ai::brain::behavior::gate::{GateBehavior, OrderPolicy, RunningPolicy};
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::memory::nearest_visible::NearestVisibleLivingEntities;
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStatus};
use crate::entity::ai::brain::sensor::BrainSensor;
use crate::entity::ai::brain::{ActivityData, Brain};
use crate::entity::ai::goal::Tameable;
use crate::entity::ai::sensing;
use crate::entity::ai::sensing::tempting::TemptingSensor;
use crate::entity::ai::sensing::villager as villager_sensors;
use crate::entity::ai::targeting::TargetingConditions;
use crate::entity::{AgeableMob, Entity, Mob, SharedEntity};
use crate::world::World;

/// Vanilla `NautilusAi.TIME_BETWEEN_NON_PLAYER_ATTACKS`.
const TIME_BETWEEN_NON_PLAYER_ATTACKS: UniformIntProvider = UniformIntProvider {
    min_inclusive: 2400,
    max_inclusive: 3600,
};
/// Vanilla `NautilusAi.ANGER_DURATION`.
const ANGER_DURATION: i64 = 400;
/// Vanilla `NautilusAi.TIME_BETWEEN_ATTACKS` (also used by `ZombieNautilusAi`).
const TIME_BETWEEN_ATTACKS: i32 = 80;
/// Vanilla `NautilusAi.MAX_CHARGE_DISTANCE`.
const MAX_CHARGE_DISTANCE: f64 = 12.0;
/// Vanilla `NautilusAi.MAX_TARGET_DETECTION_DISTANCE`.
const MAX_TARGET_DETECTION_DISTANCE: f64 = 11.0;
/// Vanilla `NautilusAi.ATTACK_KNOCKBACK_FORCE`.
const ATTACK_KNOCKBACK_FORCE: f32 = 2.0;
/// Vanilla `NautilusAi.SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER`.
const SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER: f32 = 1.0;
/// Vanilla `NautilusAi.SPEED_MULTIPLIER_WHEN_TEMPTED`.
const SPEED_MULTIPLIER_WHEN_TEMPTED: f32 = 1.3;
/// Vanilla `NautilusAi.SPEED_MULTIPLIER_WHEN_MAKING_LOVE`.
const SPEED_MULTIPLIER_WHEN_MAKING_LOVE: f32 = 0.4;
/// Vanilla `NautilusAi.SPEED_MULTIPLIER_WHEN_PANICKING`.
const SPEED_MULTIPLIER_WHEN_PANICKING: f32 = 1.6;
/// Vanilla `NautilusAi.SPEED_WHEN_ATTACKING`.
const SPEED_WHEN_ATTACKING: f32 = 0.6;
/// Vanilla `ZombieNautilusAi.SPEED_MULTIPLIER_WHEN_TEMPTED`.
const ZOMBIE_SPEED_MULTIPLIER_WHEN_TEMPTED: f32 = 0.9;
/// Vanilla `ZombieNautilusAi.SPEED_WHEN_ATTACKING`.
const ZOMBIE_SPEED_WHEN_ATTACKING: f32 = 0.5;
/// Vanilla `AnimalMakeLove(EntityTypes.NAUTILUS, 0.4F, 2)` close-enough distance.
const MAKE_LOVE_CLOSE_ENOUGH_DISTANCE: i32 = 2;

/// Vanilla `Nautilus.BRAIN_PROVIDER` / `ZombieNautilus.BRAIN_PROVIDER` memory list.
static MEMORY_TYPES: [MemoryModuleTypeRef; 2] = [&mem::ANGRY_AT, &mem::ATTACK_TARGET_COOLDOWN];

/// Vanilla `SensorType.NAUTILUS_TEMPTATIONS` (`new TemptingSensor(NautilusAi.getTemptations())`).
static NAUTILUS_TEMPTATIONS: SensorType =
    SensorType::new(create_nautilus_temptations, "nautilus_temptations");

/// Vanilla `SensorType.NAUTILUS_TEMPTATIONS` factory.
fn create_nautilus_temptations() -> Box<dyn BrainSensor> {
    Box::new(TemptingSensor::new(|_body, item_stack| {
        REGISTRY
            .items
            .is_in_tag(item_stack.item, &ItemTag::NAUTILUS_FOOD)
    }))
}

/// Vanilla `Nautilus.BRAIN_PROVIDER` sensor list.
static SENSOR_TYPES: [&SensorType; 5] = [
    &villager_sensors::NEAREST_LIVING_ENTITIES,
    &sensing::adult::NEAREST_ADULT,
    &villager_sensors::NEAREST_PLAYERS,
    &villager_sensors::HURT_BY,
    &NAUTILUS_TEMPTATIONS,
];

/// Vanilla `Nautilus.makeBrain` -> `BRAIN_PROVIDER.makeBrain`.
#[must_use]
pub(crate) fn make_brain() -> Brain {
    Brain::new(&MEMORY_TYPES, &SENSOR_TYPES, activities())
}

/// Vanilla `ZombieNautilus.makeBrain` -> `BRAIN_PROVIDER.makeBrain`.
#[must_use]
pub(crate) fn make_zombie_brain() -> Brain {
    Brain::new(&MEMORY_TYPES, &SENSOR_TYPES, zombie_activities())
}

/// Vanilla `NautilusAi.initMemories` / the shared `AbstractNautilus.finalizeSpawn`
/// call: seed the non-player attack cooldown.
///
/// `UniformInt.sample` is `min + random.nextInt(max - min)`, i.e. the upper bound
/// is exclusive, so the range is sampled exclusively.
pub(crate) fn init_memories(brain: &mut Brain) {
    let cooldown = rand::random_range(
        TIME_BETWEEN_NON_PLAYER_ATTACKS.min_inclusive
            ..TIME_BETWEEN_NON_PLAYER_ATTACKS.max_inclusive,
    );
    brain.set_memory(&mem::ATTACK_TARGET_COOLDOWN, Some(cooldown));
}

/// Vanilla `NautilusAi.getActivities`.
#[must_use]
fn activities() -> Vec<ActivityData> {
    vec![
        core_activity(true),
        idle_activity(true),
        fight_activity(SPEED_WHEN_ATTACKING),
    ]
}

/// Vanilla `ZombieNautilusAi.getActivities`.
#[must_use]
fn zombie_activities() -> Vec<ActivityData> {
    vec![
        core_activity(false),
        idle_activity(false),
        fight_activity(ZOMBIE_SPEED_WHEN_ATTACKING),
    ]
}

/// Vanilla `NautilusAi.initCoreActivity` / `ZombieNautilusAi.initCoreActivity`.
///
/// The zombie nautilus registers no `AnimalPanic`.
fn core_activity(include_panic: bool) -> ActivityData {
    let mut behaviors = Vec::new();
    if include_panic {
        behaviors.push(BehaviorBuilder::boxed(AnimalPanic::new(
            SPEED_MULTIPLIER_WHEN_PANICKING,
        )));
    }
    behaviors.push(BehaviorBuilder::boxed(LookAtTargetSink::new(45, 90)));
    behaviors.push(BehaviorBuilder::boxed(MoveToTargetSink::new()));
    behaviors.push(BehaviorBuilder::boxed(CountDownCooldownTicks::new(
        &mem::TEMPTATION_COOLDOWN_TICKS,
    )));
    behaviors.push(BehaviorBuilder::boxed(CountDownCooldownTicks::new(
        &mem::CHARGE_COOLDOWN_TICKS,
    )));
    behaviors.push(BehaviorBuilder::boxed(CountDownCooldownTicks::new(
        &mem::ATTACK_TARGET_COOLDOWN,
    )));

    ActivityData::create_priority(Activity::Core, 0, behaviors)
}

/// Vanilla `NautilusAi.initIdleActivity` / `ZombieNautilusAi.initIdleActivity`.
fn idle_activity(with_breeding: bool) -> ActivityData {
    let mut behaviors = Vec::new();

    if with_breeding {
        behaviors.push(BehaviorBuilder::boxed(AnimalMakeLove::with_speed(
            &vanilla_entities::NAUTILUS,
            SPEED_MULTIPLIER_WHEN_MAKING_LOVE,
            MAKE_LOVE_CLOSE_ENOUGH_DISTANCE,
        )));
    }

    behaviors.push(BehaviorBuilder::boxed(FollowTemptation::create_full(
        if with_breeding {
            speed_modifier_tempted
        } else {
            speed_modifier_zombie_tempted
        },
        close_enough_distance_tempted,
        false,
    )));
    behaviors.push(BehaviorBuilder::boxed(OneShot::new(NautilusStartAttacking)));
    behaviors.push(BehaviorBuilder::boxed(GateBehavior::new(
        vec![(&mem::WALK_TARGET, MemoryStatus::ValueAbsent)],
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
                    SPEED_MULTIPLIER_WHEN_IDLING_IN_WATER,
                    3,
                )),
                3,
            ),
        ],
    )));

    ActivityData::create_priority(Activity::Idle, 1, behaviors)
}

/// Vanilla `NautilusAi.initFightActivity` / `ZombieNautilusAi.initFightActivity`.
fn fight_activity(speed: f32) -> ActivityData {
    ActivityData::create_full(
        Activity::Fight,
        vec![(
            0,
            BehaviorBuilder::boxed(ChargeAttack::new(
                TIME_BETWEEN_ATTACKS,
                attack_target_conditions(),
                speed,
                ATTACK_KNOCKBACK_FORCE,
                MAX_CHARGE_DISTANCE,
                MAX_TARGET_DETECTION_DISTANCE,
            )),
        )],
        vec![
            (&mem::ATTACK_TARGET, MemoryStatus::ValuePresent),
            (&mem::TEMPTING_PLAYER, MemoryStatus::ValueAbsent),
            (&mem::BREED_TARGET, MemoryStatus::ValueAbsent),
            (&mem::CHARGE_COOLDOWN_TICKS, MemoryStatus::ValueAbsent),
        ],
        Vec::new(),
    )
}

/// Vanilla `NautilusAi.ATTACK_TARGET_CONDITIONS`.
fn attack_target_conditions() -> TargetingConditions {
    TargetingConditions::for_combat().selector(|target, level| {
        (level.get_game_rule(&MOB_GRIEFING)
            || target.entity_type() != &vanilla_entities::ARMOR_STAND)
            && level
                .world_border_snapshot()
                .is_within_bounds(target.bounding_box())
    })
}

/// Vanilla `StartAttacking.create(NautilusAi::findNearestValidAttackTarget)`.
///
/// Vanilla's `findNearestValidAttackTarget` writes `ATTACK_TARGET_COOLDOWN`, so it needs
/// a mutable behavior context; Steel's shared `StartAttacking.TargetFinder` is a
/// read-only function pointer, hence this local trigger that mirrors
/// `StartAttacking.trigger` and adds the vanilla cooldown write. It replaces
/// `StartAttacking::create` for both nautilus brains.
pub(crate) struct NautilusStartAttacking;

impl Trigger for NautilusStartAttacking {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        // Vanilla `StartAttacking.trigger` entry condition: `absent(ATTACK_TARGET)`.
        if ctx.check_memory(&mem::ATTACK_TARGET, MemoryStatus::ValuePresent) {
            return false;
        }

        let Some(target) = find_nearest_valid_attack_target(ctx) else {
            return false;
        };

        let can_attack = target.as_living_entity().is_some_and(|living| {
            ctx.body
                .as_mob()
                .is_some_and(|mob| Mob::can_attack(mob, living))
        });
        if !can_attack {
            return false;
        }

        ctx.set_memory(&mem::ATTACK_TARGET, target);
        ctx.erase_memory(&mem::CANT_REACH_WALK_TARGET_SINCE);
        true
    }
}

/// Vanilla `NautilusAi.findNearestValidAttackTarget`.
fn find_nearest_valid_attack_target(ctx: &mut BehaviorContext<'_>) -> Option<SharedEntity> {
    let body = ctx.body;
    let is_baby = body.as_ageable_mob().is_some_and(AgeableMob::is_baby);
    let is_tame = body.as_tameable().is_some_and(Tameable::is_tame);
    if ctx.has_memory_value(&mem::BREED_TARGET) || !body.is_in_water() || is_baby || is_tame {
        return None;
    }

    if let Some(angry_at) = angry_at(ctx)
        && is_entity_attackable_ignoring_line_of_sight(
            ctx.level,
            body.as_entity_event_source(),
            &angry_at,
        )
    {
        return Some(angry_at);
    }

    if ctx.has_memory_value(&mem::ATTACK_TARGET_COOLDOWN) {
        return None;
    }

    ctx.set_memory(
        &mem::ATTACK_TARGET_COOLDOWN,
        rand::random_range(
            TIME_BETWEEN_NON_PLAYER_ATTACKS.min_inclusive
                ..TIME_BETWEEN_NON_PLAYER_ATTACKS.max_inclusive,
        ),
    );

    if rand::random::<f32>() < 0.5 {
        return None;
    }

    ctx.get_memory(&mem::NEAREST_VISIBLE_LIVING_ENTITIES)
        .unwrap_or_else(NearestVisibleLivingEntities::empty)
        .find_closest(is_hostile_target)
}

/// Vanilla `BehaviorUtils.getLivingEntityFromUUIDMemory(ANGRY_AT)`.
fn angry_at(ctx: &BehaviorContext<'_>) -> Option<SharedEntity> {
    let uuid = ctx.get_memory(&mem::ANGRY_AT)?;
    let target = ctx.level.get_entity_by_uuid(&uuid)?;
    target.as_living_entity()?;
    Some(target)
}

/// Vanilla `NautilusAi.isHostileTarget`.
fn is_hostile_target(entity: &SharedEntity) -> bool {
    entity.is_in_water()
        && REGISTRY
            .entity_types
            .is_in_tag(entity.entity_type(), &EntityTypeTag::NAUTILUS_HOSTILES)
}

/// Vanilla `NautilusAi.setAngerTarget`.
///
/// Runs outside the brain tick (damage), so it locks the brain itself.
pub(crate) fn set_anger_target(level: &World, body: &dyn Entity, target: &SharedEntity) {
    if !is_entity_attackable_ignoring_line_of_sight(level, body, target) {
        return;
    }

    let Some(brain) = body.as_mob().and_then(Mob::brain) else {
        return;
    };
    let mut brain = brain.lock();
    brain.erase_memory(&mem::CANT_REACH_WALK_TARGET_SINCE);
    brain.set_memory_with_expiry(&mem::ANGRY_AT, target.uuid(), ANGER_DURATION);
}

/// Vanilla `NautilusAi.updateActivity` / `ZombieNautilusAi.updateActivity`.
pub(crate) fn update_activity(brain: &mut Brain) {
    brain.set_active_activity_to_first_valid(&[Activity::Fight, Activity::Idle]);
}

/// Vanilla `Nautilus.customServerAiStep` brain half (brain tick + activity update).
pub(crate) fn tick(brain: &mut Brain, level: &World, body: &SharedEntity) {
    brain.tick(level, body);
    update_activity(brain);
}

/// Vanilla `NautilusAi.getTemptations` speed: `mob -> 1.3F`.
fn speed_modifier_tempted(_body: &SharedEntity) -> f32 {
    SPEED_MULTIPLIER_WHEN_TEMPTED
}

/// Vanilla `ZombieNautilusAi` temptation speed: `mob -> 0.9F`.
fn speed_modifier_zombie_tempted(_body: &SharedEntity) -> f32 {
    ZOMBIE_SPEED_MULTIPLIER_WHEN_TEMPTED
}

/// Vanilla `mob -> mob.isBaby() ? 2.5 : 3.5`.
fn close_enough_distance_tempted(body: &SharedEntity) -> f64 {
    if body.as_ageable_mob().is_some_and(AgeableMob::is_baby) {
        2.5
    } else {
        3.5
    }
}
