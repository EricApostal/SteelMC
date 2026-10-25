//! Vanilla vibration semantics for sculk blocks.
//!
//! Ports `net.minecraft.world.level.gameevent.vibrations.VibrationSystem`,
//! `VibrationInfo` and `VibrationSelector`:
//!
//! * [`vibration_frequency_for_event`] is Vanilla's `VIBRATION_FREQUENCY_FOR_EVENT` map used for
//!   sculk sensor power and calibrated-sensor resonance filtering.
//! * [`VibrationSelector`] buffers candidates for one game tick and picks the closest vibration,
//!   breaking ties by higher frequency.
//! * [`VibrationSystemListener`] is a [`GameEventListener`] that schedules the chosen vibration and
//!   counts the travel time down one tick per block, then reports arrival to a
//!   [`VibrationCallback`] owned by the concrete block entity (sculk sensor, calibrated sculk
//!   sensor, sculk shrieker, sculk catalyst).
use std::sync::Arc;

use glam::DVec3;
use steel_registry::game_events::GameEventRef;
use steel_registry::particle_type::{ParticleData, VibrationParticleOption};
use steel_registry::position_source::{BlockPositionSource, PositionSource};
use steel_registry::vanilla_game_events;
use steel_registry::vanilla_particle_types;
use steel_registry::vanilla_position_source_types;
use steel_utils::BlockPos;
use steel_utils::locks::SyncMutex;

use super::{GameEventContext, GameEventDeliveryMode, GameEventListener};
use crate::entity::Entity;
use crate::entity::projectile::Projectile;
use crate::world::World;
use crate::world::vibration::{are_adjacent_chunks_ticking, is_occluded, notify_avoid_vibration};

/// Builds Vanilla's `new VibrationParticleOption(positionSource, arrivalInTicks)` payload.
fn vibration_particle(destination: PositionSource, arrival_in_ticks: i32) -> ParticleData {
    ParticleData::new(
        &vanilla_particle_types::VIBRATION,
        VibrationParticleOption::new(destination, arrival_in_ticks),
    )
}

/// Vanilla `BlockPositionSource` for a block listener's own position.
#[must_use]
pub fn block_position_source(pos: BlockPos) -> PositionSource {
    PositionSource::new(
        &vanilla_position_source_types::BLOCK,
        BlockPositionSource::new(pos),
    )
}

/// Vanilla `VibrationSystem.NO_VIBRATION_FREQUENCY`.
pub const NO_VIBRATION_FREQUENCY: i32 = 0;

/// Vanilla `SculkSensorBlockEntity.VibrationUser.LISTENER_RANGE`.
pub const SENSOR_LISTENER_RADIUS: i32 = 8;

/// Vanilla `CalibratedSculkSensorBlockEntity.VibrationUser.getListenerRadius`.
pub const CALIBRATED_SENSOR_LISTENER_RADIUS: i32 = 16;

/// Vanilla `SculkShriekerBlockEntity.VibrationUser.LISTENER_RADIUS`.
pub const SHRIEKER_LISTENER_RADIUS: i32 = 8;

/// Vanilla `SculkCatalystBlockEntity.CatalystListener.getListenerRadius`.
pub const CATALYST_LISTENER_RADIUS: i32 = 8;

/// Vanilla `VibrationSystem.getGameEventFrequency(ResourceKey)`.
///
/// Events absent from the Vanilla map (for example `item_interact_start` and `jukebox_play`) map to
/// [`NO_VIBRATION_FREQUENCY`], which is exactly how Vanilla reproduces the `#vibrations` tag.
#[must_use]
pub fn vibration_frequency_for_event(event: GameEventRef) -> i32 {
    let key = &event.key;
    if key == &vanilla_game_events::STEP.key
        || key == &vanilla_game_events::SWIM.key
        || key == &vanilla_game_events::FLAP.key
    {
        1
    } else if key == &vanilla_game_events::PROJECTILE_LAND.key
        || key == &vanilla_game_events::HIT_GROUND.key
        || key == &vanilla_game_events::SPLASH.key
        || key == &vanilla_game_events::BOUNCE.key
    {
        2
    } else if key == &vanilla_game_events::ITEM_INTERACT_FINISH.key
        || key == &vanilla_game_events::PROJECTILE_SHOOT.key
        || key == &vanilla_game_events::INSTRUMENT_PLAY.key
    {
        3
    } else if key == &vanilla_game_events::ENTITY_ACTION.key
        || key == &vanilla_game_events::ELYTRA_GLIDE.key
        || key == &vanilla_game_events::UNEQUIP.key
    {
        4
    } else if key == &vanilla_game_events::ENTITY_DISMOUNT.key
        || key == &vanilla_game_events::EQUIP.key
    {
        5
    } else if key == &vanilla_game_events::ENTITY_INTERACT.key
        || key == &vanilla_game_events::SHEAR.key
        || key == &vanilla_game_events::ENTITY_MOUNT.key
    {
        6
    } else if key == &vanilla_game_events::ENTITY_DAMAGE.key {
        7
    } else if key == &vanilla_game_events::DRINK.key || key == &vanilla_game_events::EAT.key {
        8
    } else if key == &vanilla_game_events::CONTAINER_CLOSE.key
        || key == &vanilla_game_events::BLOCK_CLOSE.key
        || key == &vanilla_game_events::BLOCK_DEACTIVATE.key
        || key == &vanilla_game_events::BLOCK_DETACH.key
    {
        9
    } else if key == &vanilla_game_events::CONTAINER_OPEN.key
        || key == &vanilla_game_events::BLOCK_OPEN.key
        || key == &vanilla_game_events::BLOCK_ACTIVATE.key
        || key == &vanilla_game_events::BLOCK_ATTACH.key
        || key == &vanilla_game_events::PRIME_FUSE.key
        || key == &vanilla_game_events::NOTE_BLOCK_PLAY.key
    {
        10
    } else if key == &vanilla_game_events::BLOCK_CHANGE.key {
        11
    } else if key == &vanilla_game_events::BLOCK_DESTROY.key
        || key == &vanilla_game_events::FLUID_PICKUP.key
    {
        12
    } else if key == &vanilla_game_events::BLOCK_PLACE.key
        || key == &vanilla_game_events::FLUID_PLACE.key
    {
        13
    } else if key == &vanilla_game_events::ENTITY_PLACE.key
        || key == &vanilla_game_events::LIGHTNING_STRIKE.key
        || key == &vanilla_game_events::TELEPORT.key
    {
        14
    } else if key == &vanilla_game_events::ENTITY_DIE.key
        || key == &vanilla_game_events::EXPLODE.key
    {
        15
    } else {
        // Vanilla also seeds the map with `RESONATE_1..RESONATE_15`, each mapping to its own
        // frequency, which the resonance table below covers.
        resonance_frequency_for_event(event).unwrap_or(NO_VIBRATION_FREQUENCY)
    }
}

/// Returns the resonance frequency (1-15) of a `RESONATE_n` event, if it is one.
fn resonance_frequency_for_event(event: GameEventRef) -> Option<i32> {
    let key = &event.key;
    for frequency in 1..=15 {
        let resonance = resonance_event_by_frequency(frequency)?;
        if key == &resonance.key {
            return Some(frequency);
        }
    }
    None
}

/// Vanilla `VibrationSystem.getResonanceEventByFrequency`.
#[must_use]
pub fn resonance_event_by_frequency(vibration_frequency: i32) -> Option<GameEventRef> {
    Some(match vibration_frequency {
        1 => &vanilla_game_events::RESONATE_1,
        2 => &vanilla_game_events::RESONATE_2,
        3 => &vanilla_game_events::RESONATE_3,
        4 => &vanilla_game_events::RESONATE_4,
        5 => &vanilla_game_events::RESONATE_5,
        6 => &vanilla_game_events::RESONATE_6,
        7 => &vanilla_game_events::RESONATE_7,
        8 => &vanilla_game_events::RESONATE_8,
        9 => &vanilla_game_events::RESONATE_9,
        10 => &vanilla_game_events::RESONATE_10,
        11 => &vanilla_game_events::RESONATE_11,
        12 => &vanilla_game_events::RESONATE_12,
        13 => &vanilla_game_events::RESONATE_13,
        14 => &vanilla_game_events::RESONATE_14,
        15 => &vanilla_game_events::RESONATE_15,
        _ => return None,
    })
}

/// Vanilla `VibrationSystem.getRedstoneStrengthForDistance`.
#[must_use]
pub fn redstone_strength_for_distance(distance: f32, listener_radius: i32) -> i32 {
    let power_scale = 15.0 / f64::from(listener_radius);
    (15 - (power_scale * f64::from(distance)).floor() as i32).max(1)
}

/// Returns whether `event` is in Vanilla's `#ignore_vibrations_sneaking` game-event tag.
#[must_use]
pub fn ignores_vibrations_when_sneaking(event: GameEventRef) -> bool {
    let key = &event.key;
    key == &vanilla_game_events::HIT_GROUND.key
        || key == &vanilla_game_events::PROJECTILE_SHOOT.key
        || key == &vanilla_game_events::STEP.key
        || key == &vanilla_game_events::SWIM.key
        || key == &vanilla_game_events::ITEM_INTERACT_START.key
        || key == &vanilla_game_events::ITEM_INTERACT_FINISH.key
}

/// Vanilla `VibrationInfo`: one candidate vibration traveling toward a listener.
#[derive(Clone, Debug)]
pub struct VibrationInfo {
    /// The game event that produced the vibration.
    pub event: GameEventRef,
    /// Exact distance in blocks between source and listener.
    pub distance: f32,
    /// Exact source position.
    pub pos: DVec3,
    /// UUID of the source entity, when the event had one.
    pub uuid: Option<uuid::Uuid>,
    /// UUID of a projectile's owner, when the source entity was a projectile.
    pub projectile_owner_uuid: Option<uuid::Uuid>,
}

impl VibrationInfo {
    /// Creates vibration info without entity-derived UUIDs.
    #[must_use]
    pub const fn new(event: GameEventRef, distance: f32, pos: DVec3) -> Self {
        Self {
            event,
            distance,
            pos,
            uuid: None,
            projectile_owner_uuid: None,
        }
    }

    /// Vanilla `VibrationInfo(entity)`: the source UUID plus the owner UUID when the source is a
    /// projectile.
    #[must_use]
    pub fn new_for_source(
        event: GameEventRef,
        distance: f32,
        pos: DVec3,
        source: Option<&dyn Entity>,
    ) -> Self {
        let mut info = Self::new(event, distance, pos);
        if let Some(source) = source {
            info.uuid = Some(source.uuid());
            info.projectile_owner_uuid = projectile_owner_uuid(source);
        }
        info
    }

    /// Vanilla `VibrationSystem.User.calculateTravelTimeInTicks`.
    #[must_use]
    pub const fn travel_time_in_ticks(&self) -> i32 {
        self.distance.floor() as i32
    }
}

/// Vanilla `VibrationSelector`: picks at most one candidate per game tick.
#[derive(Default)]
pub struct VibrationSelector {
    current_vibration: Option<SelectedVibration>,
}

struct SelectedVibration {
    event: VibrationInfo,
    tick: i64,
}

impl VibrationSelector {
    /// Returns the currently selected candidate, if any.
    #[must_use]
    pub fn current(&self) -> Option<&VibrationInfo> {
        self.current_vibration
            .as_ref()
            .map(|selected| &selected.event)
    }

    /// Vanilla `VibrationSelector.addCandidate`.
    pub fn add_candidate(&mut self, new_vibration: VibrationInfo, tick_time: i64) {
        if self.should_replace_vibration(&new_vibration, tick_time) {
            self.current_vibration = Some(SelectedVibration {
                event: new_vibration,
                tick: tick_time,
            });
        }
    }

    fn should_replace_vibration(&self, new_vibration: &VibrationInfo, tick_time: i64) -> bool {
        let Some(previous) = &self.current_vibration else {
            return true;
        };
        if tick_time != previous.tick {
            return false;
        }
        if new_vibration.distance < previous.event.distance {
            return true;
        }
        if new_vibration.distance > previous.event.distance {
            return false;
        }
        vibration_frequency_for_event(new_vibration.event)
            > vibration_frequency_for_event(previous.event.event)
    }

    /// Vanilla `VibrationSelector.chosenCandidate`.
    #[must_use]
    pub fn chosen_candidate(&self, time: i64) -> Option<VibrationInfo> {
        let selected = self.current_vibration.as_ref()?;
        (selected.tick < time).then(|| selected.event.clone())
    }

    /// Game tick the current candidate was buffered on, as persisted by
    /// `VibrationSelector.CODEC`'s `tick` field (Vanilla writes `-1` for an empty selector).
    #[must_use]
    pub fn selected_tick(&self) -> Option<i64> {
        self.current_vibration
            .as_ref()
            .map(|selected| selected.tick)
    }

    /// Vanilla `VibrationSelector(VibrationInfo, long)`: restores a persisted candidate.
    pub const fn restore(&mut self, vibration: VibrationInfo, tick: i64) {
        self.current_vibration = Some(SelectedVibration {
            event: vibration,
            tick,
        });
    }

    /// Vanilla `VibrationSelector.startOver`.
    pub const fn start_over(&mut self) {
        self.current_vibration = None;
    }
}

/// Vanilla `VibrationSystem.Data`.
#[derive(Default)]
pub struct VibrationListenerState {
    current_vibration: Option<VibrationInfo>,
    travel_time_in_ticks: i32,
    selector: VibrationSelector,
    reload_vibration_particle: bool,
}

impl VibrationListenerState {
    /// Returns the vibration currently traveling toward the listener.
    #[must_use]
    pub const fn current_vibration(&self) -> Option<&VibrationInfo> {
        self.current_vibration.as_ref()
    }

    /// Replaces the in-flight vibration.
    pub const fn set_current_vibration(&mut self, vibration: Option<VibrationInfo>) {
        self.current_vibration = vibration;
    }

    /// Remaining travel time in game ticks.
    #[must_use]
    pub const fn travel_time_in_ticks(&self) -> i32 {
        self.travel_time_in_ticks
    }

    /// Sets remaining travel time in game ticks.
    pub const fn set_travel_time_in_ticks(&mut self, ticks: i32) {
        self.travel_time_in_ticks = ticks;
    }

    /// Vanilla `VibrationSystem.Data.decrementTravelTime`.
    pub fn decrement_travel_time(&mut self) {
        self.travel_time_in_ticks = (self.travel_time_in_ticks - 1).max(0);
    }

    /// Vanilla `VibrationSystem.Data.shouldReloadVibrationParticle`.
    #[must_use]
    pub const fn should_reload_vibration_particle(&self) -> bool {
        self.reload_vibration_particle
    }

    /// Vanilla `VibrationSystem.Data.setReloadVibrationParticle`.
    pub const fn set_reload_vibration_particle(&mut self, reload: bool) {
        self.reload_vibration_particle = reload;
    }

    /// Access to the candidate selector.
    pub const fn selector_mut(&mut self) -> &mut VibrationSelector {
        &mut self.selector
    }

    /// Read access to the candidate selector for persistence.
    #[must_use]
    pub const fn selector(&self) -> &VibrationSelector {
        &self.selector
    }
}

/// Per-listener hooks supplied by the owning block entity.
///
/// Vanilla models this as `VibrationSystem.User`; blocks that differ only in radius, filtering or
/// arrival handling implement this trait instead of duplicating the listener machinery.
pub trait VibrationCallback: Send + Sync {
    /// Vanilla `VibrationSystem.User.getListenerRadius`.
    fn listener_radius(&self) -> i32 {
        SENSOR_LISTENER_RADIUS
    }

    /// Vanilla `VibrationSystem.User.getPositionSource`.
    ///
    /// Every block listener in Vanilla returns a `BlockPositionSource` for its own position.
    fn position_source(&self) -> PositionSource;

    /// Vanilla `VibrationSystem.User.getListenableEvents`.
    ///
    /// The default accepts every event with a non-zero vibration frequency, which matches
    /// Vanilla's `#vibrations` tag (the tag contains exactly the frequency-mapped events plus the
    /// resonance events).
    fn is_listenable_event(&self, event: GameEventRef) -> bool {
        vibration_frequency_for_event(event) != NO_VIBRATION_FREQUENCY
    }

    /// Vanilla `VibrationSystem.User.canTriggerAvoidVibration`.
    ///
    /// Only the sculk sensors return `true`; it gates the `avoid_vibration` criterion raise.
    fn can_trigger_avoid_vibration(&self) -> bool {
        false
    }

    /// Vanilla `VibrationSystem.User.requiresAdjacentChunksToBeTicking`.
    ///
    /// Both sculk sensor variants and the shrieker return `true`, which makes the ticker retain
    /// an arrived vibration until the listener's 3x3 chunk neighbourhood is ticking.
    fn requires_adjacent_chunks_to_be_ticking(&self) -> bool {
        false
    }

    /// Vanilla `VibrationSystem.User.isValidVibration`.
    fn is_valid_vibration(
        &self,
        world: &Arc<World>,
        event: GameEventRef,
        context: &GameEventContext<'_>,
    ) -> bool {
        use crate::world::vibration::damping;
        if !self.is_listenable_event(event) {
            return false;
        }
        if let Some(source) = context.source_entity() {
            if source.is_spectator() {
                return false;
            }
            if source.is_stepping_carefully() && ignores_vibrations_when_sneaking(event) {
                if self.can_trigger_avoid_vibration()
                    && let Some(player) = source.as_player()
                {
                    notify_avoid_vibration(player, world);
                }
                return false;
            }
            if damping::entity_dampens_vibrations(world, source) {
                return false;
            }
        }
        !context
            .affected_state()
            .is_some_and(damping::state_dampens_vibrations)
    }

    /// Vanilla `VibrationSystem.User.canReceiveVibration`.
    fn can_receive_vibration(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        event: GameEventRef,
        context: &GameEventContext<'_>,
    ) -> bool;

    /// Vanilla `VibrationSystem.User.calculateTravelTimeInTicks`.
    fn calculate_travel_time_in_ticks(&self, distance: f32) -> i32 {
        distance.floor() as i32
    }

    /// Vanilla `VibrationSystem.User.onDataChanged`.
    fn on_data_changed(&self, world: &Arc<World>);

    /// Vanilla `VibrationSystem.User.onReceiveVibration` (entity/projectile arguments are folded
    /// into `event` and `distance` because Steel's event context already resolved the source).
    fn on_signal_received(&self, world: &Arc<World>, event: GameEventRef, distance: f32);

    /// Vanilla `VibrationSystem.User.onReceiveVibration` with the persisted source UUIDs, which
    /// listeners that need the causing player (sculk shrieker) resolve through the world.
    ///
    /// Vanilla forwards the source entity and, separately, its projectile owner; the shrieker
    /// prefers the projectile owner. `projectile_owner_uuid` is `None` for non-projectiles.
    fn on_signal_received_from(
        &self,
        world: &Arc<World>,
        event: GameEventRef,
        distance: f32,
        source_uuid: Option<uuid::Uuid>,
        projectile_owner_uuid: Option<uuid::Uuid>,
    ) {
        let _ = (source_uuid, projectile_owner_uuid);
        self.on_signal_received(world, event, distance);
    }
}

/// A [`GameEventListener`] driving one [`VibrationCallback`].
pub struct VibrationSystemListener {
    listener_pos: BlockPos,
    listener_radius: i32,
    callback: Arc<dyn VibrationCallback>,
    state: SyncMutex<VibrationListenerState>,
}

impl VibrationSystemListener {
    /// Creates a listener at `listener_pos` with `callback`'s radius.
    #[must_use]
    pub fn new(listener_pos: BlockPos, callback: Arc<dyn VibrationCallback>) -> Self {
        let listener_radius = callback.listener_radius();
        Self {
            listener_pos,
            listener_radius,
            callback,
            state: SyncMutex::new(VibrationListenerState::default()),
        }
    }

    /// Shared listener state for persistence and arrival processing.
    #[must_use]
    pub const fn state(&self) -> &SyncMutex<VibrationListenerState> {
        &self.state
    }

    /// Vanilla `VibrationSystem.Ticker.tick`.
    pub fn tick(&self, world: &Arc<World>) {
        let mut state = self.state.lock();
        if state.current_vibration.is_none() {
            let game_time = world.game_time();
            if let Some(candidate) = state.selector.chosen_candidate(game_time) {
                let travel_time = self
                    .callback
                    .calculate_travel_time_in_ticks(candidate.distance);
                let origin = candidate.pos;
                state.set_travel_time_in_ticks(travel_time);
                state.set_current_vibration(Some(candidate));
                state.selector_mut().start_over();
                // Vanilla `trySelectAndScheduleVibration`.
                world.send_particles(
                    vibration_particle(self.callback.position_source(), travel_time),
                    origin,
                    1,
                    DVec3::ZERO,
                    0.0,
                );
                self.callback.on_data_changed(world);
            }
        }

        let Some(vibration) = state.current_vibration().cloned() else {
            return;
        };

        // Vanilla `tryReloadVibrationParticle`: re-emit the in-flight particle at its current
        // interpolated position after a reload, and only clear the flag once a player saw it.
        if state.should_reload_vibration_particle() {
            let travel_time = state.travel_time_in_ticks();
            let initial_travel_time = self
                .callback
                .calculate_travel_time_in_ticks(vibration.distance);
            let destination = position_of(self.listener_pos);
            let origin = vibration.pos;
            let alpha = if initial_travel_time > 0 {
                1.0 - f64::from(travel_time) / f64::from(initial_travel_time)
            } else {
                1.0
            };
            let particle_pos = origin + (destination - origin) * alpha;
            let sent = world.send_particles(
                vibration_particle(self.callback.position_source(), travel_time),
                particle_pos,
                1,
                DVec3::ZERO,
                0.0,
            );
            if sent > 0 {
                state.set_reload_vibration_particle(false);
            }
        }

        let changed = state.travel_time_in_ticks() > 0;
        state.decrement_travel_time();
        if state.travel_time_in_ticks() > 0 {
            drop(state);
            if changed {
                self.callback.on_data_changed(world);
            }
            return;
        }

        let origin = BlockPos::from(vibration.pos);
        let distance = distance_between_in_blocks(origin, self.listener_pos);
        // Vanilla `receiveVibration`: with `requiresAdjacentChunksToBeTicking` the arrived
        // vibration is retained (not delivered and not cleared) until the listener's 3x3 chunk
        // neighbourhood ticks, so the next tick retries with zero travel time left.
        if self.callback.requires_adjacent_chunks_to_be_ticking()
            && !are_adjacent_chunks_ticking(world, self.listener_pos)
        {
            drop(state);
            return;
        }
        state.set_current_vibration(None);
        drop(state);
        self.callback.on_signal_received_from(
            world,
            vibration.event,
            distance,
            vibration.uuid,
            vibration.projectile_owner_uuid,
        );
        self.callback.on_data_changed(world);
    }

    /// Vanilla `VibrationSystem.Listener.forceScheduleVibration`.
    pub fn force_schedule_vibration(
        &self,
        world: &Arc<World>,
        event: GameEventRef,
        context: &GameEventContext<'_>,
        origin: DVec3,
    ) {
        let destination = position_of(self.listener_pos);
        let vibration = VibrationInfo::new_for_source(
            event,
            origin.distance(destination) as f32,
            origin,
            context.source_entity(),
        );
        self.state
            .lock()
            .selector_mut()
            .add_candidate(vibration, world.game_time());
    }
}

impl GameEventListener for VibrationSystemListener {
    fn listener_pos(&self) -> Option<DVec3> {
        Some(position_of(self.listener_pos))
    }

    fn listener_radius(&self) -> i32 {
        self.listener_radius
    }

    fn delivery_mode(&self) -> GameEventDeliveryMode {
        GameEventDeliveryMode::ByDistance
    }

    fn handle_game_event(
        &self,
        world: &Arc<World>,
        event: GameEventRef,
        context: &GameEventContext<'_>,
        source_pos: DVec3,
    ) -> bool {
        {
            let state = self.state.lock();
            if state.current_vibration().is_some() {
                return false;
            }
        }

        if !self.callback.is_valid_vibration(world, event, context) {
            return false;
        }

        let source_block_pos = BlockPos::from(source_pos);
        if !self
            .callback
            .can_receive_vibration(world, source_block_pos, event, context)
        {
            return false;
        }

        let destination = position_of(self.listener_pos);
        // Vanilla `VibrationSystem.Listener.isOccluded`.
        if is_occluded(world, source_pos, destination) {
            return false;
        }

        let vibration = VibrationInfo::new_for_source(
            event,
            source_pos.distance(destination) as f32,
            source_pos,
            context.source_entity(),
        );
        self.state
            .lock()
            .selector_mut()
            .add_candidate(vibration, world.game_time());
        true
    }
}

/// Vanilla `VibrationInfo.getProjectileOwner`: the UUID of the entity that fired `source`,
/// when `source` is a projectile with an owner.
#[must_use]
pub fn projectile_owner_uuid(source: &dyn Entity) -> Option<uuid::Uuid> {
    source
        .as_projectile()
        .and_then(Projectile::get_owner)
        .map(|owner| owner.uuid())
}

/// Vanilla `VibrationSystem.Listener.distanceBetweenInBlocks`.
#[must_use]
pub fn distance_between_in_blocks(origin: BlockPos, destination: BlockPos) -> f32 {
    let dx = f64::from(origin.x() - destination.x());
    let dy = f64::from(origin.y() - destination.y());
    let dz = f64::from(origin.z() - destination.z());
    (dx * dx + dy * dy + dz * dz).sqrt() as f32
}

/// Vanilla `PositionSource.getPosition` for a block position source.
#[must_use]
pub fn position_of(pos: BlockPos) -> DVec3 {
    DVec3::new(
        f64::from(pos.x()) + 0.5,
        f64::from(pos.y()) + 0.5,
        f64::from(pos.z()) + 0.5,
    )
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, vanilla_game_events};

    use super::*;

    #[test]
    fn redstone_strength_matches_the_vanilla_distance_curve() {
        init_vanilla_registry();
        assert_eq!(redstone_strength_for_distance(0.0, 8), 15);
        assert_eq!(redstone_strength_for_distance(2.0, 8), 12);
        assert_eq!(redstone_strength_for_distance(8.0, 8), 1);
        assert_eq!(redstone_strength_for_distance(64.0, 8), 1);
        assert_eq!(redstone_strength_for_distance(1.0, 16), 15);
        assert_eq!(redstone_strength_for_distance(16.0, 16), 1);
    }

    #[test]
    fn frequency_table_matches_the_vibrations_tag() {
        init_vanilla_registry();
        assert_eq!(vibration_frequency_for_event(&vanilla_game_events::STEP), 1);
        assert_eq!(vibration_frequency_for_event(&vanilla_game_events::FLAP), 1);
        assert_eq!(
            vibration_frequency_for_event(&vanilla_game_events::BLOCK_CHANGE),
            11
        );
        assert_eq!(
            vibration_frequency_for_event(&vanilla_game_events::NOTE_BLOCK_PLAY),
            10
        );
        assert_eq!(
            vibration_frequency_for_event(&vanilla_game_events::ENTITY_DIE),
            15
        );
        assert_eq!(
            vibration_frequency_for_event(&vanilla_game_events::RESONATE_7),
            7
        );
        assert_eq!(
            vibration_frequency_for_event(&vanilla_game_events::SHRIEK),
            NO_VIBRATION_FREQUENCY
        );
    }

    #[test]
    fn resonance_events_round_trip_by_frequency() {
        init_vanilla_registry();
        for frequency in 1..=15 {
            let event = resonance_event_by_frequency(frequency).expect("resonance event");
            assert_eq!(vibration_frequency_for_event(event), frequency);
        }
        assert!(resonance_event_by_frequency(0).is_none());
        assert!(resonance_event_by_frequency(16).is_none());
    }

    #[test]
    fn selector_prefers_the_closest_then_the_highest_frequency() {
        init_vanilla_registry();
        let mut selector = VibrationSelector::default();
        selector.add_candidate(
            VibrationInfo::new(&vanilla_game_events::BLOCK_CHANGE, 6.0, DVec3::ZERO),
            10,
        );
        selector.add_candidate(
            VibrationInfo::new(&vanilla_game_events::BLOCK_DESTROY, 2.0, DVec3::ZERO),
            10,
        );
        assert_eq!(selector.current().map(|info| info.distance), Some(2.0));

        // A candidate buffered on an earlier tick never replaces the current one.
        selector.add_candidate(
            VibrationInfo::new(&vanilla_game_events::STEP, 1.0, DVec3::ZERO),
            9,
        );
        assert_eq!(selector.current().map(|info| info.distance), Some(2.0));

        // Equal distance keeps the higher frequency.
        selector.add_candidate(
            VibrationInfo::new(&vanilla_game_events::ENTITY_DIE, 2.0, DVec3::ZERO),
            10,
        );
        assert_eq!(
            selector
                .current()
                .map(|info| vibration_frequency_for_event(info.event)),
            Some(15)
        );

        assert!(selector.chosen_candidate(10).is_none());
        assert!(selector.chosen_candidate(11).is_some());
        assert_eq!(selector.selected_tick(), Some(10));
        selector.start_over();
        assert!(selector.current().is_none());
        assert_eq!(selector.selected_tick(), None);
    }

    #[test]
    fn sneaking_ignore_tag_matches_vanilla() {
        init_vanilla_registry();
        assert!(ignores_vibrations_when_sneaking(
            &vanilla_game_events::ITEM_INTERACT_START
        ));
        assert!(ignores_vibrations_when_sneaking(&vanilla_game_events::STEP));
        assert!(!ignores_vibrations_when_sneaking(
            &vanilla_game_events::ENTITY_DIE
        ));
    }
}
