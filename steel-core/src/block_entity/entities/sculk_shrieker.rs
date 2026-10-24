//! Vanilla `SculkShriekerBlockEntity`.
//!
//! Vanilla reference: `SculkShriekerBlockEntity`. Owns `warning_level` persistence, the
//! 90-tick shriek (block state + `PARTICLES_SCULK_SHRIEK` level event 3007 + `shriek` game event),
//! the `WardenSpawnTracker` warning escalation, the warden summon on the final warning level, and a
//! vibration listener restricted to `GameEventTags.SHRIEKER_CAN_LISTEN`
//! (`sculk_sensor_tendrils_clicking`) whose vibrations must be caused by a player.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::NbtCompound;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, BoolProperty};
use steel_registry::game_events::GameEventRef;
use steel_registry::position_source::PositionSource;
use steel_registry::sound_events;
use steel_registry::{vanilla_block_entity_types, vanilla_game_events};
use steel_utils::Downcast as _;
use steel_utils::types::Difficulty;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, locks::SyncMutex};

use crate::behavior::blocks::SculkShriekerBlock;
use crate::block_entity::entities::sculk_sensor::{
    load_listener_nbt, restore_vibration, save_listener_nbt,
};
use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::entity::Entity;
use crate::entity::entities::ItemEntity;
use crate::entity::entities::mobs::hostile::warden::WardenSpawnTracker;
use crate::player::Player;
use crate::world::game_event::vibration::{
    SHRIEKER_LISTENER_RADIUS, VibrationCallback, VibrationSystemListener, block_position_source,
};
use crate::world::game_event::{GameEventContext, SharedGameEventListener};
use crate::world::{LevelReader, World};

/// Vanilla `SculkShriekerBlockEntity.WARNING_SOUND_RADIUS`.
const WARNING_SOUND_RADIUS: i32 = 10;

/// Vanilla `SculkShriekerBlockEntity.MAX_WARNING_LEVEL` gate for `trySummonWarden`.
const WARDEN_SUMMON_WARNING_LEVEL: i32 = 4;

const SHRIEKING: &BoolProperty = &BlockStateProperties::SHRIEKING;
const CAN_SUMMON: &BoolProperty = &BlockStateProperties::CAN_SUMMON;

/// Vanilla `SculkShriekerBlockEntity.tryGetPlayer`.
fn try_get_player(world: &Arc<World>, source: &dyn Entity) -> Option<Arc<Player>> {
    if source.as_player().is_some() {
        return world.players.get_by_entity_id(source.id());
    }
    if let Some(controller) = source.controlling_passenger()
        && controller.as_player().is_some()
    {
        return world.players.get_by_entity_id(controller.id());
    }
    if let Some(owner) = source
        .as_projectile()
        .and_then(|projectile| projectile.get_owner())
        && owner.as_player().is_some()
    {
        return world.players.get_by_entity_id(owner.id());
    }
    // Vanilla `ItemEntity.getOwner()` is a UUID, resolved through the level.
    let item_owner = source
        .downcast_ref::<ItemEntity>()
        .and_then(|item| item.get_owner())
        .and_then(|uuid| world.get_entity_by_uuid(&uuid))?;
    if item_owner.as_player().is_some() {
        return world.players.get_by_entity_id(item_owner.id());
    }
    None
}

/// Vanilla `SculkShriekerBlockEntity.tryGetPlayer` for a persisted source UUID.
fn player_for_uuid(world: &Arc<World>, source_uuid: Option<uuid::Uuid>) -> Option<Arc<Player>> {
    let source = world.get_entity_by_uuid(&source_uuid?)?;
    try_get_player(world, source.as_ref())
}

/// State shared between the entity and its listener callback.
struct ShriekerCore {
    pos: BlockPos,
    warning_level: SyncMutex<i32>,
    shriek_count: SyncMutex<i32>,
}

impl ShriekerCore {
    fn set_changed(&self, world: &Arc<World>) {
        if let Some(entity) = world.get_block_entity(self.pos) {
            entity.set_changed();
        }
    }

    /// Vanilla `tryShriek`: reset the warning level, escalate the players' `WardenSpawnTracker`
    /// (which suppresses the shriek on the tick the tracker escalates), otherwise shriek.
    fn try_shriek(&self, world: &Arc<World>, player: Option<&Arc<Player>>) {
        let Some(player) = player else {
            return;
        };
        let state = world.get_block_state(self.pos);
        if SculkShriekerBlock::is_shrieking(state) {
            return;
        }
        *self.warning_level.lock() = 0;
        if !self.can_respond(world) || self.try_to_warn(world, player) {
            self.shriek(world, Some(player.as_ref() as &dyn Entity));
            *self.shriek_count.lock() += 1;
        }
    }

    /// Vanilla `tryToWarn`.
    fn try_to_warn(&self, world: &Arc<World>, player: &Arc<Player>) -> bool {
        let maybe_warning_level = WardenSpawnTracker::try_warn(world, self.pos, player);
        if let Some(warning_level) = maybe_warning_level {
            *self.warning_level.lock() = warning_level;
        }
        maybe_warning_level.is_some()
    }

    /// Vanilla `shriek`: set `SHRIEKING`, schedule the 90-tick expiry, emit the client particle
    /// event, then announce the `shriek` game event with the triggering player as its source.
    fn shriek(&self, world: &Arc<World>, source: Option<&dyn Entity>) {
        let state = world.get_block_state(self.pos);
        if state.get_value(SHRIEKING) {
            return;
        }
        SculkShriekerBlock::start_shriek(state.get_block(), world, self.pos, state);
        // Vanilla emits `LevelEvent 3007` (`PARTICLES_SCULK_SHRIEK`) while starting the shriek.
        world.level_event(
            steel_registry::level_events::PARTICLES_SCULK_SHRIEK,
            self.pos,
            0,
            None,
        );
        world.game_event(
            &vanilla_game_events::SHRIEK,
            self.pos,
            &GameEventContext::new(source, None),
        );
        self.set_changed(world);
    }

    /// Vanilla `canRespond`.
    fn can_respond(&self, world: &Arc<World>) -> bool {
        let state = world.get_block_state(self.pos);
        state.get_value(CAN_SUMMON)
            && world.difficulty() != Difficulty::Peaceful
            && WardenSpawnTracker::can_spawn(world)
    }

    /// Vanilla `tryRespond`: summon the warden or play the warning reply, then apply darkness.
    fn try_respond(&self, world: &Arc<World>) {
        if !self.can_respond(world) {
            return;
        }
        let warning_level = *self.warning_level.lock();
        if warning_level <= 0 {
            return;
        }
        if !self.try_summon_warden(world, warning_level) {
            self.play_warden_reply_sound(world, warning_level);
        }
        // TODO(MobAI): Vanilla also runs `Warden.applyDarknessAround(level, center, null, 40)`;
        // Steel's darkness helper is an instance method on `WardenEntity`, so the static
        // center-based variant has to land in the Warden lane first.
    }

    /// Vanilla `trySummonWarden`.
    fn try_summon_warden(&self, world: &Arc<World>, warning_level: i32) -> bool {
        if warning_level < WARDEN_SUMMON_WARNING_LEVEL {
            return false;
        }
        // Vanilla `SpawnUtil.trySpawnMob(WARDEN, TRIGGERED, level, pos, 20, 5, 6,
        // ON_TOP_OF_COLLIDER, false)`; Steel's tracker helper spawns at the shrieker's own column.
        WardenSpawnTracker::summon_warden(world, self.pos).is_some()
    }

    /// Vanilla `playWardenReplySound` (`SOUND_BY_LEVEL`).
    fn play_warden_reply_sound(&self, world: &Arc<World>, warning_level: i32) {
        let sound = match warning_level {
            1 => &sound_events::ENTITY_WARDEN_NEARBY_CLOSE,
            2 => &sound_events::ENTITY_WARDEN_NEARBY_CLOSER,
            3 => &sound_events::ENTITY_WARDEN_NEARBY_CLOSEST,
            4 => &sound_events::ENTITY_WARDEN_LISTENING_ANGRY,
            _ => return,
        };
        let x = f64::from(
            self.pos.x() + rand::random_range(-WARNING_SOUND_RADIUS..=WARNING_SOUND_RADIUS),
        );
        let y = f64::from(
            self.pos.y() + rand::random_range(-WARNING_SOUND_RADIUS..=WARNING_SOUND_RADIUS),
        );
        let z = f64::from(
            self.pos.z() + rand::random_range(-WARNING_SOUND_RADIUS..=WARNING_SOUND_RADIUS),
        );
        world.play_sound_at(
            sound,
            SoundSource::Hostile,
            DVec3::new(x, y, z),
            5.0,
            1.0,
            None,
        );
    }
}

/// Vanilla `VibrationUser`: only `SHRIEKER_CAN_LISTEN` events from a player.
struct ShriekerCallback {
    core: Arc<ShriekerCore>,
}

impl VibrationCallback for ShriekerCallback {
    fn listener_radius(&self) -> i32 {
        SHRIEKER_LISTENER_RADIUS
    }

    /// Vanilla `SculkShriekerBlockEntity.VibrationUser.requiresAdjacentChunksToBeTicking`.
    fn requires_adjacent_chunks_to_be_ticking(&self) -> bool {
        true
    }

    fn position_source(&self) -> PositionSource {
        block_position_source(self.core.pos)
    }

    fn is_listenable_event(&self, event: GameEventRef) -> bool {
        // Vanilla `GameEventTags.SHRIEKER_CAN_LISTEN` contains only the sensor clicking event.
        event.key == vanilla_game_events::SCULK_SENSOR_TENDRILS_CLICKING.key
    }

    fn can_receive_vibration(
        &self,
        world: &Arc<World>,
        _pos: BlockPos,
        _event: GameEventRef,
        context: &GameEventContext<'_>,
    ) -> bool {
        !SculkShriekerBlock::is_shrieking(world.get_block_state(self.core.pos))
            && context
                .source_entity()
                .is_some_and(|source| try_get_player(world, source).is_some())
    }

    fn on_data_changed(&self, world: &Arc<World>) {
        self.core.set_changed(world);
    }

    fn on_signal_received(&self, _world: &Arc<World>, _event: GameEventRef, _distance: f32) {
        // Arrival always carries a source UUID; `on_signal_received_from` handles it.
    }

    fn on_signal_received_from(
        &self,
        world: &Arc<World>,
        _event: GameEventRef,
        _distance: f32,
        source_uuid: Option<uuid::Uuid>,
        projectile_owner_uuid: Option<uuid::Uuid>,
    ) {
        // Vanilla `VibrationUser.onReceiveVibration` resolves the player from the projectile owner
        // *instead of* the source entity when a projectile owner is present.
        let player = player_for_uuid(world, projectile_owner_uuid.or(source_uuid));
        self.core.try_shriek(world, player.as_ref());
    }
}

/// Vanilla `SculkShriekerBlockEntity`.
pub struct SculkShriekerBlockEntity {
    base: BlockEntityBase,
    core: Arc<ShriekerCore>,
    listener: Arc<VibrationSystemListener>,
}

// SAFETY: Steel-owned key uniquely identifies this concrete entity type.
unsafe impl DowncastType for SculkShriekerBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/sculk_shrieker");
}

impl SculkShriekerBlockEntity {
    /// Creates a shrieker block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        let core = Arc::new(ShriekerCore {
            pos,
            warning_level: SyncMutex::new(0),
            shriek_count: SyncMutex::new(0),
        });
        let callback: Arc<dyn VibrationCallback> = Arc::new(ShriekerCallback {
            core: Arc::clone(&core),
        });
        let listener = Arc::new(VibrationSystemListener::new(pos, callback));
        Self {
            base: BlockEntityBase::new(
                &vanilla_block_entity_types::SCULK_SHRIEKER,
                level,
                pos,
                state,
            ),
            core,
            listener,
        }
    }

    /// Current warning level (0-4).
    #[must_use]
    pub fn warning_level(&self) -> i32 {
        *self.core.warning_level.lock()
    }

    /// Number of shrieks this entity has started (Steel-side observable, Vanilla keeps no counter).
    #[must_use]
    pub fn shriek_count(&self) -> i32 {
        *self.core.shriek_count.lock()
    }

    /// Vanilla `SculkShriekerBlockEntity.tryShriek` for a source entity UUID.
    pub fn try_shriek(&self, world: &Arc<World>, source_uuid: Option<uuid::Uuid>) {
        let player = player_for_uuid(world, source_uuid);
        self.core.try_shriek(world, player.as_ref());
    }

    /// Vanilla `SculkShriekerBlockEntity.tryRespond`, called from the block tick once `shrieking`
    /// expires.
    pub fn try_respond(&self, world: &Arc<World>) {
        self.core.try_respond(world);
    }

    /// Vanilla `preRemoveSideEffects`: respond when the removed state was mid-shriek.
    ///
    /// Vanilla reads the *passed* state, which still holds `SHRIEKING` while the block is being
    /// replaced; re-reading the world here would already see the new block.
    fn respond_if_shrieking(&self, world: &Arc<World>, state: BlockStateId) {
        if SculkShriekerBlock::is_shrieking(state) {
            self.core.try_respond(world);
        }
    }
}

impl BlockEntity for SculkShriekerBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let nbt: NbtCompoundView<'_, '_> = nbt.into();
        if let Some(warning) = nbt.int("warning_level") {
            *self.core.warning_level.lock() = warning;
        }
        restore_vibration(self.listener.state(), load_listener_nbt(&nbt));
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        nbt.insert("warning_level", *self.core.warning_level.lock());
        save_listener_nbt(nbt, &self.listener.state().lock());
    }

    fn tick(&self, world: &Arc<World>) {
        self.listener.tick(world);
    }

    fn game_event_listener(&self) -> Option<SharedGameEventListener> {
        Some(Arc::clone(&self.listener) as SharedGameEventListener)
    }

    fn pre_remove_side_effects(&self, _pos: BlockPos, state: BlockStateId) {
        if let Some(world) = self.get_level() {
            self.respond_if_shrieking(&world, state);
        }
    }
}
