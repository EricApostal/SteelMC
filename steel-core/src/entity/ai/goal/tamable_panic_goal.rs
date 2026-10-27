//! Vanilla `TamableAnimal.TamableAnimalPanicGoal`.
//!
//! The goal is an inner class of `TamableAnimal`, so every tameable mob that registers it
//! (`Cat`, `Wolf`) shares this implementation.

use steel_utils::Identifier;

use super::panic_goal::PanicGoal;
use super::selector::{Goal, GoalControls};
use crate::entity::{PathfinderMob, SharedEntity};

/// Vanilla `TamableAnimal.TamableAnimalPanicGoal`: panics, and teleports home when far.
pub(crate) struct TamableAnimalPanicGoal {
    inner: PanicGoal,
}

impl TamableAnimalPanicGoal {
    /// Vanilla `TamableAnimalPanicGoal(TamableAnimal, double)`: the default `PANIC_CAUSES`.
    #[must_use]
    pub(crate) const fn new(speed_modifier: f64) -> Self {
        Self {
            inner: PanicGoal::new(speed_modifier),
        }
    }

    /// Vanilla `TamableAnimalPanicGoal(TamableAnimal, double, TagKey<DamageType>)`: used by
    /// `Wolf` with `PANIC_ENVIRONMENTAL_CAUSES` so attacker damage does not make it flee.
    #[must_use]
    pub(crate) const fn for_causes(speed_modifier: f64, panic_causes: Identifier) -> Self {
        Self {
            inner: PanicGoal::for_causes(speed_modifier, panic_causes),
        }
    }
}

impl Goal for TamableAnimalPanicGoal {
    fn controls(&self) -> GoalControls {
        self.inner.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.inner.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.inner.can_continue_to_use(mob)
    }

    fn is_interruptable(&self) -> bool {
        self.inner.is_interruptable()
    }

    fn is_panic_goal(&self) -> bool {
        self.inner.is_panic_goal()
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.inner.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.inner.stop(mob);
    }

    fn requires_update_every_tick(&self) -> bool {
        self.inner.requires_update_every_tick()
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        if let Some(tameable) = mob.as_tameable()
            && !tameable.unable_to_move_to_owner()
            && tameable.should_try_teleport_to_owner()
        {
            tameable.try_to_teleport_to_owner();
        }

        self.inner.tick(mob, entity);
    }
}
