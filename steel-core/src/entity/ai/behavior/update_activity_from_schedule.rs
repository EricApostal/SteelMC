//! Vanilla `UpdateActivityFromSchedule`.
//!
//! Vanilla refreshes the scheduled activity every tick through
//! `Brain.updateActivityFromSchedule`, which throttles itself with
//! `SCHEDULE_UPDATE_DELAY`. Steel behaviors cannot reach the [`Brain`], so the
//! refresh runs centrally in `villager::tick` (see `villager/brain.rs`) with the
//! same throttle; this behavior keeps the vanilla priority slot (99).
//!
//! [`Brain`]: crate::entity::ai::brain::Brain

use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};

/// Vanilla `UpdateActivityFromSchedule`.
pub struct UpdateActivityFromSchedule;

impl Trigger for UpdateActivityFromSchedule {
    fn trigger(&mut self, _ctx: &mut BehaviorContext<'_>) -> bool {
        true
    }
}

/// Vanilla `UpdateActivityFromSchedule.create`.
#[must_use]
pub fn create() -> OneShot<UpdateActivityFromSchedule> {
    OneShot::new(UpdateActivityFromSchedule)
}
