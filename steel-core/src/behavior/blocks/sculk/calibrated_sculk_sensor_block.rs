//! Vanilla `CalibratedSculkSensorBlock`.
//!
//! Vanilla reference: `net.minecraft.world.level.block.CalibratedSculkSensorBlock extends
//! SculkSensorBlock`. Same phase/power/waterlogged states plus `HORIZONTAL_FACING`; output is
//! directional (no signal toward `FACING`, full `POWER` elsewhere). Resonance filtering and the
//! 16-block radius live in
//! [`CalibratedSculkSensorBlockEntity`](CalibratedSculkSensorBlockEntity);
//! this behavior owns placement, redstone output (10 active ticks, Vanilla `getActiveTicks`) and
//! direct-signal rules.

use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{
    BlockStateProperties, BoolProperty, Direction, EnumProperty, IntProperty, SculkSensorPhase,
};
use steel_registry::item_stack::ItemStack;
use steel_registry::{sound_events, vanilla_block_entity_types, vanilla_game_events};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, value_providers::IntProvider};

use crate::behavior::block::schedule_water_tick_if_waterlogged;
use crate::behavior::blocks::sculk::sculk_sensor_block::SculkSensorBlock;
use crate::behavior::{BlockBehavior, BlockEntityCreation, BlockPlaceContext, try_drop_experience};
use crate::block_entity::BlockEntityTicker;
use crate::block_entity::entities::{CalibratedSculkSensorBlockEntity, SculkSensorBlockEntity};
use crate::entity::Entity;
use crate::entity::ai::path::PathComputationType;
use crate::world::game_event::GameEventContext;
use crate::world::{LevelReader, ScheduledTickAccess, SignalQueryContext, World};

/// Vanilla `CalibratedSculkSensorBlock.getActiveTicks`.
pub const CALIBRATED_SCULK_SENSOR_ACTIVE_TICKS: i32 = 10;

/// Vanilla `CalibratedSculkSensorBlock`.
#[block_behavior]
pub struct CalibratedSculkSensorBlock {
    block: BlockRef,
}

const PHASE: &EnumProperty<SculkSensorPhase> = &BlockStateProperties::SCULK_SENSOR_PHASE;
const POWER: &IntProperty = &BlockStateProperties::POWER;
const WATERLOGGED: &BoolProperty = &BlockStateProperties::WATERLOGGED;
const FACING: &EnumProperty<Direction> = &BlockStateProperties::HORIZONTAL_FACING;

impl CalibratedSculkSensorBlock {
    /// Creates calibrated-sensor behavior for `block`.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Vanilla `CalibratedSculkSensorBlock` activation path: same state machine as the base sensor
    /// with a 10-tick pulse, plus the resonance emission the base `activate` performs.
    pub fn activate(
        block: BlockRef,
        world: &Arc<World>,
        pos: BlockPos,
        state: BlockStateId,
        power: i32,
        vibration_frequency: i32,
        source_entity: Option<&dyn Entity>,
    ) {
        let active = state
            .set_value(PHASE, SculkSensorPhase::Active)
            .set_value(POWER, power.clamp(0, 15) as u8);
        world.set_block(pos, active, UpdateFlags::UPDATE_ALL);
        world.schedule_block_tick_default(pos, block, CALIBRATED_SCULK_SENSOR_ACTIVE_TICKS);
        world.update_neighbors_at(pos, block);
        world.update_neighbors_at(pos.below(), block);
        SculkSensorBlock::try_resonate_vibration(source_entity, world, pos, vibration_frequency);
        // Vanilla `LevelAccessor.gameEvent(entity, event, pos)` builds a state-less context.
        world.game_event(
            &vanilla_game_events::SCULK_SENSOR_TENDRILS_CLICKING,
            pos,
            &GameEventContext::new(source_entity, None),
        );
        if !state.get_value(WATERLOGGED) {
            let pitch = rand::random::<f32>() * 0.2 + 0.8;
            world.play_sound(
                &sound_events::BLOCK_SCULK_SENSOR_CLICKING,
                SoundSource::Blocks,
                pos,
                1.0,
                pitch,
                None,
            );
        }
    }
}

impl BlockBehavior for CalibratedSculkSensorBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(
            self.block
                .default_state()
                .set_value(FACING, context.horizontal_direction())
                .set_value(WATERLOGGED, context.is_water_source()),
        )
    }

    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        // Vanilla does not override `tick`: the calibrated block inherits `SculkSensorBlock.tick`
        // unchanged (only the pulse length differs, via `getActiveTicks`).
        SculkSensorBlock::tick_phase(self.block, world, pos, state);
    }

    fn on_place(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        old_state: BlockStateId,
        _moved_by_piston: bool,
    ) {
        // Vanilla inherits `SculkSensorBlock.onPlace` unchanged.
        SculkSensorBlock::reset_power_on_place(self.block, world, pos, state, old_state);
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
        // Vanilla inherits `SculkSensorBlock.updateShape` (the waterlogged fluid tick).
        schedule_water_tick_if_waterlogged(state, world, pos);
        state
    }

    fn step_on(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos, entity: &dyn Entity) {
        // Vanilla inherits `SculkSensorBlock.stepOn`; Steel must re-declare it because behaviors
        // dispatch per block instead of by Java class inheritance.
        SculkSensorBlock::schedule_step_vibration(world, pos, state, entity);
        self.default_step_on(state, world, pos, entity);
    }

    fn is_pathfindable(
        &self,
        _state: BlockStateId,
        _computation_type: PathComputationType,
    ) -> bool {
        // Vanilla `SculkSensorBlock.isPathfindable` always returns `false`, inherited unchanged.
        false
    }

    fn affect_neighbors_after_removal(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _moved_by_piston: bool,
    ) {
        if SculkSensorBlock::phase(state) == SculkSensorPhase::Active {
            world.update_neighbors_at(pos, self.block);
            world.update_neighbors_at(pos.below(), self.block);
        }
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
        i32::from(state.get_value(POWER))
    }

    fn get_signal(
        &self,
        state: BlockStateId,
        world: &dyn LevelReader,
        pos: BlockPos,
        direction: Direction,
        context: SignalQueryContext,
    ) -> i32 {
        // Vanilla `CalibratedSculkSensorBlock.getSignal`: no signal toward FACING.
        if direction == state.get_value(FACING) {
            0
        } else {
            self.get_own_signal(state, world, pos, context)
        }
    }

    fn get_direct_signal(
        &self,
        state: BlockStateId,
        world: &dyn LevelReader,
        pos: BlockPos,
        direction: Direction,
        context: SignalQueryContext,
    ) -> i32 {
        if direction == Direction::Up {
            self.get_signal(state, world, pos, direction, context)
        } else {
            0
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
        use steel_utils::Downcast as _;
        if SculkSensorBlock::phase(state) != SculkSensorPhase::Active {
            return 0;
        }
        let Some(entity) = world.get_block_entity(pos) else {
            return 0;
        };
        // Calibrated sensors use their own entity type; both variants expose the frequency.
        if let Some(sensor) = entity.downcast_ref::<CalibratedSculkSensorBlockEntity>() {
            return sensor.last_vibration_frequency();
        }
        entity
            .downcast_ref::<SculkSensorBlockEntity>()
            .map_or(0, SculkSensorBlockEntity::last_vibration_frequency)
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::Created(Arc::new(CalibratedSculkSensorBlockEntity::new(
            level, pos, state,
        )))
    }

    fn get_block_entity_ticker(
        &self,
        _world: &Arc<World>,
        _state: BlockStateId,
        block_entity_type: BlockEntityTypeRef,
    ) -> Option<BlockEntityTicker> {
        BlockEntityTicker::for_matching_entity_tick(
            block_entity_type,
            &vanilla_block_entity_types::CALIBRATED_SCULK_SENSOR,
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
            try_drop_experience(world, pos, tool, &IntProvider::Constant(5));
        }
    }
}
