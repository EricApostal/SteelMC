//! `SnifferEggBlock` behavior (`net.minecraft.world.level.block.SnifferEggBlock`).
//!
//! Vanilla schedules a hatch progression tick on placement (24 000 ticks, halved to 12 000 on
//! moss, plus a random 0-299 offset) and increments `hatch` on each tick until the third stage
//! spawns a baby sniffer. Steel has no sniffer entity yet, so the hatch stages, sounds and block
//! removal are complete while the spawn is a reported dependency.

use std::sync::Arc;

use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, IntProperty};
use steel_registry::sound_events;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId};

use crate::behavior::{BlockBehavior, BlockPlaceContext};
use crate::world::World;

/// Vanilla `SnifferEggBlock.MAX_HATCH_LEVEL`.
pub const MAX_HATCH_LEVEL: u8 = 2;

/// Vanilla `SnifferEggBlock.REGULAR_HATCH_TIME_TICKS`.
const REGULAR_HATCH_TIME_TICKS: i32 = 24_000;
/// Vanilla `SnifferEggBlock.BOOSTED_HATCH_TIME_TICKS` (on moss).
const BOOSTED_HATCH_TIME_TICKS: i32 = 12_000;
/// Vanilla `SnifferEggBlock.RANDOM_HATCH_OFFSET_TICKS`.
const RANDOM_HATCH_OFFSET_TICKS: i32 = 300;

const HATCH: &IntProperty = &BlockStateProperties::HATCH;

/// `SnifferEggBlock` behavior.
#[block_behavior]
pub struct SnifferEggBlock {
    block: BlockRef,
}

impl SnifferEggBlock {
    /// Creates a sniffer-egg block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Vanilla `SnifferEggBlock.getHatchLevel`.
    #[must_use]
    pub fn hatch_level(state: BlockStateId) -> u8 {
        state.get_value(HATCH)
    }

    /// Vanilla `SnifferEggBlock.isReadyToHatch`.
    #[must_use]
    pub fn is_ready_to_hatch(state: BlockStateId) -> bool {
        Self::hatch_level(state) == MAX_HATCH_LEVEL
    }
}

impl BlockBehavior for SnifferEggBlock {
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(self.block.default_state().set_value(HATCH, 0))
    }

    fn on_place(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        old_state: BlockStateId,
        _moved_by_piston: bool,
    ) {
        if old_state.get_block() == self.block {
            return;
        }

        // Vanilla `hatchBoost`: a block below tagged `#sniffer_egg_hatch_boost` (moss blocks)
        // halves the hatch time and plays level event 3009 (`PARTICLES_EGG_CRACK`).
        let boosted = world
            .get_block_state(pos.below())
            .get_block()
            .has_tag(&steel_registry::vanilla_block_tags::BlockTag::SNIFFER_EGG_HATCH_BOOST);
        if boosted {
            world.level_event(3009, pos, 0, None);
        }

        let hatch_time = if boosted {
            BOOSTED_HATCH_TIME_TICKS
        } else {
            REGULAR_HATCH_TIME_TICKS
        };
        let progression_delay = hatch_time / 3 + rand::random_range(0..RANDOM_HATCH_OFFSET_TICKS);
        world.schedule_block_tick_default(pos, self.block, progression_delay);
        // Vanilla re-emits `BLOCK_PLACE` here (the placement path already emitted it).
        world.game_event(
            &steel_registry::vanilla_game_events::BLOCK_PLACE,
            pos,
            &crate::world::game_event::GameEventContext::new(None, Some(state)),
        );
    }

    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        let pitch = rand::random::<f32>() * 0.2 + 0.9;
        if !Self::is_ready_to_hatch(state) {
            world.play_block_sound(
                &sound_events::BLOCK_SNIFFER_EGG_CRACK,
                pos,
                0.7,
                pitch,
                None,
            );
            world.set_block(
                pos,
                state.set_value(HATCH, Self::hatch_level(state) + 1),
                UpdateFlags::UPDATE_CLIENTS,
            );
            return;
        }

        world.play_block_sound(
            &sound_events::BLOCK_SNIFFER_EGG_HATCH,
            pos,
            0.7,
            pitch,
            None,
        );
        world.destroy_block(pos, false);
        // TODO(Entities): spawn a baby `SNIFFER` at the block center once the entity lane
        // registers a factory for `vanilla_entities::SNIFFER` (vanilla
        // `SnifferEggBlock.tick` → `EntityTypes.SNIFFER.create` + `setBaby` + `addFreshEntity`).
    }
}
