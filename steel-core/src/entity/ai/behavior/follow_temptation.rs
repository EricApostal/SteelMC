//! Vanilla `FollowTemptation`: follow the player holding the tempting item.

use std::sync::Arc;

use super::utils;
use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::tracker::{EntityTracker, WalkTarget};

/// Vanilla `FollowTemptation.TEMPTATION_COOLDOWN`.
pub const TEMPTATION_COOLDOWN: i32 = 100;
/// Vanilla `FollowTemptation.DEFAULT_CLOSE_ENOUGH_DIST`.
pub const DEFAULT_CLOSE_ENOUGH_DIST: f64 = 2.5;
/// Vanilla `FollowTemptation.BACKED_UP_CLOSE_ENOUGH_DIST`.
pub const BACKED_UP_CLOSE_ENOUGH_DIST: f64 = 3.5;
/// Vanilla `FollowTemptation.tick`'s `WalkTarget(..., 2)` close-enough distance.
const WALK_CLOSE_ENOUGH: i32 = 2;
/// Vanilla `FollowTemptation.timedOut` returns `false` unconditionally.
/// `BehaviorState` derives its timeout from a duration, so the largest duration
/// stands in for "never".
const NEVER_TIMES_OUT: i32 = i32::MAX;

/// Vanilla `Function<LivingEntity, Float>` speed source.
#[derive(Clone, Copy)]
enum SpeedModifier {
    /// Vanilla `new FollowTemptation(float speedModifier)`.
    Constant(f32),
    /// Vanilla `new FollowTemptation(Function<LivingEntity, Float> speedModifier)`.
    Function(fn(&SharedEntity) -> f32),
}

impl SpeedModifier {
    fn apply(self, body: &SharedEntity) -> f32 {
        match self {
            Self::Constant(speed_modifier) => speed_modifier,
            Self::Function(speed_modifier) => speed_modifier(body),
        }
    }
}

/// Vanilla `Function<LivingEntity, Double>` close-enough source.
#[derive(Clone, Copy)]
enum CloseEnoughDistance {
    /// Vanilla `new FollowTemptation(speedModifier)`.
    Constant(f64),
    /// Vanilla `new FollowTemptation(speedModifier, Function<LivingEntity, Double>)`.
    Function(fn(&SharedEntity) -> f64),
}

impl CloseEnoughDistance {
    fn apply(self, body: &SharedEntity) -> f64 {
        match self {
            Self::Constant(close_enough_distance) => close_enough_distance,
            Self::Function(close_enough_distance) => close_enough_distance(body),
        }
    }
}

/// Vanilla `FollowTemptation extends Behavior<PathfinderMob>`: walks to the
/// remembered `TEMPTING_PLAYER`, erasing the walk target once inside the
/// close-enough radius and applying a 100-tick temptation cooldown on stop.
pub struct FollowTemptation {
    state: BehaviorState,
    speed_modifier: SpeedModifier,
    close_enough_distance: CloseEnoughDistance,
    look_in_the_eyes: bool,
}

impl FollowTemptation {
    /// Vanilla `new FollowTemptation(float speedModifier)`.
    #[must_use]
    pub fn new(speed_modifier: f32) -> Self {
        Self::build(
            SpeedModifier::Constant(speed_modifier),
            CloseEnoughDistance::Constant(DEFAULT_CLOSE_ENOUGH_DIST),
            false,
        )
    }

    fn build(
        speed_modifier: SpeedModifier,
        close_enough_distance: CloseEnoughDistance,
        look_in_the_eyes: bool,
    ) -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (&mem::LOOK_TARGET, MemoryStatus::Registered),
                    (&mem::WALK_TARGET, MemoryStatus::Registered),
                    (&mem::TEMPTATION_COOLDOWN_TICKS, MemoryStatus::ValueAbsent),
                    (&mem::TEMPTING_PLAYER, MemoryStatus::ValuePresent),
                    (&mem::BREED_TARGET, MemoryStatus::ValueAbsent),
                    (&mem::IS_PANICKING, MemoryStatus::ValueAbsent),
                ],
                NEVER_TIMES_OUT,
                NEVER_TIMES_OUT,
            ),
            speed_modifier,
            close_enough_distance,
            look_in_the_eyes,
        }
    }

    /// Vanilla `new FollowTemptation(Function<LivingEntity, Float> speedModifier)`.
    ///
    /// Steel takes a plain function pointer so the behavior stays `Send + Sync`.
    #[must_use]
    pub fn with_speed_fn(speed_modifier: fn(&SharedEntity) -> f32) -> Self {
        Self::build(
            SpeedModifier::Function(speed_modifier),
            CloseEnoughDistance::Constant(DEFAULT_CLOSE_ENOUGH_DIST),
            false,
        )
    }

    /// Vanilla `new FollowTemptation(Function<LivingEntity, Float>,
    /// Function<LivingEntity, Double>, boolean lookInTheEyes)`.
    #[must_use]
    pub fn create_full(
        speed_modifier: fn(&SharedEntity) -> f32,
        close_enough_distance: fn(&SharedEntity) -> f64,
        look_in_the_eyes: bool,
    ) -> Self {
        Self::build(
            SpeedModifier::Function(speed_modifier),
            CloseEnoughDistance::Function(close_enough_distance),
            look_in_the_eyes,
        )
    }
}

impl Behavior for FollowTemptation {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.has_memory_value(&mem::TEMPTING_PLAYER)
            && !ctx.has_memory_value(&mem::BREED_TARGET)
            && !ctx.has_memory_value(&mem::IS_PANICKING)
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        let Some(player) = ctx.get_memory(&mem::TEMPTING_PLAYER) else {
            return;
        };

        utils::set_look_target(ctx, Arc::new(EntityTracker::new(player.clone(), true)));

        let body = ctx.body;
        let close_enough = self.close_enough_distance.apply(body);
        if body.distance_to_sqr(player.position()) < close_enough * close_enough {
            ctx.erase_memory(&mem::WALK_TARGET);
            return;
        }

        let speed_modifier = self.speed_modifier.apply(body);
        ctx.set_memory(
            &mem::WALK_TARGET,
            WalkTarget::new(
                Arc::new(EntityTracker::with_target_eye_height(
                    player,
                    self.look_in_the_eyes,
                    self.look_in_the_eyes,
                )),
                speed_modifier,
                WALK_CLOSE_ENOUGH,
            ),
        );
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        ctx.set_memory(&mem::TEMPTATION_COOLDOWN_TICKS, TEMPTATION_COOLDOWN);
        ctx.erase_memory(&mem::WALK_TARGET);
        ctx.erase_memory(&mem::LOOK_TARGET);
    }
}
