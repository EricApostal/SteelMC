//! Vanilla `StopAttackingIfTargetInvalid`: drop a stale attack target.

use std::sync::Arc;

use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;

/// Vanilla `StopAttackingIfTargetInvalid.TIMEOUT_TO_GET_WITHIN_ATTACK_RANGE`.
const TIMEOUT_TO_GET_WITHIN_ATTACK_RANGE: i64 = 200;

/// Vanilla `StopAttackingIfTargetInvalid.StopAttackCondition`.
pub type StopAttackCondition = fn(&BehaviorContext<'_>, &SharedEntity) -> bool;
/// Vanilla `StopAttackingIfTargetInvalid.TargetErasedCallback<E>`.
pub type TargetErasedCallback = fn(&mut BehaviorContext<'_>, &SharedEntity);

/// Vanilla do-nothing erased callback, `(_, _, _) -> {}`.
fn no_erased_callback(_: &mut BehaviorContext<'_>, _: &SharedEntity) {}

/// Vanilla `StopAttackingIfTargetInvalid`: always returns `true`, erasing
/// `ATTACK_TARGET` whenever the body can no longer attack it, the target left
/// the body's level, it died, the caller's predicate rejects it, or the body
/// has been stuck trying to reach it for 200 ticks.
pub struct StopAttackingIfTargetInvalid {
    stop_attacking_when: StopAttackCondition,
    on_target_erased: TargetErasedCallback,
    can_grow_tired_of_trying_to_reach_target: bool,
}

impl StopAttackingIfTargetInvalid {
    /// Vanilla `StopAttackingIfTargetInvalid.create()`.
    #[must_use]
    pub fn create() -> OneShot<Self> {
        Self::create_full(|_, _| false, no_erased_callback, true)
    }

    /// Vanilla `StopAttackingIfTargetInvalid.create(StopAttackCondition)`.
    #[must_use]
    pub fn create_with(stop_attacking_when: StopAttackCondition) -> OneShot<Self> {
        Self::create_full(stop_attacking_when, no_erased_callback, true)
    }

    /// Vanilla `StopAttackingIfTargetInvalid.create(StopAttackCondition,
    /// TargetErasedCallback, boolean canGrowTiredOfTryingToReachTarget)`.
    #[must_use]
    pub fn create_full(
        stop_attacking_when: StopAttackCondition,
        on_target_erased: TargetErasedCallback,
        can_grow_tired_of_trying_to_reach_target: bool,
    ) -> OneShot<Self> {
        OneShot::new(Self {
            stop_attacking_when,
            on_target_erased,
            can_grow_tired_of_trying_to_reach_target,
        })
        .with_required_memories(vec![
            &mem::ATTACK_TARGET,
            &mem::CANT_REACH_WALK_TARGET_SINCE,
        ])
    }

    /// Vanilla `StopAttackingIfTargetInvalid.isTiredOfTryingToReachTarget`.
    fn is_tired_of_trying_to_reach_target(&self, ctx: &BehaviorContext<'_>) -> bool {
        self.can_grow_tired_of_trying_to_reach_target
            && ctx
                .get_memory(&mem::CANT_REACH_WALK_TARGET_SINCE)
                .is_some_and(|cant_reach_since| {
                    ctx.level.game_time() - cant_reach_since > TIMEOUT_TO_GET_WITHIN_ATTACK_RANGE
                })
    }
}

/// Vanilla `target.level() == body.level()`, compared as world pointers.
fn same_level(body: &SharedEntity, target: &SharedEntity) -> bool {
    match (body.level(), target.level()) {
        (Some(body_level), Some(target_level)) => Arc::ptr_eq(&body_level, &target_level),
        _ => false,
    }
}

impl Trigger for StopAttackingIfTargetInvalid {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        let Some(target) = ctx.get_memory(&mem::ATTACK_TARGET) else {
            return false;
        };

        let target_alive = target.is_alive();
        let can_attack = target.as_living_entity().is_some_and(|living| {
            ctx.body
                .as_mob()
                .is_some_and(|mob| crate::entity::Mob::can_attack(mob, living))
        });

        let still_valid = can_attack
            && !self.is_tired_of_trying_to_reach_target(ctx)
            && target_alive
            && same_level(ctx.body, &target)
            && !(self.stop_attacking_when)(ctx, &target);

        if still_valid {
            return true;
        }

        (self.on_target_erased)(ctx, &target);
        ctx.erase_memory(&mem::ATTACK_TARGET);
        true
    }
}
