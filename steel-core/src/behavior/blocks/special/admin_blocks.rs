//! Admin-block behaviors: `CommandBlock`, `StructureBlock`, `JigsawBlock`, `TestBlock` and
//! `TestInstanceBlock`.
//!
//! All five are vanilla `GameMasterBlock`s: only creative-mode operators with game-master
//! permission may edit them. The heavy lifting lives in their block entities; the block layer owns
//! placement, redstone triggering and comparator output, exactly like vanilla.
//!
//! Deviation: the structure/jigsaw/test/test-instance editor screens are opened by the vanilla
//! client from its own block-entity data; Steel sends no dedicated open-editor packet for them
//! (owner: protocol lane). Only the command-block editor is pushed explicitly, via
//! `CBlockEntityData`, matching vanilla `ServerPlayer.openCommandBlock`.

use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_registry::RegistryEntry as _;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{
    BlockStateProperties, Direction, EnumProperty, FrontAndTop, StructureMode, TestBlockMode,
};
use steel_registry::vanilla_block_entity_types;
use steel_utils::{BlockPos, BlockStateId, Downcast as _};

use crate::behavior::{
    BlockBehavior, BlockEntityCreation, BlockHitResult, BlockPlaceContext, InteractionResult,
    InventoryAccess,
};
use crate::block_entity::entities::{
    CommandBlockEntity, CommandBlockMode, StructureBlockEntity, TestInstanceBlockEntity,
};
use crate::block_entity::{BLOCK_ENTITIES, BlockEntityTicker};
use crate::player::Player;
use crate::world::{LevelReader, SignalGetter as _, SignalQueryContext, World};

const FACING: &EnumProperty<Direction> = &BlockStateProperties::FACING;
const STRUCTURE_MODE: &EnumProperty<StructureMode> = &BlockStateProperties::STRUCTUREBLOCK_MODE;
const ORIENTATION: &EnumProperty<FrontAndTop> = &BlockStateProperties::ORIENTATION;
const TEST_BLOCK_MODE: &EnumProperty<TestBlockMode> = &BlockStateProperties::TEST_BLOCK_MODE;

/// Vanilla `FrontAndTop.fromFrontAndTop`: builds the jigsaw orientation from the clicked face and
/// the placer's horizontal direction.
const fn front_and_top(front: Direction, top: Direction) -> FrontAndTop {
    match (front, top) {
        (Direction::Up, Direction::East) => FrontAndTop::UpEast,
        (Direction::Up, Direction::North) => FrontAndTop::UpNorth,
        (Direction::Up, Direction::South) => FrontAndTop::UpSouth,
        (Direction::Up, _) => FrontAndTop::UpWest,
        (Direction::Down, Direction::East) => FrontAndTop::DownEast,
        (Direction::Down, Direction::North) => FrontAndTop::DownNorth,
        (Direction::Down, Direction::South) => FrontAndTop::DownSouth,
        (Direction::Down, _) => FrontAndTop::DownWest,
        (Direction::North, _) => FrontAndTop::NorthUp,
        (Direction::East, _) => FrontAndTop::EastUp,
        (Direction::South, _) => FrontAndTop::SouthUp,
        (Direction::West, _) => FrontAndTop::WestUp,
    }
}

// ---------------------------------------------------------------------------
// CommandBlock
// ---------------------------------------------------------------------------

/// `CommandBlock` behavior (`command_block` / `chain_command_block` / `repeating_command_block`).
///
/// Vanilla models all three with the same class plus an `automatic` constructor flag and a `Mode`
/// stored in the block entity, so one behavior covers them.
#[block_behavior]
pub struct CommandBlock {
    block: BlockRef,
}

impl CommandBlock {
    /// Creates a command-block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }
}

impl BlockBehavior for CommandBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(
            self.block
                .default_state()
                .set_value(FACING, context.horizontal_direction().opposite()),
        )
    }

    fn handle_neighbor_changed(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _source_block: BlockRef,
        _moved_by_piston: bool,
    ) {
        let Some(entity) = world.get_block_entity(pos) else {
            return;
        };
        let Some(command_be) = entity.downcast_ref::<CommandBlockEntity>() else {
            return;
        };
        // Vanilla `CommandBlock.setPoweredAndUpdate`: on a rising edge an automatic
        // (chain) or sequence block never schedules its own tick; otherwise the block
        // marks its condition met and schedules the one-tick execution.
        let is_powered = world.has_neighbor_signal(pos);
        // Vanilla `CommandBlock.setPoweredAndUpdate`: compare against the previous state
        // first and do nothing when the edge did not change. `set_powered` returns the
        // *previous* value, so a rising edge (false -> true) must not be mistaken for
        // "unchanged".
        let previous = command_be.set_powered(is_powered);
        if previous == is_powered || !is_powered {
            return;
        }
        // Vanilla: an automatic (chain) or sequence block never schedules its own tick.
        if command_be.is_automatic() || command_be.mode() == CommandBlockMode::Sequence {
            return;
        }
        command_be.mark_condition_met();
        world.schedule_block_tick_default(pos, self.block, 1);
    }

    fn tick(&self, _state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        let Some(entity) = world.get_block_entity(pos) else {
            return;
        };
        let Some(command_be) = entity.downcast_ref::<CommandBlockEntity>() else {
            return;
        };
        command_be.server_tick(world, pos, self.block);
    }

    fn use_without_item(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        use steel_protocol::packets::game::CBlockEntityData;
        use steel_utils::serial::OptionalNbt;

        // Vanilla `CommandBlock.useWithoutItem`: only game-master players may open the editor.
        if !player.can_use_game_master_blocks() {
            return InteractionResult::Pass;
        }
        let Some(entity) = world.get_block_entity(pos) else {
            return InteractionResult::Pass;
        };
        // Vanilla `ServerPlayer.openCommandBlock`: the editor opens on the client once it
        // receives the block entity's custom-only data.
        player.send_packet(CBlockEntityData {
            pos,
            block_entity_type: entity.get_type().id() as i32,
            nbt: OptionalNbt(Some(entity.save_custom_only())),
        });
        InteractionResult::Success
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
            &vanilla_block_entity_types::COMMAND_BLOCK,
            level,
            pos,
            state,
        ))
    }

    fn has_analog_output_signal(&self, _state: BlockStateId) -> bool {
        true
    }

    fn get_analog_output_signal(
        &self,
        _state: BlockStateId,
        world: &dyn LevelReader,
        pos: BlockPos,
        _direction: Direction,
    ) -> i32 {
        // Vanilla `CommandBlock.getAnalogOutputSignal` → `BaseCommandBlock.getSuccessCount`.
        let Some(entity) = world.get_block_entity(pos) else {
            return 0;
        };
        entity
            .downcast_ref::<CommandBlockEntity>()
            .map_or(0, |command_be| command_be.success_count().clamp(0, 15))
    }
}

// ---------------------------------------------------------------------------
// StructureBlock
// ---------------------------------------------------------------------------

/// `StructureBlock` behavior.
#[block_behavior]
pub struct StructureBlock {
    block: BlockRef,
}

impl StructureBlock {
    /// Creates the behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }
}

impl BlockBehavior for StructureBlock {
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(
            self.block
                .default_state()
                .set_value(STRUCTURE_MODE, StructureMode::Load),
        )
    }

    fn handle_neighbor_changed(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _source_block: BlockRef,
        _moved_by_piston: bool,
    ) {
        let Some(entity) = world.get_block_entity(pos) else {
            return;
        };
        let Some(structure_be) = entity.downcast_ref::<StructureBlockEntity>() else {
            return;
        };
        // Vanilla `StructureBlock.neighborChanged`: rising edge triggers, falling edge clears.
        let powered = world.has_neighbor_signal(pos);
        if powered && !structure_be.is_powered() {
            structure_be.set_powered(true);
            StructureBlockEntity::trigger(world, pos);
        } else if !powered {
            structure_be.set_powered(false);
        }
    }

    fn use_without_item(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        // Vanilla `StructureBlockEntity.usedBy`: game-master players get success; the editor
        // screen itself is opened client-side.
        if !player.can_use_game_master_blocks() || world.get_block_entity(pos).is_none() {
            return InteractionResult::Pass;
        }
        InteractionResult::Success
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
            &vanilla_block_entity_types::STRUCTURE_BLOCK,
            level,
            pos,
            state,
        ))
    }
}

// ---------------------------------------------------------------------------
// JigsawBlock
// ---------------------------------------------------------------------------

/// `JigsawBlock` behavior.
#[block_behavior]
pub struct JigsawBlock {
    block: BlockRef,
}

impl JigsawBlock {
    /// Creates the behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }
}

impl BlockBehavior for JigsawBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(self.block.default_state().set_value(
            ORIENTATION,
            front_and_top(context.clicked_face(), context.horizontal_direction()),
        ))
    }

    fn use_without_item(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        // Vanilla `JigsawBlock.useWithoutItem`: game-master players only; the editor screen is
        // opened client-side.
        if !player.can_use_game_master_blocks() || world.get_block_entity(pos).is_none() {
            return InteractionResult::Pass;
        }
        InteractionResult::Success
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
            &vanilla_block_entity_types::JIGSAW,
            level,
            pos,
            state,
        ))
    }
}

// ---------------------------------------------------------------------------
// TestBlock / TestInstanceBlock
// ---------------------------------------------------------------------------

/// `TestBlock` behavior.
#[block_behavior]
pub struct TestBlock {
    block: BlockRef,
}

impl TestBlock {
    /// Creates the behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }
}

impl BlockBehavior for TestBlock {
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(
            self.block
                .default_state()
                .set_value(TEST_BLOCK_MODE, TestBlockMode::Start),
        )
    }

    fn use_without_item(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        // Vanilla `TestBlock.useWithoutItem`: game-master players only; the editor screen is
        // opened client-side.
        if !player.can_use_game_master_blocks() || world.get_block_entity(pos).is_none() {
            return InteractionResult::Pass;
        }
        InteractionResult::Success
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
            &vanilla_block_entity_types::TEST_BLOCK,
            level,
            pos,
            state,
        ))
    }
}

/// `TestInstanceBlock` behavior.
#[block_behavior]
pub struct TestInstanceBlock {
    block: BlockRef,
}

impl TestInstanceBlock {
    /// Creates the behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }
}

impl BlockBehavior for TestInstanceBlock {
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(self.block.default_state())
    }

    fn use_without_item(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        // Vanilla `TestInstanceBlock.useWithoutItem`: game-master players only; the editor screen
        // is opened client-side.
        if !player.can_use_game_master_blocks() || world.get_block_entity(pos).is_none() {
            return InteractionResult::Pass;
        }
        InteractionResult::Success
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
            &vanilla_block_entity_types::TEST_INSTANCE_BLOCK,
            level,
            pos,
            state,
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
            &vanilla_block_entity_types::TEST_INSTANCE_BLOCK,
        )
    }

    fn is_signal_source(&self, _state: BlockStateId, _context: SignalQueryContext) -> bool {
        true
    }

    fn get_signal(
        &self,
        _state: BlockStateId,
        world: &dyn LevelReader,
        pos: BlockPos,
        direction: Direction,
        _context: SignalQueryContext,
    ) -> i32 {
        // Vanilla `TestInstanceBlock.getSignal`: 15 upward while the test completed successfully.
        if direction != Direction::Up {
            return 0;
        }
        let Some(entity) = world.get_block_entity(pos) else {
            return 0;
        };
        entity
            .downcast_ref::<TestInstanceBlockEntity>()
            .map_or(0, |test_be| if test_be.is_complete() { 15 } else { 0 })
    }
}
