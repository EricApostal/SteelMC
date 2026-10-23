//! Vanilla `StartCelebratingIfTargetDead`: celebrate over a defeated target.

use steel_registry::vanilla_entities;
use steel_registry::vanilla_game_rules::FORGIVE_DEAD_PLAYERS;

use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::{MemoryStatus, module_type as mem};

/// Vanilla `BiPredicate<LivingEntity, LivingEntity>` dance gate.
pub type DancePredicate = fn(&SharedEntity, &SharedEntity) -> bool;

/// Vanilla `StartCelebratingIfTargetDead.create(int celebrateDuration, BiPredicate
/// dancePredicate)`: once `ATTACK_TARGET` is dead, remembers where it fell and
/// starts dancing for `celebrateDuration` ticks when the predicate agrees, then
/// clears the target (and `ANGRY_AT`) unless the target is an unforgiven player.
pub struct StartCelebratingIfTargetDead {
    celebrate_duration: i64,
    dance_predicate: DancePredicate,
}

impl StartCelebratingIfTargetDead {
    /// Vanilla `StartCelebratingIfTargetDead.create(int celebrateDuration,
    /// BiPredicate dancePredicate)`.
    #[must_use]
    pub fn create(celebrate_duration: i32, dance_predicate: DancePredicate) -> OneShot<Self> {
        OneShot::new(Self {
            celebrate_duration: i64::from(celebrate_duration),
            dance_predicate,
        })
        .with_required_memories(vec![
            &mem::ATTACK_TARGET,
            &mem::ANGRY_AT,
            &mem::CELEBRATE_LOCATION,
            &mem::DANCING,
        ])
    }
}

impl Trigger for StartCelebratingIfTargetDead {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        let Some(target) = ctx.get_memory(&mem::ATTACK_TARGET) else {
            return false;
        };

        // Vanilla entry condition: `absent(CELEBRATE_LOCATION)`.
        if ctx.check_memory(&mem::CELEBRATE_LOCATION, MemoryStatus::ValuePresent) {
            return false;
        }

        if !target
            .as_living_entity()
            .is_some_and(crate::entity::LivingEntity::is_dead_or_dying)
        {
            return false;
        }

        if (self.dance_predicate)(ctx.body, &target) {
            ctx.set_memory_with_expiry(&mem::DANCING, true, self.celebrate_duration);
        }

        ctx.set_memory_with_expiry(
            &mem::CELEBRATE_LOCATION,
            target.block_position(),
            self.celebrate_duration,
        );

        if target.entity_type() != &vanilla_entities::PLAYER
            || ctx.level.get_game_rule(&FORGIVE_DEAD_PLAYERS)
        {
            ctx.erase_memory(&mem::ATTACK_TARGET);
            ctx.erase_memory(&mem::ANGRY_AT);
        }

        true
    }
}
