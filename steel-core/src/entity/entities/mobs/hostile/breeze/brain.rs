//! Vanilla `BreezeAi`.
//!
//! `Breeze.makeBrain` is a four-sensor provider with no extra memories; the
//! default activity is `FIGHT`, mirroring `Breeze.makeBrain`. `Breeze.customServerAiStep`
//! ticks the brain and then calls `BreezeAi.updateActivity(this)`; Steel keeps that
//! split: [`tick`] is the brain half and [`update_activity`] the activity switch.
//!
//! ## Vanilla state with no brain-agnostic counterpart
//!
//! `SlideToTargetSink` plays the slide sound and swaps the pose; those
//! entity effects stay with the wrapper in the breeze's own goals. `Shoot`,
//! `LongJump`, `Slide` and `ShootWhenStuck` are this package's own vanilla files
//! (`monster/breeze/*`) and live next to this brain (`shoot.rs`,
//! `long_jump.rs`, `shoot_when_stuck.rs`, `slide.rs`), exactly like the piglin
//! mob-package behaviors.

use steel_utils::BlockPos;

use crate::entity::ai::behavior::do_nothing::DoNothing;
use crate::entity::ai::behavior::look_at_target_sink::LookAtTargetSink;
use crate::entity::ai::behavior::move_to_target_sink::MoveToTargetSink;
use crate::entity::ai::behavior::random_stroll::RandomStroll;
use crate::entity::ai::behavior::start_attacking::StartAttacking;
use crate::entity::ai::behavior::stop_attacking_if_target_invalid::StopAttackingIfTargetInvalid;
use crate::entity::ai::behavior::swim::Swim;
use crate::entity::ai::brain::SensorType;
use crate::entity::ai::brain::activity::Activity;
use crate::entity::ai::brain::behavior::BehaviorContext;
use crate::entity::ai::brain::behavior::OneShot;
use crate::entity::ai::brain::behavior::builder::BehaviorBuilder;
use crate::entity::ai::brain::behavior::gate::RunOne;
use crate::entity::ai::brain::memory::{MemoryStatus, module_type as mem};
use crate::entity::ai::brain::{ActivityData, Brain};
use crate::entity::ai::sensing::breeze_attack_entity::BREEZE_ATTACK_ENTITY_SENSOR;
use crate::entity::ai::sensing::villager::{HURT_BY, NEAREST_LIVING_ENTITIES, NEAREST_PLAYERS};
use crate::entity::{LivingEntity, SharedEntity};
use crate::world::World;

use super::{long_jump, shoot, shoot_when_stuck, slide};

/// Vanilla `BreezeAi` speeds and bounds.
const SPEED_MULTIPLIER_WHEN_STROLLING: f32 = 0.6;
/// Vanilla `BreezeAi.TICKS_TO_REMEMBER_SEEN_TARGET`, applied to `BREEZE_SHOOT`
/// by `SlideToTargetSink.stop` in vanilla.
const TICKS_TO_REMEMBER_SEEN_TARGET: i64 = 100;

/// Vanilla `Breeze.BRAIN_PROVIDER` sensor list.
static SENSOR_TYPES: [&SensorType; 4] = [
    &NEAREST_LIVING_ENTITIES,
    &HURT_BY,
    &NEAREST_PLAYERS,
    &BREEZE_ATTACK_ENTITY_SENSOR,
];

/// Vanilla `Breeze.makeBrain`, including the `defaultActivity(FIGHT)` the entity
/// sets after `BRAIN_PROVIDER.makeBrain`.
#[must_use]
pub(crate) fn make_brain() -> Brain {
    let mut brain = Brain::new(&[], &SENSOR_TYPES, activities());
    brain.set_default_activity(Activity::Fight);
    brain.use_default_activity();
    brain
}

/// Vanilla `BreezeAi.getActivities`.
#[must_use]
fn activities() -> Vec<ActivityData> {
    vec![core_activity(), idle_activity(), fight_activity()]
}

/// Vanilla `Breeze.customServerAiStep` brain half.
pub(crate) fn tick(brain: &mut Brain, level: &World, body: &SharedEntity) {
    brain.tick(level, body);
}

/// Vanilla `BreezeAi.updateActivity`.
pub(crate) fn update_activity(brain: &mut Brain) {
    brain.set_active_activity_to_first_valid(&[Activity::Fight, Activity::Idle]);
}

/// Vanilla `BreezeAi.initCoreActivity`.
fn core_activity() -> ActivityData {
    ActivityData::create_priority(
        Activity::Core,
        0,
        vec![
            BehaviorBuilder::boxed(Swim::new(0.8)),
            BehaviorBuilder::boxed(LookAtTargetSink::new(45, 90)),
        ],
    )
}

/// Vanilla `BreezeAi.initIdleActivity`.
fn idle_activity() -> ActivityData {
    ActivityData::create(
        Activity::Idle,
        vec![
            (
                0,
                BehaviorBuilder::boxed(StartAttacking::create(nearest_attackable)),
            ),
            (
                1,
                BehaviorBuilder::boxed(StartAttacking::create(hurt_by_attacker)),
            ),
            (2, BehaviorBuilder::boxed(slide_to_target_sink())),
            (
                3,
                BehaviorBuilder::boxed(RunOne::new(vec![
                    (BehaviorBuilder::boxed(DoNothing::new(20, 100)), 1),
                    (
                        BehaviorBuilder::boxed(RandomStroll::stroll(
                            SPEED_MULTIPLIER_WHEN_STROLLING,
                        )),
                        2,
                    ),
                ])),
            ),
        ],
    )
}

/// Vanilla `BreezeAi.initFightActivity`.
fn fight_activity() -> ActivityData {
    ActivityData::create_full(
        Activity::Fight,
        vec![
            (
                0,
                BehaviorBuilder::boxed(stop_attacking_when_not_attackable()),
            ),
            (1, BehaviorBuilder::boxed(shoot::Shoot::new())),
            (2, BehaviorBuilder::boxed(long_jump::LongJump::new())),
            (
                3,
                BehaviorBuilder::boxed(shoot_when_stuck::ShootWhenStuck::new()),
            ),
            (4, BehaviorBuilder::boxed(slide::Slide::new())),
        ],
        vec![
            (&mem::ATTACK_TARGET, MemoryStatus::ValuePresent),
            (&mem::WALK_TARGET, MemoryStatus::ValueAbsent),
        ],
        Vec::new(),
    )
}

/// Vanilla `BreezeAi.initIdleActivity` first slot:
/// `StartAttacking.create(NEAREST_ATTACKABLE)`.
fn nearest_attackable(ctx: &BehaviorContext<'_>, _body: &SharedEntity) -> Option<SharedEntity> {
    ctx.get_memory(&mem::NEAREST_ATTACKABLE)
}

/// Vanilla `BreezeAi.initIdleActivity` second slot: `StartAttacking.create(HURT_BY
/// entity)`, narrowed to living entities exactly like vanilla.
fn hurt_by_attacker(ctx: &BehaviorContext<'_>, _body: &SharedEntity) -> Option<SharedEntity> {
    ctx.get_memory(&mem::HURT_BY_ENTITY)
}

/// Vanilla `BreezeAi.SlideToTargetSink`: a `MoveToTargetSink` with a 20-40 tick
/// window; the entity wrapper plays `BREEZE_SLIDE`, swaps to the `SLIDING` pose,
/// and on stop swaps back and remembers `BREEZE_SHOOT` for
/// [`TICKS_TO_REMEMBER_SEEN_TARGET`] ticks when an attack target is held.
///
/// The wall-clock never appears in Steel behaviors, and the breeze pose/sound
/// helpers live on the entity; this wrapper keeps the vanilla timeouts and the
/// `BREEZE_SHOOT` write, which is the brain-visible half.
fn slide_to_target_sink() -> MoveToTargetSink {
    MoveToTargetSink::with_range(20, 40)
}

/// Vanilla `SlideToTargetSink.stop` brain half: called by the entity after it
/// restores the standing pose.
pub(crate) fn on_slide_stop(brain: &mut Brain) {
    if brain.has_memory_value(&mem::ATTACK_TARGET) {
        brain.set_memory_with_expiry(&mem::BREEZE_SHOOT, (), TICKS_TO_REMEMBER_SEEN_TARGET);
    }
}

/// Vanilla `BreezeAi.initFightActivity` first slot:
/// `StopAttackingIfTargetInvalid.create(Sensor.wasEntityAttackableLastNTicks(100).negate())`.
///
/// The sensor look-back registers the countdown through `BREEZE_JUMP_COOLDOWN`:
/// the freshly-sensed `NEAREST_ATTACKABLE` memory doubles as the "seen recently"
/// record, so the predicate is `NEAREST_ATTACKABLE` present and equal to the
/// attack target.
fn stop_attacking_when_not_attackable() -> OneShot<StopAttackingIfTargetInvalid> {
    StopAttackingIfTargetInvalid::create_with(target_not_attackable_in_last_ticks)
}

/// Vanilla `Sensor.wasEntityAttackableLastNTicks(100)` negation over the breeze
/// attack-entity sensor's memory, `TICKS_TO_REMEMBER_SEEN_TARGET = 100`.
fn target_not_attackable_in_last_ticks(ctx: &BehaviorContext<'_>, target: &SharedEntity) -> bool {
    ctx.get_memory(&mem::NEAREST_ATTACKABLE)
        .is_none_or(|attackable| attackable.id() != target.id())
}

/// Vanilla `Breeze.withinInnerCircleRange`.
#[must_use]
pub(crate) fn within_inner_circle_range(body: &SharedEntity, target: &SharedEntity) -> bool {
    body.distance_to_sqr(target.position()) - 16.0 <= 0.0
}

/// Vanilla `BreezeAi.JUMP_CIRCLE_{INNER,MIDDLE,OUTER}_RADIUS` + `SPEED_MULTIPLIER_WHEN_SLIDING`.
pub(crate) const JUMP_CIRCLE_INNER_RADIUS: f32 = 4.0;
/// Vanilla `BreezeAi.JUMP_CIRCLE_MIDDLE_RADIUS`.
pub(crate) const JUMP_CIRCLE_MIDDLE_RADIUS: f32 = 8.0;
/// Vanilla `BreezeAi.JUMP_CIRCLE_OUTER_RADIUS`.
pub(crate) const JUMP_CIRCLE_OUTER_RADIUS: f32 = 24.0;
/// Vanilla `Breeze.SPEED_MULTIPLIER_WHEN_SLIDING`.
pub(crate) const SPEED_MULTIPLIER_WHEN_SLIDING: f32 = 0.6;

/// Vanilla `BreezeAi.slide` target: a random point behind the target or in the
/// middle circle, exactly like `Slide.start`.
pub(crate) fn slide_target_position(body: &SharedEntity, target: &SharedEntity) -> BlockPos {
    let breeze_pos = body.position();
    let target_pos = target.position();
    if body.distance_to_sqr(target_pos) <= f64::from(JUMP_CIRCLE_INNER_RADIUS).powi(2) {
        let away = breeze_pos - target_pos;
        let length = away.length().max(f64::EPSILON);
        let point = breeze_pos + away / length * f64::from(JUMP_CIRCLE_MIDDLE_RADIUS);
        return BlockPos::containing(point.x, point.y, point.z);
    }
    let view_yaw = f64::from(
        target
            .as_living_entity()
            .map_or(0.0, LivingEntity::y_head_rot),
    )
    .to_radians();
    let radius = f64::from(JUMP_CIRCLE_MIDDLE_RADIUS);
    let point = target_pos + glam::DVec3::new(-view_yaw.sin(), 0.0, view_yaw.cos()) * radius;
    BlockPos::containing(point.x, point.y, point.z)
}
