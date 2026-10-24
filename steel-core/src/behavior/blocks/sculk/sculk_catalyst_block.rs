//! Vanilla `SculkCatalystBlock`.
//!
//! Vanilla reference: `net.minecraft.world.level.block.SculkCatalystBlock` (`bloom` pulse with an
//! 8-tick reset, `ConstantInt.of(5)` experience drop). Spreading itself is owned by
//! [`SculkCatalystBlockEntity`](crate::block_entity::entities::SculkCatalystBlockEntity); this
//! behavior only owns placement state, the bloom-pulse tick and the break drop.

use std::sync::{Arc, Weak};

use glam::DVec3;
use steel_macros::block_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, BoolProperty};
use steel_registry::item_stack::ItemStack;
use steel_registry::particle_type::ParticleData;
use steel_registry::{sound_events, vanilla_block_entity_types, vanilla_particle_types};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, value_providers::IntProvider};

use crate::behavior::{BlockBehavior, BlockEntityCreation, BlockPlaceContext, try_drop_experience};
use crate::block_entity::BlockEntityTicker;
use crate::world::World;

/// Vanilla `SculkCatalystBlockEntity.CatalystListener.PULSE_TICKS`.
pub const SCULK_CATALYST_PULSE_TICKS: i32 = 8;

/// Vanilla `SculkCatalystBlock`.
#[block_behavior]
pub struct SculkCatalystBlock {
    block: BlockRef,
}

const BLOOM: &BoolProperty = &BlockStateProperties::BLOOM;

impl SculkCatalystBlock {
    /// Creates catalyst behavior for `block`.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Current bloom-pulsing state of `state`.
    #[must_use]
    pub fn is_blooming(state: BlockStateId) -> bool {
        state.get_value(BLOOM)
    }

    /// Vanilla `CatalystListener.bloom`: pulse the catalyst for [`SCULK_CATALYST_PULSE_TICKS`],
    /// then play `SCULK_CATALYST_BLOOM` at `2.0` volume with the vanilla pitch jitter.
    ///
    /// Vanilla has no `PULSE` guard: every qualifying death re-runs the whole pulse (state write,
    /// re-scheduled tick, particles, sound).
    pub fn bloom(block: BlockRef, world: &Arc<World>, pos: BlockPos, state: BlockStateId) {
        world.set_block(pos, state.set_value(BLOOM, true), UpdateFlags::UPDATE_ALL);
        world.schedule_block_tick_default(pos, block, SCULK_CATALYST_PULSE_TICKS);
        // Vanilla `SculkCatalystBlockEntity.CatalystListener.bloom`.
        world.send_particles(
            ParticleData::simple(&vanilla_particle_types::SCULK_SOUL),
            DVec3::new(
                f64::from(pos.x()) + 0.5,
                f64::from(pos.y()) + 1.15,
                f64::from(pos.z()) + 0.5,
            ),
            2,
            DVec3::new(0.2, 0.0, 0.2),
            0.0,
        );
        world.play_sound(
            &sound_events::BLOCK_SCULK_CATALYST_BLOOM,
            SoundSource::Blocks,
            pos,
            2.0,
            0.6 + rand::random::<f32>() * 0.4,
            None,
        );
    }
}

impl BlockBehavior for SculkCatalystBlock {
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(self.block.default_state())
    }

    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        if state.get_value(BLOOM) {
            world.set_block(pos, state.set_value(BLOOM, false), UpdateFlags::UPDATE_ALL);
        }
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        // Constructed directly (like the sculk sensors) so the game-event listener exists even
        // before the registry factory line lands; disk loading still goes through `BLOCK_ENTITIES`.
        BlockEntityCreation::Created(Arc::new(
            crate::block_entity::entities::SculkCatalystBlockEntity::new(level, pos, state),
        ))
    }

    fn get_block_entity_ticker(
        &self,
        _world: &Arc<World>,
        _state: BlockStateId,
        block_entity_type: BlockEntityTypeRef,
    ) -> Option<BlockEntityTicker> {
        BlockEntityTicker::for_matching_entity_tick(
            block_entity_type,
            &vanilla_block_entity_types::SCULK_CATALYST,
        )
    }

    fn spawn_after_break(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        tool: &ItemStack,
        drop_experience: bool,
    ) {
        if drop_experience {
            // Vanilla `SculkCatalystBlock`: `ConstantInt.of(5)`.
            try_drop_experience(world, pos, tool, &IntProvider::Constant(5));
        }
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, vanilla_blocks};
    use steel_utils::ChunkPos;
    use steel_utils::types::UpdateFlags;

    use super::*;
    use crate::behavior::init_behaviors;
    use crate::test_support::{fresh_test_world, insert_ready_full_chunk};

    #[test]
    fn a_second_death_refreshes_the_bloom_pulse_like_vanilla() {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world("sculk_catalyst_bloom");
        let world = &world_fixture.world;
        let pos = BlockPos::new(8, 64, 8);
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));
        let state = vanilla_blocks::SCULK_CATALYST.default_state();
        world.set_block(pos, state, UpdateFlags::UPDATE_ALL);

        SculkCatalystBlock::bloom(&vanilla_blocks::SCULK_CATALYST, world, pos, state);
        assert!(SculkCatalystBlock::is_blooming(world.get_block_state(pos)));
        assert!(world.has_scheduled_block_tick(pos, &vanilla_blocks::SCULK_CATALYST));

        // Vanilla has no bloom guard, so a second death re-runs the pulse.
        SculkCatalystBlock::bloom(
            &vanilla_blocks::SCULK_CATALYST,
            world,
            pos,
            world.get_block_state(pos),
        );
        assert!(SculkCatalystBlock::is_blooming(world.get_block_state(pos)));
    }

    #[test]
    fn bloom_tick_clears_the_pulse() {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world("sculk_catalyst_bloom_tick");
        let world = &world_fixture.world;
        let pos = BlockPos::new(8, 64, 8);
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));
        let blooming = vanilla_blocks::SCULK_CATALYST
            .default_state()
            .set_value(BLOOM, true);
        world.set_block(pos, blooming, UpdateFlags::UPDATE_ALL);

        SculkCatalystBlock::new(&vanilla_blocks::SCULK_CATALYST).tick(blooming, world, pos);
        assert!(!SculkCatalystBlock::is_blooming(world.get_block_state(pos)));
    }
}
