//! Vanilla `StopBeingAngryIfTargetDead`: drop anger once the target is gone.

use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;
use steel_registry::vanilla_entities;
use steel_registry::vanilla_game_rules::FORGIVE_DEAD_PLAYERS;

/// Vanilla `StopBeingAngryIfTargetDead.create()`: erases `ANGRY_AT` when the
/// world-level entity behind it is a dead or dying living entity, forgiving dead
/// players only while `FORGIVE_DEAD_PLAYERS` is enabled.
pub struct StopBeingAngryIfTargetDead;

impl StopBeingAngryIfTargetDead {
    /// Vanilla `StopBeingAngryIfTargetDead.create()`.
    #[must_use]
    pub fn create() -> OneShot<Self> {
        OneShot::new(Self).with_required_memories(vec![&mem::ANGRY_AT])
    }
}

impl Trigger for StopBeingAngryIfTargetDead {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        let Some(angry_at) = ctx.get_memory(&mem::ANGRY_AT) else {
            return false;
        };

        if let Some(entity) = ctx.level.get_entity_by_uuid(&angry_at)
            && is_dead_living(&entity)
            && is_forgivable(ctx, &entity)
        {
            ctx.erase_memory(&mem::ANGRY_AT);
        }

        true
    }
}

/// Vanilla `entity instanceof LivingEntity ? (LivingEntity) entity : null`
/// followed by `LivingEntity.isDeadOrDying`.
fn is_dead_living(entity: &SharedEntity) -> bool {
    entity
        .as_living_entity()
        .is_some_and(crate::entity::LivingEntity::is_dead_or_dying)
}

/// Vanilla `.filter(angerTarget -> !angerTarget.is(EntityTypes.PLAYER) ||
/// level.getGameRules().get(GameRules.FORGIVE_DEAD_PLAYERS))`.
fn is_forgivable(ctx: &BehaviorContext<'_>, entity: &SharedEntity) -> bool {
    entity.entity_type() != &vanilla_entities::PLAYER
        || ctx.level.get_game_rule(&FORGIVE_DEAD_PLAYERS)
}
