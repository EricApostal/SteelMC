//! Vanilla `TryToSniff`: start sniffing when something attackable is nearby.

use steel_registry::entity_data::EntityPose;

use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;

/// Vanilla `TryToSniff.SNIFF_COOLDOWN`, `UniformInt.of(100, 200)`.
const SNIFF_COOLDOWN_MIN: i32 = 100;
const SNIFF_COOLDOWN_MAX: i32 = 200;

/// Vanilla `TryToSniff.create`: remember `IS_SNIFFING`, roll the sniff cooldown,
/// drop the walk target and enter the `SNIFFING` pose.
pub struct TryToSniff;

impl TryToSniff {
    /// Vanilla `TryToSniff.create()`.
    #[must_use]
    pub fn create() -> OneShot<Self> {
        OneShot::new(Self).with_required_memories(vec![
            &mem::IS_SNIFFING,
            &mem::WALK_TARGET,
            &mem::SNIFF_COOLDOWN,
            &mem::NEAREST_ATTACKABLE,
            &mem::DISTURBANCE_LOCATION,
        ])
    }
}

impl Trigger for TryToSniff {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if ctx.has_memory_value(&mem::SNIFF_COOLDOWN)
            || !ctx.has_memory_value(&mem::NEAREST_ATTACKABLE)
            || ctx.has_memory_value(&mem::DISTURBANCE_LOCATION)
        {
            return false;
        }

        ctx.set_memory(&mem::IS_SNIFFING, ());
        ctx.set_memory_with_expiry(
            &mem::SNIFF_COOLDOWN,
            (),
            i64::from(rand::random_range(SNIFF_COOLDOWN_MIN..=SNIFF_COOLDOWN_MAX)),
        );
        ctx.erase_memory(&mem::WALK_TARGET);
        ctx.body.set_pose(EntityPose::Sniffing);
        true
    }
}
