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
    BlockStateProperties, BoolProperty, Direction, EnumProperty, IntProperty, SculkSensorPhase,
};
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_entities::WARDEN;
use steel_registry::vanilla_game_events::SCULK_SENSOR_TENDRILS_CLICKING;
use steel_registry::{sound_events, vanilla_block_entity_types, vanilla_game_events};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, Downcast as _, value_providers::IntProvider};

use crate::behavior::block::schedule_water_tick_if_waterlogged;
use crate::behavior::blocks::NoteBlock;
use crate::behavior::{BlockBehavior, BlockEntityCreation, BlockPlaceContext, try_drop_experience};
use crate::block_entity::BlockEntityTicker;
use crate::block_entity::entities::{CalibratedSculkSensorBlockEntity, SculkSensorBlockEntity};
use crate::entity::Entity;
use crate::entity::ai::path::PathComputationType;
use crate::world::ScheduledTickAccess;
use crate::world::game_event::GameEventContext;
use crate::world::game_event::vibration::resonance_event_by_frequency;
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

const PHASE: &EnumProperty<SculkSensorPhase> = &BlockStateProperties::SCULK_SENSOR_PHASE;
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
    /// active tick, updates neighbors and emits the clicking game event.
    ///
    /// Sound playback beyond the game event is server-side audible context handled by listeners;
    /// particle emission (`DustColorTransitionOptions.SCULK_TO_REDSTONE`) is client-local
    /// `animateTick` and has no server equivalent.
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors vanilla activate signature (block/world/pos/state/power/frequency/entity/ticks)"
    )]
    pub fn activate(
        block: BlockRef,
        world: &Arc<World>,
        pos: BlockPos,
        state: BlockStateId,
        power: i32,
        vibration_frequency: i32,
        source_entity: Option<&dyn Entity>,
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
            &SCULK_SENSOR_TENDRILS_CLICKING,
            pos,
            &GameEventContext::new(source_entity, None),
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
        source_entity: Option<&dyn Entity>,
        world: &Arc<World>,
        pos: BlockPos,
        vibration_frequency: i32,
    ) {
        // Vanilla `RESONANCE_PITCH_BEND`, built from `NoteBlock.getPitchFromNote` over the
        // resonance tone map `{0,0,2,4,6,7,9,10,12,14,15,18,19,21,22,24}`.
        const RESONANCE_TONE_MAP: [u8; 16] =
            [0, 0, 2, 4, 6, 7, 9, 10, 12, 14, 15, 18, 19, 21, 22, 24];
        let Some(resonance_event) = resonance_event_by_frequency(vibration_frequency) else {
            return;
        };
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
                &GameEventContext::new(source_entity, Some(relative_state)),
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

    /// Vanilla `SculkSensorBlock.tick`, shared with `CalibratedSculkSensorBlock` (which inherits
    /// it unchanged and only overrides `getActiveTicks`).
    pub fn tick_phase(block: BlockRef, world: &Arc<World>, pos: BlockPos, state: BlockStateId) {
        match Self::phase(state) {
            SculkSensorPhase::Active => Self::deactivate(block, world, pos, state),
            // Vanilla clears cooldown to inactive, keeping `POWER` (already 0), and plays
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

    /// Vanilla `SculkSensorBlock.onPlace`, shared with `CalibratedSculkSensorBlock`.
    ///
    /// Vanilla skips the client side, guards on a pending active tick so a re-placed sensor keeps
    /// its pulse, and uses flags 18 (`UPDATE_CLIENTS | UPDATE_KNOWN_SHAPE`).
    pub fn reset_power_on_place(
        _block: BlockRef,
        world: &Arc<World>,
        pos: BlockPos,
        state: BlockStateId,
        old_state: BlockStateId,
    ) {
        if state.get_block() != old_state.get_block()
            && state.get_value(POWER) > 0
            && !world.has_scheduled_block_tick(pos, state.get_block())
        {
            world.set_block(
                pos,
                state.set_value(POWER, 0_u8),
                UpdateFlags::UPDATE_CLIENTS | UpdateFlags::UPDATE_KNOWN_SHAPE,
            );
        }
    }

    /// Vanilla `SculkSensorBlock.stepOn` vibration half, shared with
    /// `CalibratedSculkSensorBlock` (which inherits it unchanged).
    ///
    /// Vanilla gates on `canActivate`, the entity not being a warden, the block entity being a
    /// `SculkSensorBlockEntity` (either variant) and `canReceiveVibration` with a state-only
    /// context, then force-schedules the STEP vibration from the entity's exact position.
    pub fn schedule_step_vibration(
        world: &Arc<World>,
        pos: BlockPos,
        state: BlockStateId,
        entity: &dyn Entity,
    ) {
        if !Self::can_activate(state) || entity.entity_type() == &WARDEN {
            return;
        }
        let Some(block_entity) = world.get_block_entity(pos) else {
            return;
        };
        let probe = GameEventContext::new(None, Some(state));
        let listener = if let Some(sensor) = block_entity.downcast_ref::<SculkSensorBlockEntity>() {
            if !sensor.can_receive_vibration(world, pos, &vanilla_game_events::STEP, &probe) {
                return;
            }
            Arc::clone(sensor.vibration_listener())
        } else if let Some(sensor) = block_entity.downcast_ref::<CalibratedSculkSensorBlockEntity>()
        {
            if !sensor.can_receive_vibration(world, pos, &vanilla_game_events::STEP, &probe) {
                return;
            }
            Arc::clone(sensor.vibration_listener())
        } else {
            return;
        };
        listener.force_schedule_vibration(
            world,
            &vanilla_game_events::STEP,
            &GameEventContext::new(Some(entity), Some(state)),
            entity.position(),
        );
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
        world: &dyn ScheduledTickAccess,
        pos: BlockPos,
        _direction: Direction,
        _neighbor_pos: BlockPos,
        _neighbor_state: BlockStateId,
    ) -> BlockStateId {
        schedule_water_tick_if_waterlogged(state, world, pos);
        state
    }

    fn step_on(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos, entity: &dyn Entity) {
        // Vanilla `stepOn` runs the vibration half first, then `super.stepOn`.
        Self::schedule_step_vibration(world, pos, state, entity);
        self.default_step_on(state, world, pos, entity);
    }

    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        Self::tick_phase(self.block, world, pos, state);
    }

    fn on_place(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        old_state: BlockStateId,
        _moved_by_piston: bool,
    ) {
        Self::reset_power_on_place(self.block, world, pos, state, old_state);
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

    fn is_pathfindable(
        &self,
        _state: BlockStateId,
        _computation_type: PathComputationType,
    ) -> bool {
        // Vanilla `SculkSensorBlock.isPathfindable` always returns `false`.
        false
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
        direction: Direction,
        _context: SignalQueryContext,
    ) -> i32 {
        // Vanilla `getDirectSignal`: only upward.
        if direction == Direction::Up {
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
        _direction: Direction,
    ) -> i32 {
        // Vanilla returns the BE's last vibration frequency while active.
        if Self::phase(state) != SculkSensorPhase::Active {
            return 0;
        }
        let Some(entity) = world.get_block_entity(pos) else {
            return 0;
        };
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
        BlockEntityCreation::Created(Arc::new(SculkSensorBlockEntity::new(level, pos, state)))
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
    use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_game_events};
    use steel_utils::ChunkPos;
    use steel_utils::types::UpdateFlags;

    use super::*;
    use crate::behavior::init_behaviors;
    use crate::test_support::{
        advance_test_game_time_to, fresh_test_world, insert_ready_full_chunk,
    };
    use crate::world::game_event::GameEventContext;

    /// Places a sculk sensor with its block entity in a ticking test chunk whose 3x3 chunk
    /// neighbourhood is loaded, as `VibrationUser.requiresAdjacentChunksToBeTicking` demands
    /// before an arrived vibration is delivered.
    fn place_sensor(world: &Arc<World>, pos: BlockPos) {
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));
        let chunk = ChunkPos::from_block_pos(pos);
        for dx in -1..=1 {
            for dz in -1..=1 {
                if dx != 0 || dz != 0 {
                    insert_ready_full_chunk(world, ChunkPos::new(chunk.0.x + dx, chunk.0.y + dz));
                }
            }
        }
        assert!(world.set_block(
            pos,
            vanilla_blocks::SCULK_SENSOR.default_state(),
            UpdateFlags::UPDATE_ALL,
        ));
    }

    /// Ticks the sensor block entity `count` times.
    fn tick_sensor(world: &Arc<World>, pos: BlockPos, count: usize) {
        let entity = world
            .get_block_entity(pos)
            .expect("placed sculk sensor should have a block entity");
        for _ in 0..count {
            entity.tick(world);
        }
    }

    /// Vanilla `SculkSensorBlock` never emits redstone while inactive; this exercises the whole
    /// vibration path (event -> listener candidate -> travel time -> `activate`) end to end.
    #[test]
    fn absorbed_vibration_activates_and_emits_redstone_power() {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world("sculk_sensor_activation");
        let world = &world_fixture.world;
        let pos = BlockPos::new(8, 64, 8);
        place_sensor(world, pos);
        assert_eq!(world.get_block_state(pos).get_value(POWER), 0);

        // A step two blocks away, exactly what `Entity.walkDist` reports server-side.
        world.game_event(
            &vanilla_game_events::STEP,
            pos.offset(2, 0, 0),
            &GameEventContext::default(),
        );
        // `VibrationSelector.chosenCandidate` only releases a candidate on a later game tick than
        // the one it was buffered on.
        advance_test_game_time_to(world, world.game_time() + 1);

        // One tick to schedule the two blocks of travel time, then the arrival tick that
        // activates the sensor.
        tick_sensor(world, pos, 2);

        let activated = world.get_block_state(pos);
        assert_eq!(SculkSensorBlock::phase(activated), SculkSensorPhase::Active);
        // Vanilla `getRedstoneStrengthForDistance(2.0, 8) = max(1, 15 - floor(15/8*2)) = 12`.
        assert_eq!(activated.get_value(POWER), 12);
        assert!(world.has_scheduled_block_tick(pos, &vanilla_blocks::SCULK_SENSOR));

        let entity = world.get_block_entity(pos).expect("sensor block entity");
        let sensor = entity
            .downcast_ref::<SculkSensorBlockEntity>()
            .expect("sculk sensor block entity");
        assert_eq!(sensor.last_power(), 12);
        // STEP maps to vibration frequency 1, which the comparator output mirrors.
        assert_eq!(sensor.last_vibration_frequency(), 1);
        let behavior = SculkSensorBlock::new(&vanilla_blocks::SCULK_SENSOR);
        assert_eq!(
            behavior.get_analog_output_signal(activated, world.as_ref(), pos, Direction::Down,),
            1,
        );
        assert_eq!(
            behavior.get_own_signal(activated, world.as_ref(), pos, SignalQueryContext::DEFAULT),
            12,
        );
    }

    #[test]
    fn active_tick_cools_down_and_cooldown_tick_returns_to_inactive() {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world("sculk_sensor_cooldown");
        let world = &world_fixture.world;
        let pos = BlockPos::new(8, 64, 8);
        place_sensor(world, pos);
        let behavior = SculkSensorBlock::new(&vanilla_blocks::SCULK_SENSOR);

        let active = world
            .get_block_state(pos)
            .set_value(PHASE, SculkSensorPhase::Active)
            .set_value(POWER, 7_u8);
        world.set_block(pos, active, UpdateFlags::UPDATE_ALL);

        behavior.tick(active, world, pos);
        let cooldown = world.get_block_state(pos);
        assert_eq!(
            SculkSensorBlock::phase(cooldown),
            SculkSensorPhase::Cooldown
        );
        assert_eq!(cooldown.get_value(POWER), 0);
        assert!(world.has_scheduled_block_tick(pos, &vanilla_blocks::SCULK_SENSOR));

        behavior.tick(cooldown, world, pos);
        assert_eq!(
            SculkSensorBlock::phase(world.get_block_state(pos)),
            SculkSensorPhase::Inactive
        );
    }

    #[test]
    fn direct_signal_is_upward_only_while_the_own_signal_is_full_power() {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world("sculk_sensor_direct_signal");
        let world = &world_fixture.world;
        let pos = BlockPos::new(8, 64, 8);
        place_sensor(world, pos);
        let behavior = SculkSensorBlock::new(&vanilla_blocks::SCULK_SENSOR);
        let active = world
            .get_block_state(pos)
            .set_value(PHASE, SculkSensorPhase::Active)
            .set_value(POWER, 9_u8);

        assert_eq!(
            behavior.get_direct_signal(
                active,
                world.as_ref(),
                pos,
                Direction::Up,
                SignalQueryContext::DEFAULT,
            ),
            9,
        );
        assert_eq!(
            behavior.get_direct_signal(
                active,
                world.as_ref(),
                pos,
                Direction::North,
                SignalQueryContext::DEFAULT,
            ),
            0,
        );
    }
}
