//! Nether portal block behavior.

use std::sync::Arc;

use crate::behavior::block::BlockBehavior;
use crate::behavior::context::BlockPlaceContext;
use crate::entity::{
    Entity, EntitySpawnPlacement, EntitySpawnReason, EntitySpawnRequest,
    InsideBlockEffectCollector, spawn_entity,
};
use crate::portal::PortalKind;
use crate::portal::portal_shape::{PortalShape, nether_portal_config};
use crate::world::ScheduledTickAccess;
use crate::world::World;
use crate::world::spawn::local_mob_cap::players_close_for_spawning;
use crate::world::spawn::placement::is_valid_spawn_on_block;
use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::blocks::properties::{Axis, BlockStateProperties, EnumProperty};
use steel_registry::vanilla_blocks::AIR;
use steel_registry::vanilla_entities;
use steel_utils::{BlockPos, BlockStateId, ChunkPos, Difficulty, Direction};

/// Behavior for the nether portal block.
#[block_behavior]
pub struct NetherPortalBlock {
    block: BlockRef,
}

const HORIZONTAL_AXIS: &EnumProperty<Axis> = &BlockStateProperties::HORIZONTAL_AXIS;

impl NetherPortalBlock {
    /// Create a new `NetherPortalBlock`
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    fn apply_entity_inside(pos: BlockPos, entity: &dyn Entity) {
        if entity.can_use_portal(false) {
            entity.set_as_inside_portal(PortalKind::Nether, pos);
        }
    }
}

impl BlockBehavior for NetherPortalBlock {
    fn update_shape(
        &self,
        state: BlockStateId,
        world: &dyn ScheduledTickAccess,
        pos: BlockPos,
        direction: Direction,
        _neighbor_pos: BlockPos,
        neighbor_state: BlockStateId,
    ) -> BlockStateId {
        let update_axis = direction.get_axis();
        let axis: Axis = state.get_value(HORIZONTAL_AXIS);
        let wrong_axis = axis != update_axis && update_axis != Axis::Y;

        if !wrong_axis
            && neighbor_state.get_block() != self.block
            && !PortalShape::find_any_shape(world, pos, axis, &nether_portal_config())
                .is_some_and(|s| s.is_complete())
        {
            return AIR.default_state();
        }
        state
    }

    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(self.block.default_state())
    }

    /// Vanilla `NetherPortalBlock.randomTick`: spawns a zombified piglin on the block
    /// below the portal column, gated by monsters spawning, difficulty, the
    /// `nether_portal_spawns_piglin` dimension attribute, and player proximity.
    fn random_tick(&self, _state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        if !world.is_spawning_monsters()
            || world.difficulty() == Difficulty::Peaceful
            || !world.dimension_type.nether_portal_spawns_piglin
        {
            return;
        }

        let difficulty_id = i32::from(u8::from(world.difficulty()));
        if rand::random_range(0..2000) >= difficulty_id {
            return;
        }
        if players_close_for_spawning(world, ChunkPos::from_block_pos(pos)).is_empty() {
            return;
        }

        // Vanilla walks down to the first block that is not a portal block.
        let mut spawn_pos = pos;
        while world.get_block_state(spawn_pos).get_block() == self.block {
            spawn_pos = spawn_pos.below();
        }

        let spawn_state = world.get_block_state(spawn_pos);
        if !is_valid_spawn_on_block(
            world.as_ref(),
            spawn_pos,
            spawn_state,
            &vanilla_entities::ZOMBIFIED_PIGLIN,
        ) {
            return;
        }

        let Ok(entity) = spawn_entity(
            world,
            EntitySpawnRequest {
                entity_type: &vanilla_entities::ZOMBIFIED_PIGLIN,
                placement: EntitySpawnPlacement::Block {
                    pos: spawn_pos.above(),
                    try_move_down: false,
                    moved_up: false,
                },
                reason: EntitySpawnReason::Structure,
                finalize_spawn: true,
                play_ambient_sound: true,
                item_stack: None,
                user_is_operator: false,
            },
        ) else {
            return;
        };

        entity.set_portal_cooldown(entity.dimension_changing_delay());
        if let Some(vehicle) = entity.vehicle() {
            vehicle.set_portal_cooldown(vehicle.dimension_changing_delay());
        }
    }

    fn entity_inside(
        &self,
        _state: BlockStateId,
        _world: &Arc<World>,
        pos: BlockPos,
        entity: &dyn Entity,
        _effect_collector: &mut InsideBlockEffectCollector,
        _is_precise: bool,
    ) {
        Self::apply_entity_inside(pos, entity);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::entity_type::EntityTypeRef;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::*;
    use crate::entity::EntityBase;

    struct TestEntity {
        base: EntityBase,
    }

    impl TestEntity {
        fn new() -> Self {
            Self {
                base: EntityBase::new(
                    1,
                    DVec3::ZERO,
                    vanilla_entities::ITEM.dimensions,
                    Weak::new(),
                ),
            }
        }
    }

    crate::entity::impl_test_downcast_type!(TestEntity);

    impl Entity for TestEntity {
        fn base(&self) -> &EntityBase {
            &self.base
        }

        fn entity_type(&self) -> EntityTypeRef {
            &vanilla_entities::ITEM
        }
    }

    #[test]
    fn nether_portal_marks_entity_inside_portal() {
        init_vanilla_registry();
        let entity = TestEntity::new();
        let pos = BlockPos::new(3, 70, 3);

        NetherPortalBlock::apply_entity_inside(pos, &entity);

        let process = entity.base().portal_process().expect("portal process");
        assert_eq!(process.portal(), PortalKind::Nether);
        assert_eq!(process.entry_position(), pos);
    }
}
