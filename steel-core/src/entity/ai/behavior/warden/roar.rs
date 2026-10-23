//! Vanilla `Roar`: hold the `ROARING` pose and convert the roar target into an
//! attack target.

use steel_registry::entity_data::EntityPose;

use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStatus};

use super::super::utils;
use super::ROAR_DURATION;

/// Vanilla `Roar.TICKS_BEFORE_PLAYING_ROAR_SOUND`.
const TICKS_BEFORE_PLAYING_ROAR_SOUND: i64 = 25;

/// Vanilla `Roar`: hold the `ROARING` pose for `WardenAi.ROAR_DURATION`, then
/// promote `ROAR_TARGET` to `ATTACK_TARGET`.
pub struct Roar {
    state: BehaviorState,
}

impl Roar {
    /// Vanilla `new Roar()`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (
                        &mem::ROAR_TARGET as MemoryModuleTypeRef,
                        MemoryStatus::ValuePresent,
                    ),
                    (
                        &mem::ATTACK_TARGET as MemoryModuleTypeRef,
                        MemoryStatus::ValueAbsent,
                    ),
                    (
                        &mem::ROAR_SOUND_COOLDOWN as MemoryModuleTypeRef,
                        MemoryStatus::Registered,
                    ),
                    (
                        &mem::ROAR_SOUND_DELAY as MemoryModuleTypeRef,
                        MemoryStatus::Registered,
                    ),
                ],
                ROAR_DURATION,
                ROAR_DURATION,
            ),
        }
    }
}

impl Default for Roar {
    fn default() -> Self {
        Self::new()
    }
}

impl Behavior for Roar {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        ctx.set_memory_with_expiry(&mem::ROAR_SOUND_DELAY, (), TICKS_BEFORE_PLAYING_ROAR_SOUND);
        ctx.erase_memory(&mem::WALK_TARGET);

        let Some(target) = ctx.get_memory(&mem::ROAR_TARGET) else {
            return;
        };

        utils::look_at_entity(ctx, &target);
        ctx.body.set_pose(EntityPose::Roaring);
        // Vanilla `body.increaseAngerAt(target, 20, false)`; the warden anger
        // management has no Steel counterpart.
    }

    fn can_still_use(&self, _ctx: &mut BehaviorContext<'_>) -> bool {
        true
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        if ctx.has_memory_value(&mem::ROAR_SOUND_DELAY)
            || ctx.has_memory_value(&mem::ROAR_SOUND_COOLDOWN)
        {
            return;
        }

        // Vanilla plays `SoundEvents.WARDEN_ROAR` here; the AI layer has no
        // server audio path.
        ctx.set_memory_with_expiry(&mem::ROAR_SOUND_COOLDOWN, (), i64::from(ROAR_DURATION - 25));
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        if ctx.body.pose() == EntityPose::Roaring {
            ctx.body.set_pose(EntityPose::Standing);
        }

        if let Some(target) = ctx.get_memory(&mem::ROAR_TARGET) {
            super::set_attack_target(ctx, target);
        }

        ctx.erase_memory(&mem::ROAR_TARGET);
    }
}
