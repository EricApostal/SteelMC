//! Vanilla `SculkShriekerBlock`.
//!
//! Vanilla reference: `net.minecraft.world.level.block.SculkShriekerBlock` (`shrieking` +
//! `can_summon` + `waterlogged`). The 90-tick shriek is started by
//! [`SculkShriekerBlockEntity`](crate::block_entity::entities::SculkShriekerBlockEntity) through
//! its vibration listener or `stepOn`; this behavior owns placement, the shriek-expiry tick that
//! clears `SHRIEKING` and delegates to the entity's `tryRespond`, and the 5-XP break drop. The
//! `PARTICLES_SCULK_SHRIEK` level event (3007) is emitted by the entity when the shriek starts.

use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, BoolProperty};
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_block_entity_types;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, value_providers::IntProvider};

use crate::behavior::{BlockBehavior, BlockEntityCreation, BlockPlaceContext, try_drop_experience};
use crate::block_entity::BlockEntityTicker;
use crate::world::{LevelReader, World};

/// Vanilla `SculkShriekerBlockEntity.SHRIEKING_TICKS`.
pub const SCULK_SHRIEKING_TICKS: i32 = 90;

/// Vanilla `SculkShriekerBlock`.
#[block_behavior]
pub struct SculkShriekerBlock {
    block: BlockRef,
}

const SHRIEKING: &BoolProperty = &BlockStateProperties::SHRIEKING;
const CAN_SUMMON: &BoolProperty = &BlockStateProperties::CAN_SUMMON;
const WATERLOGGED: &BoolProperty = &BlockStateProperties::WATERLOGGED;

impl SculkShriekerBlock {
    /// Creates shrieker behavior for `block`.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Current shrieking state of `state`.
    #[must_use]
    pub fn is_shrieking(state: BlockStateId) -> bool {
        state.get_value(SHRIEKING)
    }

    /// Vanilla `SculkShriekerBlockEntity.shriek` block half: set `SHRIEKING` and schedule the
    /// 90-tick expiry.
    ///
    /// Vanilla uses `Level.setBlock(pos, state, 2)` — clients only, no neighbour updates — and
    /// emits both `LevelEvent 3007`/`PARTICLES_SCULK_SHRIEK` and the `shriek` game event
    /// separately in [`SculkShriekerBlockEntity`](crate::block_entity::entities::SculkShriekerBlockEntity).
    pub fn start_shriek(block: BlockRef, world: &Arc<World>, pos: BlockPos, state: BlockStateId) {
        if state.get_value(SHRIEKING) {
            return;
        }
        world.set_block(
            pos,
            state.set_value(SHRIEKING, true),
            UpdateFlags::UPDATE_CLIENTS,
        );
        world.schedule_block_tick_default(pos, block, SCULK_SHRIEKING_TICKS);
    }

    /// Reads `can_summon` without panicking on states that lack the property.
    #[must_use]
    pub fn can_summon(state: BlockStateId) -> bool {
        state.try_get_value(CAN_SUMMON).unwrap_or(false)
    }
}

impl BlockBehavior for SculkShriekerBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(
            self.block
                .default_state()
                .set_value(WATERLOGGED, context.is_water_source()),
        )
    }

    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        if !state.get_value(SHRIEKING) {
            return;
        }
        world.set_block(
            pos,
            state.set_value(SHRIEKING, false),
            UpdateFlags::UPDATE_ALL,
        );
        // Vanilla `tick`: clear `SHRIEKING`, then delegate to the BE's `tryRespond`. The
        // `PARTICLES_SCULK_SHRIEK` level event is emitted by the BE when the shriek *starts*.
        use steel_utils::Downcast as _;
        if let Some(block_entity) = world.get_block_entity(pos)
            && let Some(shrieker) = block_entity
                .downcast_ref::<crate::block_entity::entities::SculkShriekerBlockEntity>(
            )
        {
            shrieker.try_respond(world);
        }
    }

    // Vanilla `SculkShriekerBlock` has no `canSurvive` override; `updateShape` only maintains the
    // waterlogged tick, handled by the generic waterlogged path.

    fn step_on(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        entity: &dyn crate::entity::Entity,
    ) {
        // Vanilla `stepOn` resolves the stepping entity through
        // `SculkShriekerBlockEntity.tryGetPlayer` (direct player, controlling passenger,
        // projectile owner, or item owner); the BE resolves that rule from the source UUID.
        if let Some(block_entity) = world.get_block_entity(pos) {
            use steel_utils::Downcast as _;
            if let Some(shrieker) = block_entity
                .downcast_ref::<crate::block_entity::entities::SculkShriekerBlockEntity>(
            ) {
                shrieker.try_shriek(world, Some(entity.uuid()));
            }
        }
        self.default_step_on(state, world, pos, entity);
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::Created(Arc::new(
            crate::block_entity::entities::SculkShriekerBlockEntity::new(level, pos, state),
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
            &vanilla_block_entity_types::SCULK_SHRIEKER,
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
            // Vanilla `ConstantInt.of(5)`.
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
    fn shriek_sets_the_state_and_schedules_the_ninety_tick_expiry() {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world("sculk_shrieker_shriek");
        let world = &world_fixture.world;
        let pos = BlockPos::new(8, 64, 8);
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));
        let state = vanilla_blocks::SCULK_SHRIEKER.default_state();
        world.set_block(pos, state, UpdateFlags::UPDATE_ALL);

        SculkShriekerBlock::start_shriek(&vanilla_blocks::SCULK_SHRIEKER, world, pos, state);
        assert!(SculkShriekerBlock::is_shrieking(world.get_block_state(pos)));
        assert!(world.has_scheduled_block_tick(pos, &vanilla_blocks::SCULK_SHRIEKER));
        assert_eq!(SCULK_SHRIEKING_TICKS, 90);
    }

    #[test]
    fn expiry_tick_clears_shrieking_and_responds() {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world("sculk_shrieker_expiry");
        let world = &world_fixture.world;
        let pos = BlockPos::new(8, 64, 8);
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));
        let behavior = SculkShriekerBlock::new(&vanilla_blocks::SCULK_SHRIEKER);
        // `can_summon` gates `tryRespond`; without it the expiry only clears the state.
        let shrieking = vanilla_blocks::SCULK_SHRIEKER
            .default_state()
            .set_value(SHRIEKING, true)
            .set_value(CAN_SUMMON, true);
        world.set_block(pos, shrieking, UpdateFlags::UPDATE_ALL);

        behavior.tick(shrieking, world, pos);
        assert!(!SculkShriekerBlock::is_shrieking(
            world.get_block_state(pos)
        ));
    }

    #[test]
    fn can_summon_reads_the_block_property() {
        init_vanilla_registry();
        assert!(!SculkShriekerBlock::can_summon(
            vanilla_blocks::SCULK_SHRIEKER.default_state()
        ));
        assert!(SculkShriekerBlock::can_summon(
            vanilla_blocks::SCULK_SHRIEKER
                .default_state()
                .set_value(CAN_SUMMON, true)
        ));
    }
}
