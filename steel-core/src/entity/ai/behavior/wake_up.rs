//! Vanilla `WakeUp`.
//!
//! Vanilla wakes a sleeping body as soon as its brain stops running
//! `Activity.REST`. Steel behaviors only receive the memory store, so they cannot
//! read the active activity; the wake-up is applied centrally in
//! `villager::tick` (see `villager/brain.rs`) where the [`Brain`] is in scope.
//! This type keeps the vanilla core-package priority slot.
//!
//! [`Brain`]: crate::entity::ai::brain::Brain

use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};

/// Vanilla `WakeUp`.
pub struct WakeUp;

impl Trigger for WakeUp {
    fn trigger(&mut self, _ctx: &mut BehaviorContext<'_>) -> bool {
        false
    }
}

/// Vanilla `WakeUp.create`.
#[must_use]
pub fn create() -> OneShot<WakeUp> {
    OneShot::new(WakeUp)
}
