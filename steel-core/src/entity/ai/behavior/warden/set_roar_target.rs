//! Vanilla `SetRoarTarget`: remember the entity the warden is angry at as its
//! roar target.

use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;

/// Vanilla `SetRoarTarget.create(targetFinderFunction)`.
///
/// Vanilla's finder is `Warden::getEntityAngryAt`, a getter over the warden
/// anger management. Steel behaviors receive the already-locked memory store
/// through [`BehaviorContext`], so the finder takes the context (never the body's
/// brain lock, which is held for the whole brain tick) and reads `ANGRY_AT` from
/// the borrowed memories instead.
pub struct SetRoarTarget<F> {
    target_finder: F,
}

impl<F> SetRoarTarget<F>
where
    F: Fn(&BehaviorContext<'_>, &SharedEntity) -> Option<SharedEntity> + Send + Sync,
{
    /// Vanilla `SetRoarTarget.create(targetFinderFunction)`.
    #[must_use]
    pub fn create(target_finder: F) -> OneShot<Self> {
        OneShot::new(Self { target_finder }).with_required_memories(vec![
            &mem::ROAR_TARGET,
            &mem::ATTACK_TARGET,
            &mem::CANT_REACH_WALK_TARGET_SINCE,
        ])
    }
}

impl<F> Trigger for SetRoarTarget<F>
where
    F: Fn(&BehaviorContext<'_>, &SharedEntity) -> Option<SharedEntity> + Send + Sync,
{
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if ctx.has_memory_value(&mem::ROAR_TARGET) || ctx.has_memory_value(&mem::ATTACK_TARGET) {
            return false;
        }

        let Some(target) = (self.target_finder)(ctx, ctx.body) else {
            return false;
        };

        if !super::can_target_entity(ctx, &target) {
            return false;
        }

        ctx.set_memory(&mem::ROAR_TARGET, target);
        ctx.erase_memory(&mem::CANT_REACH_WALK_TARGET_SINCE);
        true
    }
}
