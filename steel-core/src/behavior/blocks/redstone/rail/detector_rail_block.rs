use std::sync::Arc;

use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, BoolProperty};
use steel_utils::Downcast as _;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, Direction, WorldAabb};

use crate::behavior::blocks::redstone::{MAX_REDSTONE_SIGNAL, MIN_REDSTONE_SIGNAL};
use crate::behavior::{BlockBehavior, BlockPlaceContext, RailBehavior};
use crate::entity::entities::objects::CommandBlockMinecartEntity;
use crate::entity::{Entity, InsideBlockEffectCollector, container_of_entity};
use crate::inventory::container::calculate_redstone_signal_from_container;
use crate::inventory::lock::ContainerLockGuard;
use crate::world::{LevelReader, ScheduledTickAccess, SignalQueryContext, World};

use super::base_rail_block::BaseRailBlock;
use super::rail_state::RailState;

/// Vanilla detector rail digital behavior.
#[block_behavior]
pub struct DetectorRailBlock {
    base: BaseRailBlock,
}

const POWERED: &BoolProperty = &BlockStateProperties::POWERED;

impl DetectorRailBlock {
    const PRESSED_CHECK_PERIOD: i32 = 20;
    const SEARCH_INSET: f64 = 0.2;

    /// Creates detector rail behavior for `block`.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self {
            base: BaseRailBlock::new(block, true),
        }
    }

    fn search_bounds(pos: BlockPos) -> WorldAabb {
        WorldAabb::new(
            f64::from(pos.x()) + Self::SEARCH_INSET,
            f64::from(pos.y()),
            f64::from(pos.z()) + Self::SEARCH_INSET,
            f64::from(pos.x() + 1) - Self::SEARCH_INSET,
            f64::from(pos.y() + 1) - Self::SEARCH_INSET,
            f64::from(pos.z() + 1) - Self::SEARCH_INSET,
        )
    }

    fn has_interacting_minecart(world: &World, pos: BlockPos) -> bool {
        world.has_entity_in_aabb_matching(&Self::search_bounds(pos), |entity| {
            entity.entity_type().is_abstract_minecart
        })
    }

    fn update_power_to_connected(world: &Arc<World>, pos: BlockPos, state: BlockStateId) {
        let Some(rail) = RailState::new(world, pos, state) else {
            return;
        };
        for connection_pos in rail.connections() {
            let connection_state = world.get_block_state(*connection_pos);
            world.neighbor_changed_with_state(
                connection_state,
                *connection_pos,
                connection_state.get_block(),
                false,
            );
        }
    }

    fn check_pressed(&self, world: &Arc<World>, pos: BlockPos, state: BlockStateId) {
        // Vanilla also bails unless the state is still this rail block itself.
        if !BaseRailBlock::can_survive(world.as_ref(), pos)
            || world.get_block_state(pos).get_block() != self.base.block
        {
            return;
        }

        let was_pressed = state.get_value(POWERED);
        let should_be_pressed = Self::has_interacting_minecart(world, pos);
        if should_be_pressed != was_pressed {
            let new_state = state.set_value(POWERED, should_be_pressed);
            world.set_block(pos, new_state, UpdateFlags::UPDATE_ALL);
            Self::update_power_to_connected(world, pos, new_state);
            world.update_neighbors_at(pos, self.base.block);
            world.update_neighbors_at(pos.below(), self.base.block);
            // Vanilla's setBlocksDirty is client rendering bookkeeping and has
            // no additional dedicated-server side effect.
        }

        if should_be_pressed {
            world.schedule_block_tick_default(pos, self.base.block, Self::PRESSED_CHECK_PERIOD);
        }
        world.update_neighbor_for_output_signal(pos, self.base.block);
    }

    fn signal(state: BlockStateId) -> i32 {
        if state.get_value(POWERED) {
            MAX_REDSTONE_SIGNAL
        } else {
            MIN_REDSTONE_SIGNAL
        }
    }
}

impl RailBehavior for DetectorRailBlock {
    fn is_straight(&self) -> bool {
        self.base.is_straight()
    }
}

impl BlockBehavior for DetectorRailBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(self.base.state_for_placement(context))
    }

    fn can_survive(&self, _state: BlockStateId, world: &dyn LevelReader, pos: BlockPos) -> bool {
        BaseRailBlock::can_survive(world, pos)
    }

    fn update_shape(
        &self,
        state: BlockStateId,
        world: &dyn ScheduledTickAccess,
        pos: BlockPos,
        _direction: Direction,
        _neighbor_pos: BlockPos,
        _neighbor_state: BlockStateId,
    ) -> BlockStateId {
        BaseRailBlock::update_shape(state, world, pos)
    }

    fn on_place(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        old_state: BlockStateId,
        moved_by_piston: bool,
    ) {
        if old_state.get_block() == self.base.block {
            return;
        }
        let updated = self
            .base
            .update_state_on_place(state, world, pos, moved_by_piston);
        self.check_pressed(world, pos, updated);
    }

    fn handle_neighbor_changed(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _source_block: BlockRef,
        moved_by_piston: bool,
    ) {
        self.base
            .handle_neighbor_changed(state, world, pos, moved_by_piston);
    }

    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        if state.get_value(POWERED) {
            self.check_pressed(world, pos, state);
        }
    }

    fn entity_inside(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _entity: &dyn Entity,
        _effect_collector: &mut InsideBlockEffectCollector,
        _is_precise: bool,
    ) {
        if !state.get_value(POWERED) {
            self.check_pressed(world, pos, state);
        }
    }

    fn affect_neighbors_after_removal(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        moved_by_piston: bool,
    ) {
        self.base
            .affect_neighbors_after_removal(state, world, pos, moved_by_piston);
    }

    fn is_signal_source(&self, _state: BlockStateId, _context: SignalQueryContext) -> bool {
        true
    }

    fn get_own_signal(
        &self,
        state: BlockStateId,
        _world: &dyn LevelReader,
        _pos: BlockPos,
        _context: SignalQueryContext,
    ) -> i32 {
        Self::signal(state)
    }

    fn get_direct_signal(
        &self,
        state: BlockStateId,
        _world: &dyn LevelReader,
        _pos: BlockPos,
        direction: Direction,
        _context: SignalQueryContext,
    ) -> i32 {
        if direction == Direction::Up {
            Self::signal(state)
        } else {
            MIN_REDSTONE_SIGNAL
        }
    }

    fn has_analog_output_signal(&self, _state: BlockStateId) -> bool {
        true
    }

    fn get_analog_output_signal(
        &self,
        state: BlockStateId,
        world: &dyn LevelReader,
        pos: BlockPos,
        _direction: Direction,
    ) -> i32 {
        if !state.get_value(POWERED) {
            return MIN_REDSTONE_SIGNAL;
        }

        // Vanilla `DetectorRailBlock.getAnalogOutputSignal` scans the search box
        // for a command-block minecart first, then for a container minecart.
        let entities = world.entities_in_aabb(&Self::search_bounds(pos));

        for entity in &entities {
            if let Some(command_cart) = entity.downcast_ref::<CommandBlockMinecartEntity>() {
                return command_cart.success_count();
            }
        }

        for entity in &entities {
            if !entity.entity_type().is_abstract_minecart || !entity.is_alive() {
                continue;
            }
            let Some(container) = container_of_entity(entity.as_ref()) else {
                continue;
            };
            let guard = ContainerLockGuard::lock_all(&[&container]);
            return guard.get(container.container_id()).map_or(
                MIN_REDSTONE_SIGNAL,
                calculate_redstone_signal_from_container,
            );
        }

        MIN_REDSTONE_SIGNAL
    }

    fn as_rail(&self) -> Option<&dyn RailBehavior> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::DVec3;
    use steel_registry::init_vanilla_registry;
    use steel_registry::item_stack::ItemStack;
    use steel_registry::{vanilla_blocks, vanilla_entities, vanilla_items};
    use steel_utils::ChunkPos;

    use super::*;
    use crate::behavior::{BLOCK_BEHAVIORS, init_behaviors};
    use crate::entity::entities::objects::HopperMinecartEntity;
    use crate::entity::{InsideBlockEffectCollector, RemovalReason, SharedEntity};
    use crate::inventory::container::Container;
    use crate::test_support::{TestEntity, fresh_test_world, insert_ready_full_chunk};

    #[test]
    fn minecart_powers_detector_and_schedules_relative_tick() {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world("detector_rail_minecart");
        let world = &world_fixture.world;
        let pos = BlockPos::new(8, 64, 8);
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));
        world.set_block(
            pos.below(),
            vanilla_blocks::STONE.default_state(),
            UpdateFlags::UPDATE_NONE,
        );
        let state = vanilla_blocks::DETECTOR_RAIL.default_state();
        world.set_block(pos, state, UpdateFlags::UPDATE_NONE);

        let minecart: SharedEntity = TestEntity::shared(
            8_001,
            DVec3::new(8.5, 64.0, 8.5),
            Arc::downgrade(world),
            &vanilla_entities::MINECART,
        );
        world
            .try_add_entity(Arc::clone(&minecart))
            .expect("test minecart should enter loaded chunk");

        let behavior = BLOCK_BEHAVIORS.get_behavior(&vanilla_blocks::DETECTOR_RAIL);
        let mut effects = InsideBlockEffectCollector::new();
        behavior.entity_inside(state, world, pos, minecart.as_ref(), &mut effects, true);

        let powered = world.get_block_state(pos);
        assert!(powered.get_value(POWERED));
        assert_eq!(
            behavior.get_own_signal(powered, world.as_ref(), pos, SignalQueryContext::DEFAULT,),
            15
        );
        assert_eq!(
            behavior.get_direct_signal(
                powered,
                world.as_ref(),
                pos,
                Direction::Up,
                SignalQueryContext::DEFAULT,
            ),
            15
        );
        assert_eq!(
            behavior.get_direct_signal(
                powered,
                world.as_ref(),
                pos,
                Direction::North,
                SignalQueryContext::DEFAULT,
            ),
            0
        );
        assert!(world.has_scheduled_block_tick(pos, &vanilla_blocks::DETECTOR_RAIL));

        minecart.set_removed(RemovalReason::Discarded);
        behavior.tick(powered, world, pos);
        assert!(!world.get_block_state(pos).get_value(POWERED));
    }

    #[test]
    fn container_minecart_contents_drive_analog_output() {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world("detector_rail_analog");
        let world = &world_fixture.world;
        let pos = BlockPos::new(8, 64, 8);
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));
        world.set_block(
            pos.below(),
            vanilla_blocks::STONE.default_state(),
            UpdateFlags::UPDATE_NONE,
        );
        let state = vanilla_blocks::DETECTOR_RAIL.default_state();
        world.set_block(pos, state, UpdateFlags::UPDATE_NONE);

        let cart = HopperMinecartEntity::new(
            &vanilla_entities::HOPPER_MINECART,
            8_002,
            DVec3::new(8.5, 64.0, 8.5),
            Arc::downgrade(world),
        );
        cart.with_inventory_mut(|inventory| {
            inventory.set_item(0, ItemStack::with_count(&vanilla_items::STONE, 64));
            inventory.set_item(1, ItemStack::with_count(&vanilla_items::STONE, 64));
        });
        let cart: SharedEntity = Arc::new(cart);
        world
            .try_add_entity(Arc::clone(&cart))
            .expect("test minecart should enter loaded chunk");

        let powered = state.set_value(POWERED, true);
        world.set_block(pos, powered, UpdateFlags::UPDATE_NONE);

        let behavior = BLOCK_BEHAVIORS.get_behavior(&vanilla_blocks::DETECTOR_RAIL);
        // Vanilla `AbstractContainerMenu.getRedstoneSignalFromContainer`: two full
        // stacks across five slots is 2/5, curving to floor(0.4 * 14) + 1 = 6.
        assert_eq!(
            behavior.get_analog_output_signal(powered, world.as_ref(), pos, Direction::Up),
            6
        );

        // An unpowered rail never emits an analogue signal.
        assert_eq!(
            behavior.get_analog_output_signal(state, world.as_ref(), pos, Direction::Up),
            0
        );
    }
}
