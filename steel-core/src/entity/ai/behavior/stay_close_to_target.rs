//! Vanilla `StayCloseToTarget`: keep a supplied position tracker within a band.

use std::sync::Arc;

use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::tracker::{PositionTracker, WalkTarget};

/// Vanilla `Function<LivingEntity, Optional<PositionTracker>>` target source.
///
/// Vanilla resolves this against the body's brain; Steel hands the behavior's
/// [`BehaviorContext`] instead, because the brain is already locked while a
/// behavior ticks.
pub type TargetPositionGetter = fn(&mut BehaviorContext<'_>) -> Option<Arc<dyn PositionTracker>>;
/// Vanilla `Predicate<LivingEntity>` run gate.
pub type ShouldRunPredicate = fn(&BehaviorContext<'_>) -> bool;

/// Vanilla `StayCloseToTarget.create(Function targetPositionGetter, Predicate
/// shouldRunPredicate, int closeEnough, int tooFar, float speedModifier)`:
/// re-targets the supplied tracker whenever the body drifts past `tooFar`.
pub struct StayCloseToTarget {
    target_position_getter: TargetPositionGetter,
    should_run_predicate: ShouldRunPredicate,
    close_enough: i32,
    too_far: i32,
    speed_modifier: f32,
}

impl StayCloseToTarget {
    /// Vanilla `StayCloseToTarget.create(Function, Predicate, int, int, float)`.
    #[must_use]
    pub fn create(
        target_position_getter: TargetPositionGetter,
        should_run_predicate: ShouldRunPredicate,
        close_enough: i32,
        too_far: i32,
        speed_modifier: f32,
    ) -> OneShot<Self> {
        OneShot::new(Self {
            target_position_getter,
            should_run_predicate,
            close_enough,
            too_far,
            speed_modifier,
        })
        .with_required_memories(vec![&mem::LOOK_TARGET, &mem::WALK_TARGET])
    }
}

impl Trigger for StayCloseToTarget {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        let Some(position_tracker) = (self.target_position_getter)(ctx) else {
            return false;
        };

        if !(self.should_run_predicate)(ctx) {
            return false;
        }

        // Vanilla `body.position().closerThan(positionTracker.currentPosition(), tooFar)`.
        let body = ctx.body;
        let too_far = f64::from(self.too_far);
        if body
            .position()
            .distance_squared(position_tracker.current_position())
            <= too_far * too_far
        {
            return false;
        }

        ctx.set_memory(&mem::LOOK_TARGET, position_tracker.clone());
        ctx.set_memory(
            &mem::WALK_TARGET,
            WalkTarget::new(position_tracker, self.speed_modifier, self.close_enough),
        );
        true
    }
}
