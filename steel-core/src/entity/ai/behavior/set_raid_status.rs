//! Vanilla `SetRaidStatus`.
//!
//! Vanilla rolls `1/20` per tick and switches the default/active activity between
//! `RAID` and `PRE_RAID` from the nearby raid's state. The switch is applied
//! centrally in `villager::tick` (see `villager/brain.rs`); this behavior keeps
//! the vanilla trigger cadence and priority slot.

use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};

/// Vanilla `SetRaidStatus`.
pub struct SetRaidStatus;

impl Trigger for SetRaidStatus {
    fn trigger(&mut self, _ctx: &mut BehaviorContext<'_>) -> bool {
        rand::random_range(0..20_i32) == 0
    }
}

/// Vanilla `SetRaidStatus.create`.
#[must_use]
pub fn create() -> OneShot<SetRaidStatus> {
    OneShot::new(SetRaidStatus)
}
