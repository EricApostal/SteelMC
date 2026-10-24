//! Vanilla `StopHoldingItemIfNoLongerAdmiring`.

use steel_registry::data_components::vanilla_components::BLOCKS_ATTACKS;
use steel_utils::types::InteractionHand;

use super::ai;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;

/// Vanilla `StopHoldingItemIfNoLongerAdmiring`: puts the off-hand item away (or
/// barters with it) once the admire activity ended.
pub struct StopHoldingItemIfNoLongerAdmiring;

impl StopHoldingItemIfNoLongerAdmiring {
    /// Vanilla `StopHoldingItemIfNoLongerAdmiring.create()`.
    #[must_use]
    pub fn create() -> OneShot<Self> {
        OneShot::new(Self).with_required_memories(vec![&mem::ADMIRING_ITEM])
    }
}

impl Trigger for StopHoldingItemIfNoLongerAdmiring {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        // Vanilla entry condition: `absent(ADMIRING_ITEM)`.
        if ctx.check_memory(&mem::ADMIRING_ITEM, MemoryStatus::ValuePresent) {
            return false;
        }

        let Some(living) = ctx.body.as_living_entity() else {
            return false;
        };
        let off_hand = living.get_item_in_hand(InteractionHand::OffHand);
        if off_hand.is_empty() || off_hand.has_component(BLOCKS_ATTACKS.key()) {
            return false;
        }

        let throw_target = ctx
            .get_memory(&mem::NEAREST_VISIBLE_PLAYER)
            .map(|player| player.position());
        ai::stop_holding_off_hand_item(
            ctx.level,
            ctx.body.as_entity_event_source(),
            true,
            throw_target,
        );
        true
    }
}
