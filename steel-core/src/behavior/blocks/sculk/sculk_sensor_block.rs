//! Vanilla `SculkSensorBlock`, including waterlogging and redstone output.
//!
//! Vanilla reference: `net.minecraft.world.level.block.SculkSensorBlock`
//! (`sculk_sensor_phase` inactive/active/cooldown, `power` 0-15, waterlogged).
//! Activation is driven by [`SculkSensorBlockEntity`](crate::block_entity::entities::SculkSensorBlockEntity)
//! through the vibration listener; this behavior owns placement, redstone output and deactivation
//! ticks.

use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{
    BlockStateProperties, BoolProperty, Direction, IntProperty, SculkSensorPhase,
};
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::{sound_events, vanilla_block_entity_types, vanilla_game_events};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, value_providers::IntProvider};

use crate::behavior::blocks::NoteBlock;
use crate::behavior::{BlockBehavior, BlockEntityCreation, BlockPlaceContext, try_drop_experience};
use crate::block_entity::BlockEntityTicker;
use crate::world::{LevelReader, SignalQueryContext, World};

/// Vanilla `SculkSensorBlock.ACTIVE_TICKS`.
pub const SCULK_SENSOR_ACTIVE_TICKS: i32 = 30;
/// Vanilla `SculkSensorBlock.COOLDOWN_TICKS` (deactivation delay).
pub const SCULK_SENSOR_COOLDOWN_TICKS: i32 = 10;

/// Vanilla `SculkSensorBlock`.
#[block_behavior]
pub struct SculkSensorBlock {
    block: BlockRef,
}

const PHASE: &steel_registry::blocks::properties::EnumProperty<SculkSensorPhase> =
    &BlockStateProperties::SCULK_SENSOR_PHASE;
const POWER: &IntProperty = &BlockStateProperties::POWER;
const WATERLOGGED: &BoolProperty = &BlockStateProperties::WATERLOGGED;

impl SculkSensorBlock {
    /// Creates sculk sensor behavior for `block`.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Vanilla `SculkSensorBlock.getPhase`.
    #[must_use]
    pub fn phase(state: BlockStateId) -> SculkSensorPhase {
        state.get_value(PHASE)
    }

    /// Vanilla `SculkSensorBlock.canActivate`.
    #[must_use]
    pub fn can_activate(state: BlockStateId) -> bool {
        Self::phase(state) == SculkSensorPhase::Inactive
    }

    /// Vanilla `SculkSensorBlock.activate`: turns the sensor active with `power`, schedules the
    /// active tick, updates neighbours and emits the clicking game event.
    ///
    /// Sound playback beyond the game event is server-side audible context handled by listeners;
    /// particle emission (`DustColorTransitionOptions.SCULK_TO_REDSTONE`) is client-local
    /// `animateTick` and has no server equivalent.
    pub fn activate(
        block: BlockRef,
        world: &Arc<World>,
        pos: BlockPos,
        state: BlockStateId,
        power: i32,
        vibration_frequency: i32,
        source_entity: Option<&dyn crate::entity::Entity>,
        active_ticks: i32,
    ) {
        let active = state
            .set_value(PHASE, SculkSensorPhase::Active)
            .set_value(POWER, power.clamp(0, 15) as u8);
        world.set_block(pos, active, UpdateFlags::UPDATE_ALL);
        world.schedule_block_tick_default(pos, block, active_ticks);
        world.update_neighbors_at(pos, block);
        world.update_neighbors_at(pos.below(), block);
        Self::try_resonate_vibration(source_entity, world, pos, vibration_frequency);
        world.game_event(
            &steel_registry::vanilla_game_events::SCULK_SENSOR_TENDRILS_CLICKING,
            pos,
            &crate::world::game_event::GameEventContext::new(source_entity, Some(active)),
        );
        if !state.get_value(WATERLOGGED) {
            // Vanilla `level.getRandom().nextFloat() * 0.2F + 0.8F`; ambient pitch jitter.
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

    /// Vanilla `SculkSensorBlock.tryResonateVibration`: every adjacent
    /// `#vibration_resonators` block receives the matching `RESONATE_n` game event and a
    /// pitch-bent `AMETHYST_BLOCK_RESONATE` sound.
    pub fn try_resonate_vibration(
        source_entity: Option<&dyn crate::entity::Entity>,
        world: &Arc<World>,
        pos: BlockPos,
        vibration_frequency: i32,
    ) {
        let Some(resonance_event) =
            crate::world::game_event::vibration::resonance_event_by_frequency(vibration_frequency)
        else {
            return;
        };
        // Vanilla `RESONANCE_PITCH_BEND`, built from `NoteBlock.getPitchFromNote` over the
        // resonance tone map `{0,0,2,4,6,7,9,10,12,14,15,18,19,21,22,24}`.
        const RESONANCE_TONE_MAP: [u8; 16] =
            [0, 0, 2, 4, 6, 7, 9, 10, 12, 14, 15, 18, 19, 21, 22, 24];
        let pitch = NoteBlock::pitch_from_note(
            RESONANCE_TONE_MAP[vibration_frequency.clamp(0, 15) as usize],
        );
        for direction in Direction::ALL {
            let relative_pos = pos.relative(direction);
            let relative_state = world.get_block_state(relative_pos);
            if !relative_state
                .get_block()
                .has_tag(&BlockTag::VIBRATION_RESONATORS)
            {
                continue;
            }
            world.game_event(
                resonance_event,
                relative_pos,
                &crate::world::game_event::GameEventContext::new(
                    source_entity,
                    Some(relative_state),
                ),
            );
            world.play_sound(
                &sound_events::BLOCK_AMETHYST_BLOCK_RESONATE,
                SoundSource::Blocks,
                relative_pos,
                1.0,
                pitch,
                None,
            );
        }
    }

    /// Vanilla `SculkSensorBlock.deactivate`.
    pub fn deactivate(block: BlockRef, world: &Arc<World>, pos: BlockPos, state: BlockStateId) {
        let cooldown = state
            .set_value(PHASE, SculkSensorPhase::Cooldown)
            .set_value(POWER, 0_u8);
        world.set_block(pos, cooldown, UpdateFlags::UPDATE_ALL);
        world.schedule_block_tick_default(pos, block, SCULK_SENSOR_COOLDOWN_TICKS);
        world.update_neighbors_at(pos, block);
        world.update_neighbors_at(pos.below(), block);
    }
}

impl BlockBehavior for SculkSensorBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(
            self.block
                .default_state()
                .set_value(WATERLOGGED, context.is_water_source()),
        )
    }

    fn update_shape(
        &self,
        state: BlockStateId,
        world: &dyn crate::world::ScheduledTickAccess,
        pos: BlockPos,
        _direction: steel_registry::blocks::properties::Direction,
        _neighbor_pos: BlockPos,
        _neighbor_state: BlockStateId,
    ) -> BlockStateId {
        crate::behavior::block::schedule_water_tick_if_waterlogged(state, world, pos);
        state
    }

    fn step_on(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        entity: &dyn crate::entity::Entity,
    ) {
        // Vanilla `stepOn` force-schedules a STEP vibration through the BE listener when inactive.
        // Vanilla `stepOn`: inactive, non-Warden, and the vibration user accepts `STEP`.
        if !Self::can_activate(state)
            || entity.entity_type() == &steel_registry::vanilla_entities::WARDEN
        {
            self.default_step_on(state, world, pos, entity);
            return;
        }
        use steel_utils::Downcast as _;
        let Some(block_entity) = world.get_block_entity(pos) else {
            self.default_step_on(state, world, pos, entity);
            return;
        };
        let Some(sensor) =
            block_entity.downcast_ref::<crate::block_entity::entities::SculkSensorBlockEntity>()
        else {
            self.default_step_on(state, world, pos, entity);
            return;
        };
        let context = crate::world::game_event::GameEventContext::new(Some(entity), Some(state));
        // Vanilla `stepOn` also requires `VibrationUser.canReceiveVibration` for the stepped block.
        if !sensor.can_receive_vibration(world, pos, &vanilla_game_events::STEP, &context) {
            self.default_step_on(state, world, pos, entity);
            return;
        }
        sensor.vibration_listener().force_schedule_vibration(
            world,
            &vanilla_game_events::STEP,
            &context,
            entity.position(),
        );
        self.default_step_on(state, world, pos, entity);
    }

    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        match Self::phase(state) {
            SculkSensorPhase::Active => Self::deactivate(self.block, world, pos, state),
            // Vanilla `tick` clears cooldown to inactive, keeping `POWER` (already 0), and plays
            // `SCULK_CLICKING_STOP` unless waterlogged.
            SculkSensorPhase::Cooldown => {
                world.set_block(
                    pos,
                    state.set_value(PHASE, SculkSensorPhase::Inactive),
                    UpdateFlags::UPDATE_ALL,
                );
                if !state.get_value(WATERLOGGED) {
                    world.play_sound(
                        &sound_events::BLOCK_SCULK_SENSOR_CLICKING_STOP,
                        SoundSource::Blocks,
                        pos,
                        1.0,
                        rand::random::<f32>() * 0.2 + 0.8,
                        None,
                    );
                }
            }
            SculkSensorPhase::Inactive => {}
        }
    }

    fn on_place(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        old_state: BlockStateId,
        _moved_by_piston: bool,
    ) {
        // Vanilla skips the client side, guards on a pending active tick so a re-placed sensor
        // keeps its pulse, and uses flags 18 (`UPDATE_CLIENTS | UPDATE_KNOWN_SHAPE`).
        // TODO(World): Steel has no server/client-side branch on `World`; block behaviors always
        // run server-side today. Revisit when client worlds exist.
        if state.get_block() != old_state.get_block()
            && state.get_value(POWER) > 0
            && !world.has_scheduled_block_tick(pos, self.block)
        {
            world.set_block(
                pos,
                state.set_value(POWER, 0_u8),
                UpdateFlags::UPDATE_CLIENTS | UpdateFlags::UPDATE_KNOWN_SHAPE,
            );
        }
    }

    fn affect_neighbors_after_removal(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _moved_by_piston: bool,
    ) {
        if Self::phase(state) == SculkSensorPhase::Active {
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

    fn get_direct_signal(
        &self,
        state: BlockStateId,
        world: &dyn LevelReader,
        pos: BlockPos,
        direction: steel_registry::blocks::properties::Direction,
        _context: SignalQueryContext,
    ) -> i32 {
        // Vanilla `getDirectSignal`: only upward.
        if direction == steel_registry::blocks::properties::Direction::Up {
            self.get_signal(state, world, pos, direction, SignalQueryContext::DEFAULT)
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
        _direction: steel_registry::blocks::properties::Direction,
    ) -> i32 {
        use steel_utils::Downcast as _;
        // Vanilla returns the BE's last vibration frequency while active.
        if Self::phase(state) != SculkSensorPhase::Active {
            return 0;
        }
        let Some(entity) = world.get_block_entity(pos) else {
            return 0;
        };
        entity
            .downcast_ref::<crate::block_entity::entities::SculkSensorBlockEntity>()
            .map_or(0, |sensor| sensor.last_vibration_frequency())
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::Created(Arc::new(
            crate::block_entity::entities::SculkSensorBlockEntity::new(level, pos, state),
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
            &vanilla_block_entity_types::SCULK_SENSOR,
        )
    }

    fn spawn_after_break(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        tool: &steel_registry::item_stack::ItemStack,
        drop_experience: bool,
    ) {
        if drop_experience {
            // Vanilla `ConstantInt.of(5)`.
            try_drop_experience(world, pos, tool, &IntProvider::Constant(5));
        }
    }
}
