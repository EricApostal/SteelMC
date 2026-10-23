//! Vanilla `SetHiddenState`: stay hidden until the hide timer runs out.

use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;

/// Vanilla `SetHiddenState.HIDE_TIMEOUT`.
const HIDE_TIMEOUT: i64 = 300;

/// Vanilla `SetHiddenState`.
pub struct SetHiddenState {
    stay_hidden_ticks: i32,
    close_enough_dist: i32,
    ticks_hidden: i32,
}

impl SetHiddenState {
    /// Vanilla `SetHiddenState.create(seconds, closeEnoughDist)`.
    #[must_use]
    pub fn create(seconds: i32, close_enough_dist: i32) -> OneShot<Self> {
        OneShot::new(Self {
            stay_hidden_ticks: seconds * 20,
            close_enough_dist,
            ticks_hidden: 0,
        })
        .with_required_memories(vec![&mem::HIDING_PLACE, &mem::HEARD_BELL_TIME])
    }
}

impl Trigger for SetHiddenState {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        let (Some(hiding_place), Some(heard_bell_time)) = (
            ctx.get_memory(&mem::HIDING_PLACE),
            ctx.get_memory(&mem::HEARD_BELL_TIME),
        ) else {
            return false;
        };

        if self.ticks_hidden <= self.stay_hidden_ticks
            && heard_bell_time + HIDE_TIMEOUT > ctx.timestamp
        {
            if hiding_place
                .pos
                .closer_to_center_than(ctx.body.position(), f64::from(self.close_enough_dist))
            {
                self.ticks_hidden += 1;
            }
            return true;
        }

        ctx.erase_memory(&mem::HEARD_BELL_TIME);
        ctx.erase_memory(&mem::HIDING_PLACE);
        // Vanilla also refreshes the activity schedule here; that refresh runs
        // centrally in `villager::tick` on the same memories.
        self.ticks_hidden = 0;
        true
    }
}
