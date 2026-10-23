//! Vanilla `StartAttacking`: remember the finder-supplied attack target.

use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;

/// Vanilla `StartAttacking.TargetFinder<E>`.
pub type TargetFinder = fn(&BehaviorContext<'_>, &SharedEntity) -> Option<SharedEntity>;
/// Vanilla `StartAttacking.StartAttackingCondition<E>`.
pub type StartAttackingCondition = fn(&BehaviorContext<'_>, &SharedEntity) -> bool;

/// Vanilla `StartAttacking.create(TargetFinder)`: records the found entity in
/// `ATTACK_TARGET` when the body may attack it, then drops any
/// `CANT_REACH_WALK_TARGET_SINCE` record so the new chase starts untimed.
///
/// Vanilla passes `(ServerLevel, body)` to the callbacks; Steel passes the
/// [`BehaviorContext`] (backed by the body's own brain lock) so callbacks can
/// read memories without deadlocking on the brain mutex.
pub struct StartAttacking {
    can_attack_predicate: StartAttackingCondition,
    target_finder: TargetFinder,
}

impl StartAttacking {
    /// Vanilla `StartAttacking.create(TargetFinder)`.
    #[must_use]
    pub fn create(target_finder: TargetFinder) -> OneShot<Self> {
        Self::create_if(|_, _| true, target_finder)
    }

    /// Vanilla `StartAttacking.create(StartAttackingCondition, TargetFinder)`.
    #[must_use]
    pub fn create_if(
        can_attack_predicate: StartAttackingCondition,
        target_finder: TargetFinder,
    ) -> OneShot<Self> {
        OneShot::new(Self {
            can_attack_predicate,
            target_finder,
        })
        .with_required_memories(vec![
            &mem::ATTACK_TARGET,
            &mem::CANT_REACH_WALK_TARGET_SINCE,
        ])
    }
}

impl Trigger for StartAttacking {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        // Vanilla entry condition: `absent(ATTACK_TARGET)`.
        if ctx.check_memory(&mem::ATTACK_TARGET, MemoryStatus::ValuePresent) {
            return false;
        }

        if !(self.can_attack_predicate)(ctx, ctx.body) {
            return false;
        }

        let Some(target) = (self.target_finder)(ctx, ctx.body) else {
            return false;
        };

        // Vanilla `body.canAttack(targetEntity)` (a `Mob` capability over the
        // vanilla `LivingEntity` view of the target).
        let can_attack = target.as_living_entity().is_some_and(|living| {
            ctx.body
                .as_mob()
                .is_some_and(|mob| crate::entity::Mob::can_attack(mob, living))
        });
        if !can_attack {
            return false;
        }

        ctx.set_memory(&mem::ATTACK_TARGET, target);
        ctx.erase_memory(&mem::CANT_REACH_WALK_TARGET_SINCE);
        true
    }
}
