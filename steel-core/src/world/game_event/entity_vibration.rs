//! Vanilla `VibrationSystem` for entities.
//!
//! Ports the entity half of `net.minecraft.world.level.gameevent.vibrations.VibrationSystem`:
//! `EntityPositionSource` (a listener position that follows the entity), `DynamicGameEventListener`
//! (chunk-section registration that follows the entity across sections and dimensions) and
//! `VibrationSystem.Ticker` (the per-tick selection/arrival state machine). The block half lives in
//! [`super::vibration`] and is reused unchanged: [`VibrationInfo`], [`VibrationListenerState`],
//! `VibrationSelector`, the frequency table and the sneaking filter all come from there.
//!
//! Deliberate difference from the block-side `VibrationCallback`: this trait keeps vanilla's
//! `sourceEntity`/`projectileOwner` pair on `onReceiveVibration` instead of folding both into the
//! event UUID, because `Warden.VibrationUser` treats a projectile owner differently from a direct
//! source.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as NbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtTag};
use steel_registry::game_events::GameEventRef;
use steel_registry::particle_type::{ParticleData, VibrationParticleOption};
use steel_registry::position_source::PositionSource;
use steel_registry::vanilla_particle_types;
use steel_registry::{REGISTRY, RegistryExt as _};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, Identifier, SectionPos};
use uuid::Uuid;

use super::vibration::{
    NO_VIBRATION_FREQUENCY, VibrationInfo, VibrationListenerState, distance_between_in_blocks,
    ignores_vibrations_when_sneaking, vibration_frequency_for_event,
};
use super::{GameEventContext, GameEventDeliveryMode, GameEventListener, SharedGameEventListener};
use crate::entity::SharedEntity;
use crate::world::World;

/// Builds Vanilla's `new VibrationParticleOption(positionSource, arrivalInTicks)` payload.
fn vibration_particle(destination: PositionSource, arrival_in_ticks: i32) -> ParticleData {
    ParticleData::new(
        &vanilla_particle_types::VIBRATION,
        VibrationParticleOption::new(destination, arrival_in_ticks),
    )
}

/// Vanilla `VibrationSystem.Data.NBT_TAG_KEY`.
pub const LISTENER_NBT_KEY: &str = "listener";

/// Vanilla `VibrationSystem.User` for an entity.
///
/// Vanilla resolves the listener position through a `PositionSource`; Steel models that as
/// [`Self::listener_position`] because every entity listener in vanilla uses an
/// `EntityPositionSource` whose only variable part is the entity itself.
pub trait EntityVibrationUser: Send + Sync {
    /// Vanilla `VibrationSystem.User.getListenerRadius`.
    fn listener_radius(&self) -> i32;

    /// Vanilla `EntityPositionSource.getPosition`: the entity position plus its eye height, or
    /// `None` when the entity is gone (vanilla returns an empty `Optional` for removed entities).
    fn listener_position(&self) -> Option<DVec3>;

    /// Vanilla `VibrationSystem.User.getPositionSource`.
    ///
    /// Vanilla returns an `EntityPositionSource(entity, eyeHeight)`; the network payload only
    /// needs the entity id and that offset, so Steel returns the encoded source directly.
    /// `None` matches an entity that can no longer be resolved.
    fn position_source(&self) -> Option<PositionSource> {
        None
    }

    /// Vanilla `VibrationSystem.User.getListenableEvents`.
    ///
    /// The default accepts every event with a non-zero vibration frequency, which matches
    /// Vanilla's `#vibrations` game-event tag.
    fn is_listenable_event(&self, event: GameEventRef) -> bool {
        vibration_frequency_for_event(event) != NO_VIBRATION_FREQUENCY
    }

    /// Vanilla `VibrationSystem.User.isValidVibration`, minus the block-tag damping checks Steel
    /// cannot evaluate yet (the same gap as the block-side listener).
    fn is_valid_vibration(&self, event: GameEventRef, context: &GameEventContext<'_>) -> bool {
        if !self.is_listenable_event(event) {
            return false;
        }
        let Some(source) = context.source_entity() else {
            return true;
        };
        if source.is_spectator() {
            return false;
        }
        if source.is_stepping_carefully() && ignores_vibrations_when_sneaking(event) {
            return false;
        }
        true
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
    fn on_data_changed(&self, _world: &Arc<World>) {}

    /// Vanilla `VibrationSystem.User.onReceiveVibration`.
    ///
    /// `source_entity` and `projectile_owner` are the persisted vibration UUIDs resolved through
    /// the world, exactly like vanilla's `VibrationInfo.getEntity`/`getProjectileOwner`.
    fn on_receive_vibration(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        event: GameEventRef,
        source_entity: Option<&SharedEntity>,
        projectile_owner: Option<&SharedEntity>,
        receiving_distance: f32,
    );
}

/// Vanilla `DynamicGameEventListener<VibrationSystem.Listener>` for one entity.
///
/// The listener registers itself in the game-event registry of the chunk section it currently
/// occupies and re-registers when that section (or the world) changes, mirroring
/// `DynamicGameEventListener.add`/`move`/`remove`.
pub struct EntityVibrationListener {
    user: Arc<dyn EntityVibrationUser>,
    state: Arc<SyncMutex<VibrationListenerState>>,
    registration: SyncMutex<Option<(Weak<World>, SectionPos)>>,
}

impl EntityVibrationListener {
    /// Creates a listener over `user` sharing `state` with the owning entity.
    ///
    /// Sharing the state (instead of owning it) lets the entity persist `listener` NBT without
    /// holding the listener, which needs a weak handle back to the entity.
    #[must_use]
    pub fn new(
        user: Arc<dyn EntityVibrationUser>,
        state: Arc<SyncMutex<VibrationListenerState>>,
    ) -> Self {
        Self {
            user,
            state,
            registration: SyncMutex::new(None),
        }
    }

    /// The vibration state shared with the owning entity.
    #[must_use]
    pub const fn state(&self) -> &Arc<SyncMutex<VibrationListenerState>> {
        &self.state
    }

    /// Vanilla `DynamicGameEventListener.add`.
    pub fn add(self: &Arc<Self>, world: &Arc<World>) {
        self.move_registration(world);
    }

    /// Vanilla `DynamicGameEventListener.remove`.
    pub fn remove(self: &Arc<Self>) {
        let Some((world, section)) = self.registration.lock().take() else {
            return;
        };
        if let Some(world) = world.upgrade() {
            world.unregister_game_event_listener(section, &self.shared());
        }
    }

    /// Vanilla `DynamicGameEventListener.move`.
    pub fn move_registration(self: &Arc<Self>, world: &Arc<World>) {
        let Some(position) = self.user.listener_position() else {
            return;
        };
        let current_section = SectionPos::from_block_pos(BlockPos::from(position));
        if self.is_registered_in(world, current_section) {
            return;
        }

        if let Some((previous_world, previous_section)) = self.registration.lock().take()
            && let Some(previous_world) = previous_world.upgrade()
        {
            previous_world.unregister_game_event_listener(previous_section, &self.shared());
        }

        // Vanilla registers only when the target chunk already exists; Steel's
        // `register_game_event_listener` is a no-op for a chunk without full storage, so an
        // unloaded destination behaves the same way.
        world.register_game_event_listener(current_section, self.shared());
        *self.registration.lock() = Some((Arc::downgrade(world), current_section));
    }

    /// Vanilla `VibrationSystem.Ticker.tick`.
    pub fn tick(self: &Arc<Self>, world: &Arc<World>) {
        self.move_registration(world);

        let mut state = self.state.lock();
        if state.current_vibration().is_none() {
            let game_time = world.game_time();
            if let Some(candidate) = state.selector_mut().chosen_candidate(game_time) {
                let travel_time = self.user.calculate_travel_time_in_ticks(candidate.distance);
                let origin = candidate.pos;
                state.set_travel_time_in_ticks(travel_time);
                state.set_current_vibration(Some(candidate));
                state.selector_mut().start_over();
                // Vanilla `trySelectAndScheduleVibration`.
                if let Some(position_source) = self.user.position_source() {
                    world.send_particles(
                        vibration_particle(position_source, travel_time),
                        origin,
                        1,
                        DVec3::ZERO,
                        0.0,
                    );
                }
                drop(state);
                self.user.on_data_changed(world);
                return;
            }
        }

        let Some(vibration) = state.current_vibration().cloned() else {
            return;
        };

        // Vanilla `tryReloadVibrationParticle`.
        if state.should_reload_vibration_particle() {
            let travel_time = state.travel_time_in_ticks();
            let initial_travel_time = self.user.calculate_travel_time_in_ticks(vibration.distance);
            let origin = vibration.pos;
            let destination = self.user.listener_position().unwrap_or(origin);
            let alpha = if initial_travel_time > 0 {
                1.0 - f64::from(travel_time) / f64::from(initial_travel_time)
            } else {
                1.0
            };
            let particle_pos = origin + (destination - origin) * alpha;
            if let Some(position_source) = self.user.position_source() {
                let sent = world.send_particles(
                    vibration_particle(position_source, travel_time),
                    particle_pos,
                    1,
                    DVec3::ZERO,
                    0.0,
                );
                if sent > 0 {
                    state.set_reload_vibration_particle(false);
                }
            }
        }

        let changed = state.travel_time_in_ticks() > 0;
        state.decrement_travel_time();
        if state.travel_time_in_ticks() > 0 {
            drop(state);
            if changed {
                self.user.on_data_changed(world);
            }
            return;
        }

        let origin = BlockPos::from(vibration.pos);
        let destination = self.user.listener_position().map_or(origin, BlockPos::from);
        state.set_current_vibration(None);
        drop(state);

        let source_entity = vibration
            .uuid
            .and_then(|uuid| world.get_entity_by_uuid(&uuid));
        let projectile_owner = vibration
            .projectile_owner_uuid
            .and_then(|uuid| world.get_entity_by_uuid(&uuid));
        self.user.on_receive_vibration(
            world,
            origin,
            vibration.event,
            source_entity.as_ref(),
            projectile_owner.as_ref(),
            distance_between_in_blocks(origin, destination),
        );
        self.user.on_data_changed(world);
    }

    fn is_registered_in(&self, world: &Arc<World>, section: SectionPos) -> bool {
        self.registration
            .lock()
            .as_ref()
            .is_some_and(|(registered_world, registered_section)| {
                registered_section == &section
                    && registered_world
                        .upgrade()
                        .is_some_and(|registered_world| Arc::ptr_eq(&registered_world, world))
            })
    }

    fn shared(self: &Arc<Self>) -> SharedGameEventListener {
        Arc::clone(self) as SharedGameEventListener
    }
}

impl GameEventListener for EntityVibrationListener {
    fn listener_pos(&self) -> Option<DVec3> {
        self.user.listener_position()
    }

    fn listener_radius(&self) -> i32 {
        self.user.listener_radius()
    }

    fn delivery_mode(&self) -> GameEventDeliveryMode {
        // Same choice as the block-side `VibrationSystemListener`: vanilla's registry has no
        // ordering mode, and the selector re-sorts candidates by distance anyway.
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

        if !self.user.is_valid_vibration(event, context) {
            return false;
        }

        let Some(destination) = self.user.listener_position() else {
            return false;
        };
        let source_block_pos = BlockPos::from(source_pos);
        if !self
            .user
            .can_receive_vibration(world, source_block_pos, event, context)
        {
            return false;
        }

        // TODO(Protocol): Vanilla `VibrationSystem.Listener.isOccluded` raycasts in all six
        // directions against `BlockTags.OCCLUDES_VIBRATION_SIGNALS`; the block-side listener has
        // the same gap.
        let mut vibration =
            VibrationInfo::new(event, source_pos.distance(destination) as f32, source_pos);
        if let Some(source) = context.source_entity() {
            vibration.uuid = Some(source.uuid());
            vibration.projectile_owner_uuid = source.projectile_owner_uuid();
        }
        self.state
            .lock()
            .selector_mut()
            .add_candidate(vibration, world.game_time());
        true
    }
}

/// Writes the `listener` compound of a [`VibrationListenerState`] (vanilla
/// `VibrationSystem.Data.CODEC`).
///
/// The key layout matches the sculk block entities' `listener` compound so both halves of the
/// vibration system persist identically.
pub fn save_listener_nbt(nbt: &mut NbtCompound, state: &VibrationListenerState) {
    let mut listener = NbtCompound::new();
    if let Some(current) = state.current_vibration() {
        let mut event = NbtCompound::new();
        event.insert("game_event", current.event.key.to_string());
        event.insert("distance", current.distance);
        event.insert("x", current.pos.x);
        event.insert("y", current.pos.y);
        event.insert("z", current.pos.z);
        if let Some(uuid) = current.uuid {
            event.insert("source", uuid_nbt(uuid));
        }
        if let Some(uuid) = current.projectile_owner_uuid {
            event.insert("projectile_owner", uuid_nbt(uuid));
        }
        listener.insert("event", event);
    }
    listener.insert("event_delay", state.travel_time_in_ticks());
    nbt.insert(LISTENER_NBT_KEY, listener);
}

/// Restores a [`VibrationListenerState`] from its `listener` compound.
pub fn restore_listener(state: &SyncMutex<VibrationListenerState>, nbt: &NbtCompoundView<'_, '_>) {
    let Some(listener) = nbt.compound(LISTENER_NBT_KEY) else {
        return;
    };

    let mut guard = state.lock();
    if let Some(event) = listener.compound("event") {
        let key = event
            .string("game_event")
            .and_then(|key| key.to_string_lossy().parse::<Identifier>().ok());
        if let Some(event_ref) = key.and_then(|key| REGISTRY.game_events.by_key(&key)) {
            guard.set_current_vibration(Some(VibrationInfo {
                event: event_ref,
                distance: event.float("distance").unwrap_or(0.0),
                pos: DVec3::new(
                    event.double("x").unwrap_or(0.0),
                    event.double("y").unwrap_or(0.0),
                    event.double("z").unwrap_or(0.0),
                ),
                uuid: event
                    .int_array("source")
                    .and_then(|value| uuid_from_nbt(&value)),
                projectile_owner_uuid: event
                    .int_array("projectile_owner")
                    .and_then(|value| uuid_from_nbt(&value)),
            }));
        }
    }
    guard.set_travel_time_in_ticks(listener.int("event_delay").unwrap_or(0));
}

/// Vanilla `UUIDUtil.CODEC`: four big-endian `i32`s.
fn uuid_nbt(uuid: Uuid) -> NbtTag {
    let bits = uuid.as_u128();
    let half = |shift: u32| ((bits >> shift) & u128::from(u32::MAX)) as i32;
    NbtTag::IntArray(vec![half(96), half(64), half(32), half(0)])
}

fn uuid_from_nbt(values: &[i32]) -> Option<Uuid> {
    let [a, b, c, d] = <[i32; 4]>::try_from(values).ok()?;
    let mut bits = 0_u128;
    for value in [a, b, c, d] {
        bits = (bits << 32) | u128::from(value as u32);
    }
    Some(Uuid::from_u128(bits))
}
