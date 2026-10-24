//! Vanilla `BabyFollowAdult`: a baby trails the nearest remembered adult at a
//! distance sampled from a uniform range.

use std::sync::Arc;

use steel_utils::value_providers::UniformIntProvider;

use super::utils;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::memory::{MemoryModuleType, MemoryModuleTypeRef};
use crate::entity::ai::brain::tracker::{EntityTracker, WalkTarget};
use crate::entity::{LivingEntity, SharedEntity};

/// Vanilla `Function<LivingEntity, Float>` speed source.
#[derive(Clone, Copy)]
enum SpeedModifier {
    /// Vanilla `create(followRange, float speedModifier)`.
    Constant(f32),
    /// Vanilla `create(followRange, Function<LivingEntity, Float>, ...)`.
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

/// Vanilla `BabyFollowAdult`: while the body is a baby and the adult is between
/// `minInclusive` and `maxInclusive + 1` blocks away, walk to it.
pub struct BabyFollowAdult {
    nearest_visible_type: &'static MemoryModuleType<SharedEntity>,
    follow_range: UniformIntProvider,
    speed_modifier: SpeedModifier,
    target_eye: bool,
}

impl BabyFollowAdult {
    /// Vanilla `BabyFollowAdult.create(UniformInt followRange, float speedModifier)`.
    #[must_use]
    pub fn create(follow_range: UniformIntProvider, speed_modifier: f32) -> OneShot<Self> {
        Self::create_full(
            follow_range,
            SpeedModifier::Constant(speed_modifier),
            &mem::NEAREST_VISIBLE_ADULT,
            false,
        )
    }

    /// Vanilla `BabyFollowAdult.create(UniformInt followRange, Function<LivingEntity,
    /// Float> speedModifier, MemoryModuleType nearestVisibleType, boolean targetEye)`.
    ///
    /// Steel takes a plain function pointer so the behavior stays `Send + Sync`.
    #[must_use]
    pub fn create_with_speed_fn(
        follow_range: UniformIntProvider,
        speed_modifier: fn(&SharedEntity) -> f32,
        nearest_visible_type: &'static MemoryModuleType<SharedEntity>,
        target_eye: bool,
    ) -> OneShot<Self> {
        Self::create_full(
            follow_range,
            SpeedModifier::Function(speed_modifier),
            nearest_visible_type,
            target_eye,
        )
    }

    fn create_full(
        follow_range: UniformIntProvider,
        speed_modifier: SpeedModifier,
        nearest_visible_type: &'static MemoryModuleType<SharedEntity>,
        target_eye: bool,
    ) -> OneShot<Self> {
        OneShot::new(Self {
            nearest_visible_type,
            follow_range,
            speed_modifier,
            target_eye,
        })
        .with_required_memories(vec![
            nearest_visible_type as MemoryModuleTypeRef,
            &mem::LOOK_TARGET,
            &mem::WALK_TARGET,
        ])
    }
}

impl Trigger for BabyFollowAdult {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        let body = ctx.body;
        if !body.as_living_entity().is_some_and(LivingEntity::is_baby) {
            return false;
        }

        if ctx.has_memory_value(&mem::WALK_TARGET) {
            return false;
        }

        let Some(adult) = ctx.get_memory(self.nearest_visible_type) else {
            return false;
        };

        let min_inclusive = self.follow_range.min_inclusive;
        let max_inclusive = self.follow_range.max_inclusive;
        if !utils::closer_than(body, &adult, f64::from(max_inclusive + 1))
            || utils::closer_than(body, &adult, f64::from(min_inclusive))
        {
            return false;
        }

        let speed_modifier = self.speed_modifier.apply(body);
        ctx.set_memory(
            &mem::WALK_TARGET,
            WalkTarget::new(
                Arc::new(EntityTracker::with_target_eye_height(
                    SharedEntity::clone(&adult),
                    self.target_eye,
                    self.target_eye,
                )),
                speed_modifier,
                min_inclusive - 1,
            ),
        );
        utils::set_look_target(
            ctx,
            Arc::new(EntityTracker::with_target_eye_height(
                adult,
                true,
                self.target_eye,
            )),
        );
        true
    }
}
