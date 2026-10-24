//! Vanilla `StartHuntingHoglin`.

use super::ai;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::{LivingEntity, Mob, SharedEntity};

/// Vanilla `StartHuntingHoglin`: an adult piglin starts hunting a huntable hoglin
/// when nobody in its group has hunted recently.
pub struct StartHuntingHoglin;

impl StartHuntingHoglin {
    /// Vanilla `StartHuntingHoglin.create()`.
    #[must_use]
    pub fn create() -> OneShot<Self> {
        OneShot::new(Self).with_required_memories(vec![
            &mem::NEAREST_VISIBLE_HUNTABLE_HOGLIN,
            &mem::ANGRY_AT,
            &mem::HUNTED_RECENTLY,
            &mem::NEAREST_VISIBLE_ADULT_PIGLINS,
        ])
    }
}

impl Trigger for StartHuntingHoglin {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        let body = ctx.body;
        let level = ctx.level;

        if body.as_living_entity().is_some_and(LivingEntity::is_baby) {
            return false;
        }
        if ctx.has_memory_value(&mem::ANGRY_AT) || ctx.has_memory_value(&mem::HUNTED_RECENTLY) {
            return false;
        }

        let nearest_piglins = ctx
            .get_memory(&mem::NEAREST_VISIBLE_ADULT_PIGLINS)
            .unwrap_or_default();
        if nearest_piglins.iter().any(has_hunted_recently) {
            return false;
        }

        let Some(target) = ctx.get_memory(&mem::NEAREST_VISIBLE_HUNTABLE_HOGLIN) else {
            return false;
        };

        ai::set_anger_target(
            level,
            body.as_entity_event_source(),
            &target,
            ctx.memories_mut(),
        );
        ai::dont_kill_any_more_hoglins_for_a_while(ctx.memories_mut());
        ai::broadcast_anger_target(
            level,
            body.as_entity_event_source(),
            &target,
            ctx.memories(),
        );

        for piglin in nearest_piglins {
            if let Some(brain) = piglin.as_mob().and_then(Mob::brain) {
                ai::dont_kill_any_more_hoglins_for_a_while(brain.lock().memories_mut());
            }
        }

        true
    }
}

/// Vanilla `StartHuntingHoglin.hasHuntedRecently`.
fn has_hunted_recently(piglin: &SharedEntity) -> bool {
    piglin
        .as_mob()
        .and_then(Mob::brain)
        .is_some_and(|brain| brain.lock().has_memory_value(&mem::HUNTED_RECENTLY))
}
