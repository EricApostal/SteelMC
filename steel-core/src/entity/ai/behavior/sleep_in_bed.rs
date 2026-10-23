//! Vanilla `SleepInBed`: sleep in the remembered HOME bed.

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::vanilla_block_tags::BlockTag;

use super::interact_with_door::close_doors_that_i_have_opened_or_passed_through;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;

/// Vanilla `SleepInBed.COOLDOWN_AFTER_BEING_WOKEN`.
pub const COOLDOWN_AFTER_BEING_WOKEN: i64 = 100;
/// Vanilla `SleepInBed.stop`'s `nextOkStartTime = timestamp + 40`.
const WAKE_COOLDOWN: i64 = 40;
/// Vanilla `SleepInBed.timedOut` returns `false` unconditionally. `BehaviorState`
/// derives its timeout from a duration, so the largest duration stands in for "never";
/// see the report for this deviation.
const NEVER_TIMES_OUT: i32 = i32::MAX;

/// Vanilla `SleepInBed`.
pub struct SleepInBed {
    state: BehaviorState,
    next_ok_start_time: i64,
}

impl SleepInBed {
    /// Vanilla `new SleepInBed()`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (&mem::HOME, MemoryStatus::ValuePresent),
                    (&mem::LAST_WOKEN, MemoryStatus::Registered),
                    (&mem::LAST_SLEPT, MemoryStatus::Registered),
                    (&mem::WALK_TARGET, MemoryStatus::Registered),
                    (&mem::CANT_REACH_WALK_TARGET_SINCE, MemoryStatus::Registered),
                ],
                NEVER_TIMES_OUT,
                NEVER_TIMES_OUT,
            ),
            next_ok_start_time: 0,
        }
    }
}

impl Default for SleepInBed {
    fn default() -> Self {
        Self::new()
    }
}

impl Behavior for SleepInBed {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        if ctx.body.is_passenger() {
            return false;
        }

        let Some(home) = ctx.get_memory(&mem::HOME) else {
            return false;
        };
        if ctx.level.key != home.dimension {
            return false;
        }

        if let Some(last_woken) = ctx.get_memory(&mem::LAST_WOKEN) {
            let time_since_last_woken = ctx.timestamp - last_woken;
            if time_since_last_woken > 0 && time_since_last_woken < COOLDOWN_AFTER_BEING_WOKEN {
                return false;
            }
        }

        let bed_state = ctx.level.get_block_state(home.pos);
        home.pos.closer_to_center_than(ctx.body.position(), 2.0)
            && bed_state
                .get_block()
                .has_tag(&BlockTag::VILLAGERS_CAN_SLEEP_ON_BED)
            && !bed_state.get_value(&BlockStateProperties::OCCUPIED)
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        // Vanilla additionally requires the brain's active activity to be `REST`;
        // the activity arbiter applies that gate centrally.
        let Some(home) = ctx.get_memory(&mem::HOME) else {
            return false;
        };

        ctx.body.position().y > f64::from(home.pos.y()) + 0.4
            && home.pos.closer_to_center_than(ctx.body.position(), 1.14)
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        if ctx.timestamp <= self.next_ok_start_time {
            return;
        }

        let Some(home) = ctx.get_memory(&mem::HOME) else {
            return;
        };

        // Vanilla closes the doors the body opened or passed through before sleeping.
        if let Some(mut doors) = ctx.get_memory(&mem::DOORS_TO_CLOSE)
            && let Some(world) = ctx.body.level()
        {
            let nearest = ctx.get_memory(&mem::NEAREST_LIVING_ENTITIES);
            close_doors_that_i_have_opened_or_passed_through(
                &world,
                ctx.body,
                None,
                None,
                &mut doors,
                nearest.as_deref(),
            );
            ctx.set_memory(&mem::DOORS_TO_CLOSE, doors);
        }

        let slept = ctx
            .body
            .as_living_entity()
            .is_some_and(|living| living.start_sleeping(home.pos).is_ok());
        if slept {
            ctx.set_memory(&mem::LAST_SLEPT, ctx.timestamp);
        }

        ctx.erase_memory(&mem::WALK_TARGET);
        ctx.erase_memory(&mem::CANT_REACH_WALK_TARGET_SINCE);
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        let Some(living) = ctx.body.as_living_entity() else {
            return;
        };
        if living.is_sleeping() {
            living.stop_sleeping();
            self.next_ok_start_time = ctx.timestamp + WAKE_COOLDOWN;
        }
    }
}
