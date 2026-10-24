//! Vanilla redstone repeater behavior.

use std::sync::Arc;

use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{
    BlockStateProperties, BoolProperty, Direction, EnumProperty, IntProperty,
};
use steel_registry::{REGISTRY, vanilla_blocks};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId};

use super::base::DiodeBlock;
use crate::behavior::blocks::redstone::MAX_REDSTONE_SIGNAL;
use crate::behavior::{
    BlockBehavior, BlockHitResult, BlockPlaceContext, InteractionResult, InventoryAccess,
    PlacementSource,
};
use crate::player::Player;
use crate::world::{LevelReader, ScheduledTickAccess, SignalQueryContext, World};

/// Vanilla `RepeaterBlock`, including side locking and scheduled pulse behavior.
#[block_behavior]
pub struct RepeaterBlock {
    diode: DiodeBlock,
}

const DELAY: &IntProperty = &BlockStateProperties::DELAY;
const HORIZONTAL_FACING: &EnumProperty<Direction> = &BlockStateProperties::HORIZONTAL_FACING;
const LOCKED: &BoolProperty = &BlockStateProperties::LOCKED;

impl RepeaterBlock {
    /// Creates a repeater behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self {
            diode: DiodeBlock::new(block),
        }
    }

    fn delay(state: BlockStateId) -> i32 {
        i32::from(state.get_value(DELAY)) * 2
    }

    fn is_locked_at(level: &dyn LevelReader, pos: BlockPos, state: BlockStateId) -> bool {
        DiodeBlock::get_alternate_signal(level, pos, state, true) > 0
    }

    fn should_turn_on(level: &dyn LevelReader, pos: BlockPos, state: BlockStateId) -> bool {
        DiodeBlock::get_input_signal(level, pos, state) > 0
    }

    fn check_tick_on_neighbor(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        self.diode.check_tick_on_neighbor(
            world,
            pos,
            state,
            Self::is_locked_at(world.as_ref(), pos, state),
            Self::should_turn_on(world.as_ref(), pos, state),
            Self::delay(state),
        );
    }
}

impl BlockBehavior for RepeaterBlock {
    fn can_survive(&self, _state: BlockStateId, world: &dyn LevelReader, pos: BlockPos) -> bool {
        DiodeBlock::can_survive(world, pos)
    }

    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        let state = self.diode.state_for_placement(context);
        Some(state.set_value(
            LOCKED,
            Self::is_locked_at(context.world.as_ref(), context.place_pos(), state),
        ))
    }

    fn update_shape(
        &self,
        state: BlockStateId,
        world: &dyn ScheduledTickAccess,
        pos: BlockPos,
        direction: Direction,
        neighbor_pos: BlockPos,
        neighbor_state: BlockStateId,
    ) -> BlockStateId {
        if direction == Direction::Down
            && !DiodeBlock::can_survive_on(world, neighbor_pos, neighbor_state)
        {
            return REGISTRY.blocks.get_default_state_id(&vanilla_blocks::AIR);
        }

        // Server-only port: Vanilla's `!level.isClientSide()` guard has no counterpart here.
        let facing = state.get_value(HORIZONTAL_FACING);
        if direction.get_axis() == facing.get_axis() {
            state
        } else {
            state.set_value(LOCKED, Self::is_locked_at(world, pos, state))
        }
    }

    fn use_without_item(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        if !player.abilities.lock().may_build {
            return InteractionResult::Pass;
        }

        let delay = state.get_value(DELAY);
        let next_delay = if delay == 4 { 1 } else { delay + 1 };
        world.set_block(
            pos,
            state.set_value(DELAY, next_delay),
            UpdateFlags::UPDATE_ALL,
        );
        InteractionResult::Success
    }

    fn handle_neighbor_changed(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _source_block: BlockRef,
        _moved_by_piston: bool,
    ) {
        self.diode.handle_neighbor_changed(state, world, pos, || {
            self.check_tick_on_neighbor(state, world, pos);
        });
    }

    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        self.diode.tick(
            state,
            world,
            pos,
            Self::is_locked_at(world.as_ref(), pos, state),
            Self::should_turn_on(world.as_ref(), pos, state),
            Self::delay(state),
        );
    }

    fn set_placed_by(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _source: &PlacementSource<'_>,
    ) {
        self.diode
            .set_placed_by(world, pos, Self::should_turn_on(world.as_ref(), pos, state));
    }

    fn on_place(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _old_state: BlockStateId,
        _moved_by_piston: bool,
    ) {
        self.diode.on_place(state, world, pos);
    }

    fn affect_neighbors_after_removal(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        moved_by_piston: bool,
    ) {
        self.diode
            .affect_neighbors_after_removal(state, world, pos, moved_by_piston);
    }

    fn is_signal_source(&self, _state: BlockStateId, _context: SignalQueryContext) -> bool {
        true
    }

    fn is_diode(&self) -> bool {
        true
    }

    fn get_own_signal(
        &self,
        state: BlockStateId,
        _world: &dyn LevelReader,
        _pos: BlockPos,
        _context: SignalQueryContext,
    ) -> i32 {
        DiodeBlock::own_signal(state, MAX_REDSTONE_SIGNAL)
    }

    fn get_signal(
        &self,
        state: BlockStateId,
        _world: &dyn LevelReader,
        _pos: BlockPos,
        direction: Direction,
        _context: SignalQueryContext,
    ) -> i32 {
        DiodeBlock::signal(state, direction, MAX_REDSTONE_SIGNAL)
    }

    fn get_direct_signal(
        &self,
        state: BlockStateId,
        world: &dyn LevelReader,
        pos: BlockPos,
        direction: Direction,
        context: SignalQueryContext,
    ) -> i32 {
        self.get_signal(state, world, pos, direction, context)
    }

    // `animateTick` emits client-local dust particles only.
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;
    use steel_utils::ChunkPos;

    use super::*;
    use crate::behavior::init_behaviors;
    use crate::test_support::{TestLevel, fresh_test_world, insert_ready_full_chunk};
    use crate::world::tick_scheduler::TickPriority;

    const POWER: &IntProperty = &BlockStateProperties::POWER;
    const POWERED: &BoolProperty = &BlockStateProperties::POWERED;

    fn repeater() -> RepeaterBlock {
        init_vanilla_registry();
        init_behaviors();
        RepeaterBlock::new(&vanilla_blocks::REPEATER)
    }

    fn repeater_state(facing: Direction, powered: bool) -> BlockStateId {
        vanilla_blocks::REPEATER
            .default_state()
            .set_value(HORIZONTAL_FACING, facing)
            .set_value(POWERED, powered)
    }

    #[test]
    fn input_reads_wire_power_in_front() {
        let _repeater = repeater();
        let pos = BlockPos::new(0, 64, 0);
        let state = repeater_state(Direction::East, false);
        let wire = vanilla_blocks::REDSTONE_WIRE
            .default_state()
            .set_value(POWER, 7);
        let level = TestLevel::default().with_block(pos.east(), wire);

        assert_eq!(DiodeBlock::get_input_signal(&level, pos, state), 7);
    }

    #[test]
    fn side_lock_accepts_only_another_diode() {
        let _repeater = repeater();
        let pos = BlockPos::new(0, 64, 0);
        let state = repeater_state(Direction::North, false);
        let side_repeater = repeater_state(Direction::East, true);
        let level = TestLevel::default().with_block(pos.east(), side_repeater);
        assert!(RepeaterBlock::is_locked_at(&level, pos, state));

        let redstone_block_level = TestLevel::default()
            .with_block(pos.east(), vanilla_blocks::REDSTONE_BLOCK.default_state());
        assert!(!RepeaterBlock::is_locked_at(
            &redstone_block_level,
            pos,
            state
        ));
    }

    #[test]
    fn powered_output_is_directional() {
        let repeater = repeater();
        let state = repeater_state(Direction::West, true);
        let level = TestLevel::default();
        let pos = BlockPos::new(0, 64, 0);

        assert_eq!(
            repeater.get_signal(
                state,
                &level,
                pos,
                Direction::West,
                SignalQueryContext::DEFAULT,
            ),
            15
        );
        assert_eq!(
            repeater.get_signal(
                state,
                &level,
                pos,
                Direction::East,
                SignalQueryContext::DEFAULT,
            ),
            0
        );
    }

    #[test]
    fn support_requires_vanilla_rigid_face() {
        let repeater = repeater();
        let pos = BlockPos::new(0, 64, 0);
        let stone =
            TestLevel::default().with_block(pos.below(), vanilla_blocks::STONE.default_state());
        let air = TestLevel::default();

        assert!(repeater.can_survive(vanilla_blocks::REPEATER.default_state(), &stone, pos));
        assert!(!repeater.can_survive(vanilla_blocks::REPEATER.default_state(), &air, pos));
    }

    #[test]
    fn neighbor_tick_priority_matches_vanilla_diode_ordering() {
        let _repeater = repeater();
        let pos = BlockPos::new(0, 64, 0);
        let off = repeater_state(Direction::North, false);
        let on = repeater_state(Direction::North, true);
        let plain_level = TestLevel::default();

        assert_eq!(
            DiodeBlock::tick_priority(&plain_level, pos, off, false),
            TickPriority::High
        );
        assert_eq!(
            DiodeBlock::tick_priority(&plain_level, pos, on, true),
            TickPriority::VeryHigh
        );

        let crossing_diode = repeater_state(Direction::East, false);
        let prioritized_level = TestLevel::default().with_block(pos.south(), crossing_diode);
        assert_eq!(
            DiodeBlock::tick_priority(&prioritized_level, pos, off, false),
            TickPriority::ExtremelyHigh
        );
    }

    #[test]
    fn update_shape_recomputes_locked_only_across_the_diode_axis() {
        let repeater = repeater();
        let pos = BlockPos::new(0, 64, 0);
        let state = repeater_state(Direction::North, false);
        let side_repeater = repeater_state(Direction::East, true);
        let level = TestLevel::default().with_block(pos.east(), side_repeater);

        // Vanilla: `directionToNeighbour.getAxis() != FACING.getAxis()` re-reads LOCKED.
        let locked = repeater.update_shape(
            state,
            &level,
            pos,
            Direction::East,
            pos.east(),
            side_repeater,
        );
        assert!(locked.get_value(LOCKED));

        // A change along the diode axis falls through to `DiodeBlock.updateShape`, which
        // returns the state untouched.
        let unchanged = repeater.update_shape(
            state,
            &level,
            pos,
            Direction::North,
            pos.north(),
            vanilla_blocks::AIR.default_state(),
        );
        assert_eq!(unchanged, state);
    }

    /// Vanilla placement flags that suppress the placement callbacks.
    ///
    /// Redstone wire recomputes `POWER` from real sources in `onPlace`, so a fixture
    /// that wants a specific analog level must place the pre-powered state without
    /// running the wire's placement evaluation.
    fn raw_flags() -> UpdateFlags {
        UpdateFlags::UPDATE_NONE | UpdateFlags::UPDATE_SKIP_ON_PLACE
    }

    #[test]
    fn locked_repeater_ignores_scheduled_pulses_and_unlocks_after_input_removal() {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world("repeater_locked_pulse");
        let world = &world_fixture.world;
        let pos = BlockPos::new(8, 64, 8);
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));

        // Vanilla repeaters and wire require a sturdy block below them and are
        // dropped by the first neighbor update otherwise, so stand the circuit on
        // stone.
        for support_pos in [pos.below(), pos.north().below(), pos.east().below()] {
            assert!(world.set_block(
                support_pos,
                vanilla_blocks::STONE.default_state(),
                UpdateFlags::UPDATE_NONE,
            ));
        }

        let behavior = RepeaterBlock::new(&vanilla_blocks::REPEATER);
        let powered_input = vanilla_blocks::REDSTONE_WIRE
            .default_state()
            .set_value(POWER, 7);
        assert!(world.set_block(pos.north(), powered_input, raw_flags()));
        // A powered repeater pointing into our side is what vanilla `isLocked` reads.
        let side_repeater = repeater_state(Direction::East, true);
        assert!(world.set_block(pos.east(), side_repeater, raw_flags()));

        let state = repeater_state(Direction::North, false);
        assert!(world.set_block(pos, state, raw_flags()));
        behavior.tick(state, world, pos);
        assert_eq!(
            world.get_block_state(pos),
            state,
            "vanilla `DiodeBlock.tick` returns early while the repeater is locked"
        );

        // Unlocking lets the same tick turn the repeater on from its wire input.
        assert!(world.set_block(
            pos.east(),
            vanilla_blocks::AIR.default_state(),
            UpdateFlags::UPDATE_NONE,
        ));
        behavior.tick(state, world, pos);
        let turned_on = world.get_block_state(pos);
        assert!(turned_on.get_value(POWERED));
        assert!(
            !world.has_scheduled_block_tick(pos, &vanilla_blocks::REPEATER),
            "turning on from a live input does not schedule another tick"
        );

        // With the input gone the next scheduled tick turns it back off.
        assert!(world.set_block(
            pos.north(),
            vanilla_blocks::AIR.default_state(),
            UpdateFlags::UPDATE_NONE,
        ));
        behavior.tick(turned_on, world, pos);
        assert!(!world.get_block_state(pos).get_value(POWERED));
    }
}
