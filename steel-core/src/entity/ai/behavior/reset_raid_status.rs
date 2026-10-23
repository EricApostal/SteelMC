//! Vanilla `ResetRaidStatus`.
//!
//! Vanilla rolls `1/20` per tick and returns the body to the schedule when no
//! raid is running (or it stopped/was lost). The default-activity reset and the
//! schedule refresh are applied centrally in `villager::tick` (see
//! `villager/brain.rs`); this behavior keeps the vanilla trigger cadence and
//! priority slot.

use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};

/// Vanilla `ResetRaidStatus`.
pub struct ResetRaidStatus;

impl Trigger for ResetRaidStatus {
    fn trigger(&mut self, _ctx: &mut BehaviorContext<'_>) -> bool {
        rand::random_range(0..20_i32) == 0
    }
}

/// Vanilla `ResetRaidStatus.create`.
#[must_use]
pub fn create() -> OneShot<ResetRaidStatus> {
    OneShot::new(ResetRaidStatus)
}
