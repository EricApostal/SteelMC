//! Vanilla `Mount`: walk to the remembered ride target, then ride it.

use std::sync::Arc;

use super::utils;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::tracker::{EntityTracker, WalkTarget};

/// Vanilla `Mount.CLOSE_ENOUGH_TO_START_RIDING_DIST`.
const CLOSE_ENOUGH_TO_START_RIDING_DIST: i32 = 1;

/// Vanilla `Mount`.
pub struct Mount {
    speed_modifier: f32,
}

impl Mount {
    /// Vanilla `Mount.create(speedModifier)`.
    #[must_use]
    pub fn create(speed_modifier: f32) -> OneShot<Self> {
        OneShot::new(Self { speed_modifier }).with_required_memories(vec![
            &mem::LOOK_TARGET,
            &mem::WALK_TARGET,
            &mem::RIDE_TARGET,
        ])
    }
}

impl Trigger for Mount {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if !ctx.check_memory(&mem::WALK_TARGET, MemoryStatus::ValueAbsent) {
            return false;
        }
        let Some(ridable) = ctx.get_memory(&mem::RIDE_TARGET) else {
            return false;
        };
        if ctx.body.is_passenger() {
            return false;
        }

        if utils::closer_than(
            ctx.body,
            &ridable,
            f64::from(CLOSE_ENOUGH_TO_START_RIDING_DIST),
        ) {
            ctx.body.start_riding(&ridable);
        } else {
            ctx.set_memory(
                &mem::LOOK_TARGET,
                Arc::new(EntityTracker::new(ridable.clone(), true)),
            );
            ctx.set_memory(
                &mem::WALK_TARGET,
                WalkTarget::new(
                    Arc::new(EntityTracker::new(ridable, false)),
                    self.speed_modifier,
                    CLOSE_ENOUGH_TO_START_RIDING_DIST,
                ),
            );
        }

        true
    }
}
