//! Vanilla `Sniffing`: hold the `SNIFFING` pose and register the disturbance the
//! body smelled.

use steel_registry::entity_data::EntityPose;

use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStatus};

/// Vanilla `Sniffing`: hold the `SNIFFING` pose for the full duration while
/// `IS_SNIFFING` is remembered.
///
/// Vanilla's `stop` also raises anger at the remembered attackable and plays
/// `SoundEvents.WARDEN_SNIFF` on start; the anger management and the server
/// audio path have no Steel counterpart, so only the pose and the disturbance
/// registration survive.
pub struct Sniffing {
    state: BehaviorState,
}

impl Sniffing {
    /// Vanilla `new Sniffing(ticks)`, called with `WardenAi.SNIFFING_DURATION`.
    #[must_use]
    pub fn new(ticks: i32) -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (
                        &mem::IS_SNIFFING as MemoryModuleTypeRef,
                        MemoryStatus::ValuePresent,
                    ),
                    (
                        &mem::ATTACK_TARGET as MemoryModuleTypeRef,
                        MemoryStatus::ValueAbsent,
                    ),
                    (
                        &mem::WALK_TARGET as MemoryModuleTypeRef,
                        MemoryStatus::ValueAbsent,
                    ),
                    (
                        &mem::LOOK_TARGET as MemoryModuleTypeRef,
                        MemoryStatus::Registered,
                    ),
                    (
                        &mem::NEAREST_ATTACKABLE as MemoryModuleTypeRef,
                        MemoryStatus::Registered,
                    ),
                    (
                        &mem::DISTURBANCE_LOCATION as MemoryModuleTypeRef,
                        MemoryStatus::Registered,
                    ),
                    (
                        &mem::SNIFF_COOLDOWN as MemoryModuleTypeRef,
                        MemoryStatus::Registered,
                    ),
                ],
                ticks,
                ticks,
            ),
        }
    }
}

impl Behavior for Sniffing {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn can_still_use(&self, _ctx: &mut BehaviorContext<'_>) -> bool {
        true
    }

    fn start(&mut self, _ctx: &mut BehaviorContext<'_>) {
        // Vanilla plays `SoundEvents.WARDEN_SNIFF` here; the AI layer has no
        // server audio path, so sniffing only changes the pose via TryToSniff.
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        if ctx.body.pose() == EntityPose::Sniffing {
            ctx.body.set_pose(EntityPose::Standing);
        }

        ctx.erase_memory(&mem::IS_SNIFFING);

        let attackable = ctx.get_memory(&mem::NEAREST_ATTACKABLE);

        if let Some(entity) = attackable.filter(|entity| super::can_target_entity(ctx, entity)) {
            // Vanilla increases anger at the entity when it is within 6.0
            // horizontal / 20.0 vertical (`ANGER_FROM_SNIFFING_MAX_DISTANCE_XZ`/
            // `_Y`); the warden anger management has no Steel counterpart, so
            // only the disturbance registration below survives.
            if !ctx.has_memory_value(&mem::DISTURBANCE_LOCATION) {
                super::set_disturbance_location(ctx, entity.block_position());
            }
        }
    }
}
