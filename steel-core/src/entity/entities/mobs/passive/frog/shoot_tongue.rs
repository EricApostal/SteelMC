//! Vanilla `net.minecraft.world.entity.ai.behavior.ShootTongue`.
//!
//! The frog's `TONGUE` activity entry. `frog/brain.rs` (BrainMobs) registers it with
//! the `FROG_TONGUE`/`FROG_EAT` sounds; this module owns the behavior itself.

use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_data::EntityPose;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::sound_events;
use steel_utils::Downcast as _;

use super::FrogEntity;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStatus};
use crate::entity::ai::brain::tracker::WalkTarget;
use crate::entity::{Entity as _, RemovalReason, SharedEntity};

/// Vanilla `ShootTongue.TIME_OUT_DURATION`.
pub const TIME_OUT_DURATION: i32 = 100;
/// Vanilla `ShootTongue.CATCH_ANIMATION_DURATION`.
pub const CATCH_ANIMATION_DURATION: i32 = 6;
/// Vanilla `ShootTongue.TONGUE_ANIMATION_DURATION`.
pub const TONGUE_ANIMATION_DURATION: i32 = 10;
/// Vanilla `ShootTongue.UNREACHABLE_TONGUE_TARGETS_COOLDOWN_DURATION`.
pub const UNREACHABLE_TONGUE_TARGETS_COOLDOWN_DURATION: i64 = 100;
/// Vanilla `ShootTongue.MAX_UNREACHBLE_TONGUE_TARGETS_IN_MEMORY`.
pub const MAX_UNREACHABLE_TONGUE_TARGETS_IN_MEMORY: usize = 5;
/// Vanilla `ShootTongue.EATING_DISTANCE`.
const EATING_DISTANCE: f64 = 1.75;
/// Vanilla `ShootTongue.EATING_MOVEMENT_FACTOR`.
const EATING_MOVEMENT_FACTOR: f64 = 0.75;
/// Vanilla `ShootTongue.calculatePathCounter` reset value.
const CALCULATE_PATH_COUNTER: i32 = 10;
/// Vanilla `ShootTongue` logs both sounds at volume 2 and pitch 1.
const TONGUE_SOUND_VOLUME: f32 = 2.0;
const TONGUE_SOUND_PITCH: f32 = 1.0;

/// Vanilla `ShootTongue.State`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    MoveToTarget,
    CatchAnimation,
    EatAnimation,
    Done,
}

/// Vanilla `ShootTongue`: flick the tongue at the remembered attack target, pull it
/// in, and eat it.
pub(crate) struct ShootTongue {
    state: BehaviorState,
    eat_animation_timer: i32,
    calculate_path_counter: i32,
    inner_state: State,
    tongue_sound: SoundEventRef,
    eat_sound: SoundEventRef,
}

impl ShootTongue {
    /// Vanilla `new ShootTongue(SoundEvents.FROG_TONGUE, SoundEvents.FROG_EAT)`.
    ///
    /// `FrogAi.initTongueActivity` always constructs it with the frog sounds, so the
    /// sound pair is fixed here.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (
                        &mem::WALK_TARGET as MemoryModuleTypeRef,
                        MemoryStatus::ValueAbsent,
                    ),
                    (
                        &mem::LOOK_TARGET as MemoryModuleTypeRef,
                        MemoryStatus::Registered,
                    ),
                    (
                        &mem::ATTACK_TARGET as MemoryModuleTypeRef,
                        MemoryStatus::ValuePresent,
                    ),
                    (
                        &mem::IS_PANICKING as MemoryModuleTypeRef,
                        MemoryStatus::ValueAbsent,
                    ),
                ],
                TIME_OUT_DURATION,
                TIME_OUT_DURATION,
            ),
            eat_animation_timer: 0,
            calculate_path_counter: 0,
            inner_state: State::Done,
            tongue_sound: &sound_events::ENTITY_FROG_TONGUE,
            eat_sound: &sound_events::ENTITY_FROG_EAT,
        }
    }

    fn frog(body: &SharedEntity) -> Option<&FrogEntity> {
        body.downcast_ref::<FrogEntity>()
    }

    /// Vanilla `ShootTongue.canPathfindToTarget`.
    fn can_pathfind_to_target(body: &SharedEntity, target: &SharedEntity) -> bool {
        let Some(pathfinder) = body.as_pathfinder_mob() else {
            return false;
        };
        pathfinder
            .create_path_to(target.block_position(), 0)
            .is_some_and(|path| path.dist_to_target() < EATING_DISTANCE as f32)
    }

    /// Vanilla `ShootTongue.addUnreachableTargetToMemory`.
    fn add_unreachable_target(ctx: &mut BehaviorContext<'_>, target: &SharedEntity) {
        let mut unreachable = ctx
            .get_memory(&mem::UNREACHABLE_TONGUE_TARGETS)
            .unwrap_or_default();
        let should_add = !unreachable.contains(&target.uuid());
        if unreachable.len() == MAX_UNREACHABLE_TONGUE_TARGETS_IN_MEMORY && should_add {
            unreachable.remove(0);
        }
        if should_add {
            unreachable.push(target.uuid());
        }

        ctx.set_memory_with_expiry(
            &mem::UNREACHABLE_TONGUE_TARGETS,
            unreachable,
            UNREACHABLE_TONGUE_TARGETS_COOLDOWN_DURATION,
        );
    }

    /// Vanilla `ShootTongue.playEatingSound` + `eatEntity`.
    fn eat_entity(&self, ctx: &mut BehaviorContext<'_>, body: &SharedEntity) {
        ctx.level.play_sound_at(
            self.eat_sound,
            SoundSource::Neutral,
            body.position(),
            TONGUE_SOUND_VOLUME,
            TONGUE_SOUND_PITCH,
            None,
        );

        let Some(frog) = Self::frog(body) else {
            return;
        };
        let Some(target) = frog.tongue_target() else {
            return;
        };
        if !target.is_alive() {
            return;
        }

        if let Some(mob) = body.as_mob() {
            let _ = mob.do_hurt_target(body, ctx.level, &target);
        }
        if !target.is_alive() {
            target.set_removed(RemovalReason::Killed);
        }
    }
}

impl Behavior for ShootTongue {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    /// Vanilla `ShootTongue.checkExtraStartConditions`.
    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        let Some(target) = ctx.get_memory(&mem::ATTACK_TARGET) else {
            return false;
        };
        if Self::can_pathfind_to_target(ctx.body, &target) {
            return true;
        }

        // Vanilla erases the unreachable target and remembers it for the sensor.
        ctx.erase_memory(&mem::ATTACK_TARGET);
        Self::add_unreachable_target(ctx, &target);
        false
    }

    fn start(&mut self, _ctx: &mut BehaviorContext<'_>) {
        self.eat_animation_timer = 0;
        self.calculate_path_counter = CALCULATE_PATH_COUNTER;
        self.inner_state = State::MoveToTarget;
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        let body = ctx.body;
        let Some(target) = ctx.get_memory(&mem::ATTACK_TARGET) else {
            return;
        };
        let Some(frog) = Self::frog(body) else {
            return;
        };
        frog.set_tongue_target(&target);

        match self.inner_state {
            State::MoveToTarget => {
                let distance = target.position().distance(frog.position());
                if distance < EATING_DISTANCE {
                    ctx.level.play_sound_at(
                        self.tongue_sound,
                        SoundSource::Neutral,
                        frog.position(),
                        TONGUE_SOUND_VOLUME,
                        TONGUE_SOUND_PITCH,
                        None,
                    );
                    frog.set_pose(EntityPose::UsingTongue);

                    // Vanilla pulls the target towards the frog.
                    let offset = frog.position() - target.position();
                    if let Some(direction) = offset.try_normalize() {
                        target.set_velocity(direction * EATING_MOVEMENT_FACTOR);
                    }
                    self.eat_animation_timer = 0;
                    self.inner_state = State::CatchAnimation;
                } else if self.calculate_path_counter <= 0 {
                    ctx.set_memory(
                        &mem::WALK_TARGET,
                        WalkTarget::from_vec(target.position(), 2.0, 0),
                    );
                    self.calculate_path_counter = CALCULATE_PATH_COUNTER;
                } else {
                    self.calculate_path_counter -= 1;
                }
            }
            State::CatchAnimation => {
                self.eat_animation_timer += 1;
                if self.eat_animation_timer >= CATCH_ANIMATION_DURATION {
                    self.inner_state = State::EatAnimation;
                    self.eat_entity(ctx, body);
                }
            }
            State::EatAnimation => {
                if self.eat_animation_timer >= TONGUE_ANIMATION_DURATION {
                    self.inner_state = State::Done;
                } else {
                    self.eat_animation_timer += 1;
                }
            }
            State::Done => {}
        }
    }

    /// Vanilla `ShootTongue.stop`.
    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        ctx.erase_memory(&mem::ATTACK_TARGET);
        if let Some(frog) = Self::frog(ctx.body) {
            frog.erase_tongue_target();
            frog.set_pose(EntityPose::Standing);
        }
    }
}
