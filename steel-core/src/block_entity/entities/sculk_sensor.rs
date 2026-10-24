//! Vanilla sculk sensor block entities.
//!
//! Vanilla references: `SculkSensorBlockEntity` (radius 8; activation through
//! `VibrationUser.onReceiveVibration`) and `CalibratedSculkSensorBlockEntity` (radius 16; rejects
//! vibrations whose frequency differs from the back redstone signal). Each entity owns a
//! [`VibrationSystemListener`](crate::world::game_event::vibration::VibrationSystemListener) that
//! chunk storage registers through `BlockBehavior::get_game_event_listener`, and `tick` advances
//! the travel-time countdown exactly like `VibrationSystem.Ticker.tick`.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, Direction, EnumProperty};
use steel_registry::game_events::GameEventRef;
use steel_registry::position_source::PositionSource;
use steel_registry::{REGISTRY, RegistryExt, vanilla_block_entity_types, vanilla_game_events};
use steel_utils::UuidExt as _;
use steel_utils::{
    BlockPos, BlockStateId, DowncastType, DowncastTypeKey, Identifier, locks::SyncMutex,
};

use crate::behavior::blocks::SculkSensorBlock;
use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::world::game_event::vibration::{
    CALIBRATED_SENSOR_LISTENER_RADIUS, NO_VIBRATION_FREQUENCY, SENSOR_LISTENER_RADIUS,
    VibrationCallback, VibrationInfo, VibrationListenerState, VibrationSystemListener,
    block_position_source, redstone_strength_for_distance, vibration_frequency_for_event,
};
use crate::world::game_event::{GameEventContext, SharedGameEventListener};
use crate::world::{SignalGetter as _, World};

const FACING: &EnumProperty<Direction> = &BlockStateProperties::HORIZONTAL_FACING;

/// One in-flight vibration as persisted by Vanilla's `VibrationInfo` codec.
pub(crate) struct PersistedInfo {
    event_key: Identifier,
    distance: f32,
    x: f64,
    y: f64,
    z: f64,
    source: Option<uuid::Uuid>,
    projectile_owner: Option<uuid::Uuid>,
}

/// Vanilla `VibrationSystem.Data` codec fields.
pub(crate) struct PersistedVibration {
    current: Option<PersistedInfo>,
    travel_time_in_ticks: i32,
}

fn persist_vibration(state: &VibrationListenerState) -> PersistedVibration {
    PersistedVibration {
        current: state.current_vibration().map(|info| PersistedInfo {
            event_key: info.event.key.clone(),
            distance: info.distance,
            x: info.pos.x,
            y: info.pos.y,
            z: info.pos.z,
            source: info.uuid,
            projectile_owner: info.projectile_owner_uuid,
        }),
        travel_time_in_ticks: state.travel_time_in_ticks(),
    }
}

/// Restores a persisted `listener` compound onto `state`.
pub(crate) fn restore_vibration(
    state: &SyncMutex<VibrationListenerState>,
    persisted: PersistedVibration,
) {
    let mut guard = state.lock();
    let Some(info) = persisted.current else {
        return;
    };
    let Some(event) = REGISTRY.game_events.by_key(&info.event_key) else {
        return;
    };
    guard.set_current_vibration(Some(VibrationInfo {
        event,
        distance: info.distance,
        pos: DVec3::new(info.x, info.y, info.z),
        uuid: info.source,
        projectile_owner_uuid: info.projectile_owner,
    }));
    guard.set_travel_time_in_ticks(persisted.travel_time_in_ticks);
}

/// Vanilla `UUIDUtil.CODEC` (a four-int array).
fn insert_uuid(nbt: &mut NbtCompound, key: &str, uuid: uuid::Uuid) {
    nbt.insert(key, NbtTag::IntArray(uuid.to_int_array().to_vec()));
}

/// Writes the `listener` compound shared by both sensor variants and the shrieker.
pub(crate) fn save_listener_nbt(nbt: &mut NbtCompound, state: &VibrationListenerState) {
    let persisted = persist_vibration(state);
    let mut listener = NbtCompound::new();
    if let Some(current) = persisted.current {
        let mut event = NbtCompound::new();
        event.insert("game_event", current.event_key.to_string());
        event.insert("distance", current.distance);
        // Vanilla `Vec3.CODEC`: `[x, y, z]` doubles.
        event.insert(
            "pos",
            NbtTag::List(NbtList::Double(vec![current.x, current.y, current.z])),
        );
        if let Some(source) = current.source {
            insert_uuid(&mut event, "source", source);
        }
        if let Some(projectile_owner) = current.projectile_owner {
            insert_uuid(&mut event, "projectile_owner", projectile_owner);
        }
        listener.insert("event", event);
    }
    listener.insert("event_delay", persisted.travel_time_in_ticks);
    nbt.insert("listener", listener);
}

/// Reads the `listener` compound shared by both sensor variants and the shrieker.
pub(crate) fn load_listener_nbt(nbt: &NbtCompoundView<'_, '_>) -> PersistedVibration {
    let listener = nbt.compound("listener");
    let current = listener.and_then(|listener| {
        let event = listener.compound("event")?;
        let key = event.string("game_event")?;
        let pos = event
            .list("pos")
            .and_then(|pos| pos.doubles())
            .unwrap_or_default();
        Some(PersistedInfo {
            event_key: key
                .to_string_lossy()
                .parse::<Identifier>()
                .unwrap_or_else(|_| Identifier::new_static("minecraft", "step")),
            distance: event.float("distance").unwrap_or(0.0),
            x: pos.first().copied().unwrap_or(0.0),
            y: pos.get(1).copied().unwrap_or(0.0),
            z: pos.get(2).copied().unwrap_or(0.0),
            source: event
                .int_array("source")
                .and_then(|uuid| uuid::Uuid::from_int_array(&uuid)),
            projectile_owner: event
                .int_array("projectile_owner")
                .and_then(|uuid| uuid::Uuid::from_int_array(&uuid)),
        })
    });
    PersistedVibration {
        current,
        travel_time_in_ticks: listener
            .and_then(|listener| listener.int("event_delay"))
            .unwrap_or(0),
    }
}

/// Sensor activation shared by both variants.
struct SensorCallback {
    pos: BlockPos,
    radius: i32,
    power: SyncMutex<i32>,
    frequency: SyncMutex<i32>,
}

impl SensorCallback {
    fn on_signal_received_with_context(
        &self,
        world: &Arc<World>,
        event: GameEventRef,
        distance: f32,
        source_entity: Option<&dyn crate::entity::Entity>,
    ) {
        let state = world.get_block_state(self.pos);
        // Vanilla `VibrationUser.onReceiveVibration`: both the stored frequency and the computed
        // power are only updated while the sensor can still activate.
        if !SculkSensorBlock::can_activate(state) {
            return;
        }
        let frequency = vibration_frequency_for_event(event);
        let power = redstone_strength_for_distance(distance, self.radius);
        *self.power.lock() = power;
        *self.frequency.lock() = frequency;
        SculkSensorBlock::activate(
            state.get_block(),
            world,
            self.pos,
            state,
            power,
            frequency,
            source_entity,
            crate::behavior::blocks::SCULK_SENSOR_ACTIVE_TICKS,
        );
    }

    fn new(pos: BlockPos, radius: i32) -> Self {
        Self {
            pos,
            radius,
            power: SyncMutex::new(0),
            frequency: SyncMutex::new(0),
        }
    }

    fn set_changed(&self, world: &Arc<World>) {
        if let Some(entity) = world.get_block_entity(self.pos) {
            entity.set_changed();
        }
    }
}

impl VibrationCallback for SensorCallback {
    fn listener_radius(&self) -> i32 {
        self.radius
    }

    fn position_source(&self) -> PositionSource {
        block_position_source(self.pos)
    }

    fn can_receive_vibration(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        event: GameEventRef,
        _context: &GameEventContext<'_>,
    ) -> bool {
        // Vanilla `VibrationUser.canReceiveVibration`: a vibration originating at the sensor's own
        // block is always rejected for block place/destroy, and for every other event the sensor
        // still has to be able to activate.
        if pos == self.pos
            && (event.key == vanilla_game_events::BLOCK_DESTROY.key
                || event.key == vanilla_game_events::BLOCK_PLACE.key)
        {
            return false;
        }
        vibration_frequency_for_event(event) != NO_VIBRATION_FREQUENCY
            && SculkSensorBlock::can_activate(world.get_block_state(self.pos))
    }

    fn calculate_travel_time_in_ticks(&self, distance: f32) -> i32 {
        distance.floor() as i32
    }

    fn on_data_changed(&self, world: &Arc<World>) {
        self.set_changed(world);
    }

    fn on_signal_received(&self, world: &Arc<World>, event: GameEventRef, distance: f32) {
        self.on_signal_received_with_context(world, event, distance, None);
    }

    fn on_signal_received_from(
        &self,
        world: &Arc<World>,
        event: GameEventRef,
        distance: f32,
        source_uuid: Option<uuid::Uuid>,
    ) {
        let entity = source_uuid.and_then(|uuid| world.get_entity_by_uuid(&uuid));
        self.on_signal_received_with_context(world, event, distance, entity.as_deref());
    }
}

/// Vanilla `SculkSensorBlockEntity`.
pub struct SculkSensorBlockEntity {
    base: BlockEntityBase,
    callback: Arc<SensorCallback>,
    listener: Arc<VibrationSystemListener>,
}

// SAFETY: Steel-owned key uniquely identifies this concrete entity type.
unsafe impl DowncastType for SculkSensorBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/sculk_sensor");
}

impl SculkSensorBlockEntity {
    /// Creates a sensor block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        let callback = Arc::new(SensorCallback::new(pos, SENSOR_LISTENER_RADIUS));
        let listener = Arc::new(VibrationSystemListener::new(
            pos,
            Arc::clone(&callback) as Arc<dyn VibrationCallback>,
        ));
        Self {
            base: BlockEntityBase::new(
                &vanilla_block_entity_types::SCULK_SENSOR,
                level,
                pos,
                state,
            ),
            callback,
            listener,
        }
    }

    /// Vanilla `getLastVibrationFrequency`, also the comparator output while active.
    #[must_use]
    pub fn last_vibration_frequency(&self) -> i32 {
        *self.callback.frequency.lock()
    }

    /// Redstone power applied by the most recent activation.
    #[must_use]
    pub fn last_power(&self) -> i32 {
        *self.callback.power.lock()
    }

    /// The registered game-event listener.
    #[must_use]
    pub fn vibration_listener(&self) -> &Arc<VibrationSystemListener> {
        &self.listener
    }

    /// Vanilla `VibrationUser.canReceiveVibration` evaluated through the current world state so
    /// `SculkSensorBlock::step_on` can apply the same gate as scheduled events.
    #[must_use]
    pub fn can_receive_vibration(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        event: GameEventRef,
        context: &GameEventContext<'_>,
    ) -> bool {
        self.callback
            .can_receive_vibration(world, pos, event, context)
    }
}

impl BlockEntity for SculkSensorBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let nbt: NbtCompoundView<'_, '_> = nbt.into();
        if let Some(frequency) = nbt.int("last_vibration_frequency") {
            *self.callback.frequency.lock() = frequency;
        }
        restore_vibration(self.listener.state(), load_listener_nbt(&nbt));
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        nbt.insert("last_vibration_frequency", *self.callback.frequency.lock());
        save_listener_nbt(nbt, &self.listener.state().lock());
    }

    fn tick(&self, world: &Arc<World>) {
        self.listener.tick(world);
    }

    fn game_event_listener(&self) -> Option<SharedGameEventListener> {
        Some(Arc::clone(&self.listener) as SharedGameEventListener)
    }
}

/// Calibrated sensor callback: frequency filtering against the back signal.
struct CalibratedCallback {
    inner: SensorCallback,
}

impl CalibratedCallback {
    /// Vanilla `VibrationUser.getBackSignal`: redstone signal entering the block behind `FACING`.
    fn back_signal(&self, world: &Arc<World>) -> i32 {
        let state = world.get_block_state(self.inner.pos);
        let facing = state.get_value(FACING);
        let back = facing.opposite();
        world.get_signal(self.inner.pos.relative(back), back)
    }
}

impl VibrationCallback for CalibratedCallback {
    fn listener_radius(&self) -> i32 {
        CALIBRATED_SENSOR_LISTENER_RADIUS
    }

    fn position_source(&self) -> PositionSource {
        block_position_source(self.inner.pos)
    }

    fn can_receive_vibration(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        event: GameEventRef,
        context: &GameEventContext<'_>,
    ) -> bool {
        let comparison = self.back_signal(world);
        if comparison != 0 && vibration_frequency_for_event(event) != comparison {
            return false;
        }
        self.inner.can_receive_vibration(world, pos, event, context)
    }

    fn on_data_changed(&self, world: &Arc<World>) {
        self.inner.on_data_changed(world);
    }

    fn on_signal_received(&self, world: &Arc<World>, event: GameEventRef, distance: f32) {
        self.on_signal_received_from(world, event, distance, None);
    }

    fn on_signal_received_from(
        &self,
        world: &Arc<World>,
        event: GameEventRef,
        distance: f32,
        source_uuid: Option<uuid::Uuid>,
    ) {
        let entity = source_uuid.and_then(|uuid| world.get_entity_by_uuid(&uuid));
        let state = world.get_block_state(self.inner.pos);
        // Vanilla inherits `VibrationUser.onReceiveVibration`: the frequency and power are only
        // updated (and the sensor only activated) while it can still activate.
        if !SculkSensorBlock::can_activate(state) {
            return;
        }
        let frequency = vibration_frequency_for_event(event);
        let power = redstone_strength_for_distance(distance, self.inner.radius);
        *self.inner.power.lock() = power;
        *self.inner.frequency.lock() = frequency;
        crate::behavior::blocks::CalibratedSculkSensorBlock::activate(
            state.get_block(),
            world,
            self.inner.pos,
            state,
            power,
            frequency,
            entity.as_deref(),
        );
    }
}

/// Vanilla `CalibratedSculkSensorBlockEntity`.
pub struct CalibratedSculkSensorBlockEntity {
    base: BlockEntityBase,
    callback: Arc<CalibratedCallback>,
    listener: Arc<VibrationSystemListener>,
}

// SAFETY: Steel-owned key uniquely identifies this concrete entity type.
unsafe impl DowncastType for CalibratedSculkSensorBlockEntity {
    const TYPE_KEY: DowncastTypeKey =
        DowncastTypeKey::new("steel:block_entity/calibrated_sculk_sensor");
}

impl CalibratedSculkSensorBlockEntity {
    /// Creates a calibrated sensor block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        let callback = Arc::new(CalibratedCallback {
            inner: SensorCallback::new(pos, CALIBRATED_SENSOR_LISTENER_RADIUS),
        });
        let listener = Arc::new(VibrationSystemListener::new(
            pos,
            Arc::clone(&callback) as Arc<dyn VibrationCallback>,
        ));
        Self {
            base: BlockEntityBase::new(
                &vanilla_block_entity_types::CALIBRATED_SCULK_SENSOR,
                level,
                pos,
                state,
            ),
            callback,
            listener,
        }
    }

    /// Vanilla `getLastVibrationFrequency`.
    #[must_use]
    pub fn last_vibration_frequency(&self) -> i32 {
        *self.callback.inner.frequency.lock()
    }

    /// The registered game-event listener.
    #[must_use]
    pub fn vibration_listener(&self) -> &Arc<VibrationSystemListener> {
        &self.listener
    }
}

impl BlockEntity for CalibratedSculkSensorBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let nbt: NbtCompoundView<'_, '_> = nbt.into();
        if let Some(frequency) = nbt.int("last_vibration_frequency") {
            *self.callback.inner.frequency.lock() = frequency;
        }
        restore_vibration(self.listener.state(), load_listener_nbt(&nbt));
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        nbt.insert(
            "last_vibration_frequency",
            *self.callback.inner.frequency.lock(),
        );
        save_listener_nbt(nbt, &self.listener.state().lock());
    }

    fn tick(&self, world: &Arc<World>) {
        self.listener.tick(world);
    }

    fn game_event_listener(&self) -> Option<SharedGameEventListener> {
        Some(Arc::clone(&self.listener) as SharedGameEventListener)
    }
}
