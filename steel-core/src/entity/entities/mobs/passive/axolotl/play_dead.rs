//! Vanilla axolotl play-dead behaviors: `PlayDead` and `ValidatePlayDead`.
//!
//! `PlayDead` is a 200-tick behavior that freezes the axolotl in place and
//! regenerates it; `ValidatePlayDead` counts the `PLAY_DEAD_TICKS` memory down
//! and clears the play-dead state once it reaches zero. Both are registered by
//! the axolotl activity provider (`AxolotlAi`, wired in `axolotl/brain.rs`).

use steel_registry::vanilla_mob_effects;

use crate::entity::MobEffectInstance;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;

/// Vanilla `Axolotl.TOTAL_PLAYDEAD_TIME`.
const TOTAL_PLAYDEAD_TIME: i32 = 200;
/// Vanilla `PlayDead`'s regeneration duration (`MobEffects.REGENERATION, 200, 0`).
const PLAY_DEAD_REGEN_DURATION: i32 = 200;
/// Behaviors that never time out while their `can_still_use` holds.
const NEVER_TIMES_OUT: i32 = i32::MAX;

/// Vanilla `PlayDead`: freeze the axolotl and regenerate it for 200 ticks.
pub(crate) struct PlayDead {
    state: BehaviorState,
}

impl PlayDead {
    /// Vanilla `new PlayDead()`: requires `PLAY_DEAD_TICKS` and `HURT_BY_ENTITY`,
    /// with a fixed 200-tick duration.
    pub(crate) fn new() -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (&mem::PLAY_DEAD_TICKS, MemoryStatus::ValuePresent),
                    (&mem::HURT_BY_ENTITY, MemoryStatus::ValuePresent),
                ],
                TOTAL_PLAYDEAD_TIME,
                TOTAL_PLAYDEAD_TIME,
            ),
        }
    }
}

impl Behavior for PlayDead {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    /// Vanilla `checkExtraStartConditions`: only while in water.
    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.body.is_in_water()
    }

    /// Vanilla `canStillUse`: still in water with the play-dead timer running.
    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.body.is_in_water() && ctx.has_memory_value(&mem::PLAY_DEAD_TICKS)
    }

    /// Vanilla `start`: drop movement/look targets and apply regeneration.
    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        ctx.erase_memory(&mem::WALK_TARGET);
        ctx.erase_memory(&mem::LOOK_TARGET);
        if let Some(living) = ctx.body.as_living_entity() {
            living.add_mob_effect(MobEffectInstance::with_duration(
                vanilla_mob_effects::REGENERATION,
                PLAY_DEAD_REGEN_DURATION,
                0,
            ));
        }
    }
}

/// Vanilla `ValidatePlayDead`: count `PLAY_DEAD_TICKS` down and clear it when spent.
pub(crate) struct ValidatePlayDead {
    state: BehaviorState,
}

impl ValidatePlayDead {
    /// Vanilla `ValidatePlayDead.create()`: requires `PLAY_DEAD_TICKS` and a
    /// registered `HURT_BY_ENTITY`, running every tick.
    pub(crate) fn create() -> Self {
        Self::new()
    }

    fn new() -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (&mem::PLAY_DEAD_TICKS, MemoryStatus::ValuePresent),
                    (&mem::HURT_BY_ENTITY, MemoryStatus::Registered),
                ],
                NEVER_TIMES_OUT,
                NEVER_TIMES_OUT,
            ),
        }
    }
}

impl Behavior for ValidatePlayDead {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    /// Vanilla `BehaviorBuilder` one-shots keep running while they return `true`.
    fn can_still_use(&self, _ctx: &mut BehaviorContext<'_>) -> bool {
        true
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        let ticks = ctx.get_memory(&mem::PLAY_DEAD_TICKS).unwrap_or(0);
        if ticks <= 0 {
            ctx.erase_memory(&mem::PLAY_DEAD_TICKS);
            ctx.erase_memory(&mem::HURT_BY_ENTITY);
            // GAP(axolotl-brain-provider): vanilla also calls
            // `body.getBrain().useDefaultActivity()` here. Steel passes the
            // memory store into behaviors instead of letting them re-enter the
            // (locked) brain, so the activity reset is done by the axolotl
            // activity provider in `axolotl/brain.rs` once the timer clears.
        } else {
            ctx.set_memory(&mem::PLAY_DEAD_TICKS, ticks - 1);
        }
    }
}
