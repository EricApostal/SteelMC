//! Vanilla `Swim`: keep a mob afloat in deep fluid by jumping at random.

use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};

/// Vanilla `Swim<T extends Mob>`.
pub struct Swim {
    state: BehaviorState,
    chance: f32,
}

impl Swim {
    /// Vanilla `new Swim(chance)`, using `FluidTags.ENTITY_FLOATABLE`.
    ///
    /// Steel's `is_in_water`/`water_height` cover exactly that tag's vanilla
    /// contents (`water`, `flowing_water`).
    #[must_use]
    pub fn new(chance: f32) -> Self {
        Self {
            state: BehaviorState::with_entry_condition(Vec::new()),
            chance,
        }
    }

    fn is_in_deep_fluid(ctx: &BehaviorContext<'_>) -> bool {
        let body = ctx.body;
        body.is_in_water() && body.fluid_contact().water_height() > body.get_fluid_jump_threshold()
            || body.is_in_lava()
    }
}

impl Behavior for Swim {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        Self::is_in_deep_fluid(ctx)
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        Self::is_in_deep_fluid(ctx)
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        if rand::random::<f32>() >= self.chance {
            return;
        }

        let Some(mob) = ctx.body.as_mob() else {
            return;
        };
        mob.mob_base().controls().lock().jump_control.jump();
    }
}
