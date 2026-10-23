//! Vanilla `ReactToBell`.
//!
//! Vanilla hides the body when it hears a bell outside a raid. The `HIDE`
//! activity switch is applied centrally in `villager::tick` (see
//! `villager/brain.rs`); the entry condition stays here so the behavior only
//! starts while `HEARD_BELL_TIME` is remembered.

use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, module_type as mem};

/// Vanilla `ReactToBell`.
pub struct ReactToBell;

impl Trigger for ReactToBell {
    fn trigger(&mut self, _ctx: &mut BehaviorContext<'_>) -> bool {
        true
    }
}

/// Vanilla `ReactToBell.create`.
#[must_use]
pub fn create() -> OneShot<ReactToBell> {
    OneShot::new(ReactToBell)
        .with_required_memories(vec![&mem::HEARD_BELL_TIME as MemoryModuleTypeRef])
}
