//! Vanilla `SculkShriekerBlockEntity`.
//!
//! Vanilla reference: `SculkShriekerBlockEntity`. Owns `warning_level` persistence, the
//! 90-tick shriek (block state + `PARTICLES_SCULK_SHRIEK` level event 3007 + `shriek` game event),
//! and a vibration listener restricted to `GameEventTags.SHRIEKER_CAN_LISTEN`
//! (`sculk_sensor_tendrils_clicking`) whose vibrations must be caused by a player.
//!
//! Warden summoning and the warning escalation (`WardenSpawnTracker.tryWarn`, darkness) live in
//! `MobAI`, so this entity records the shriek and leaves the summon path as a marked dependency.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::NbtCompound;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, BoolProperty};
use steel_registry::game_events::GameEventRef;
use steel_registry::sound_events;
use steel_registry::{vanilla_block_entity_types, vanilla_game_events};
use steel_utils::types::Difficulty;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, locks::SyncMutex};

use crate::behavior::blocks::SculkShriekerBlock;
use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::world::game_event::vibration::{
    SHRIEKER_LISTENER_RADIUS, VibrationCallback, VibrationSystemListener,
};
use crate::world::game_event::{GameEventContext, SharedGameEventListener};
use crate::world::{LevelReader, World};

/// Vanilla `SculkShriekerBlockEntity.WARNING_SOUND_RADIUS`.
const WARNING_SOUND_RADIUS: i32 = 10;

const SHRIEKING: &BoolProperty = &BlockStateProperties::SHRIEKING;
const CAN_SUMMON: &BoolProperty = &BlockStateProperties::CAN_SUMMON;

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

    /// Vanilla `tryShriek`: only when not already shrieking; warning escalation is MobAI's.
    fn try_shriek(&self, world: &Arc<World>, has_player: bool) {
        if !has_player {
            return;
        }
        let state = world.get_block_state(self.pos);
        if SculkShriekerBlock::is_shrieking(state) {
            return;
        }
        *self.warning_level.lock() = 0;
        // TODO(MobAI): Vanilla calls `WardenSpawnTracker.tryWarn(level, pos, player)` first and only
        // shrieks when the tracker refuses to escalate. Steel has no `WardenSpawnTracker`, so the
        // tracker gate is skipped and the shriek always runs.
        self.shriek(world);
        *self.shriek_count.lock() += 1;
    }

    /// Vanilla `shriek`.
    fn shriek(&self, world: &Arc<World>) {
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
        self.set_changed(world);
    }

    /// Vanilla `canRespond`.
    fn can_respond(&self, world: &Arc<World>) -> bool {
        let state = world.get_block_state(self.pos);
        // TODO(MobAI): Vanilla additionally requires the `spawn_wardens` game rule.
        state.get_value(CAN_SUMMON) && world.difficulty() != Difficulty::Peaceful
    }

    /// Vanilla `tryRespond`: summon the warden or play the warning reply, then darkness.
    fn try_respond(&self, world: &Arc<World>) {
        if !self.can_respond(world) {
            return;
        }
        let warning_level = *self.warning_level.lock();
        if warning_level <= 0 {
            return;
        }
        // TODO(MobAI): Vanilla `trySummonWarden` (`SpawnUtil.trySpawnMob(WARDEN, ...)`, 20
        // attempts, 5x6 range) runs first and `playWardenReplySound` is only the fallback. Steel
        // has no warden entity/goal integration, so only the sound fallback is wired. Vanilla then
        // applies `Warden.applyDarknessAround(level, center, null, 40)`, which needs the darkness
        // mob-effect path.
        self.play_warden_reply_sound(world, warning_level);
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

/// Vanilla `SculkShriekerBlockEntity.tryGetPlayer`.
fn player_source(source: Option<&dyn crate::entity::Entity>) -> bool {
    let Some(entity) = source else {
        return false;
    };
    if entity.as_player().is_some() {
        return true;
    }
    if let Some(owner) = entity
        .as_projectile()
        .and_then(|projectile| projectile.get_owner())
    {
        return owner.as_player().is_some();
    }
    false
}

impl VibrationCallback for ShriekerCallback {
    fn listener_radius(&self) -> i32 {
        SHRIEKER_LISTENER_RADIUS
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
            && player_source(context.source_entity())
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
    ) {
        let has_player = source_uuid
            .and_then(|uuid| world.get_entity_by_uuid(&uuid))
            .is_some_and(|entity| player_source(Some(entity.as_ref())));
        self.core.try_shriek(world, has_player);
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

    /// Current warning level (0-4). Vanilla escalates this through `WardenSpawnTracker`.
    #[must_use]
    pub fn warning_level(&self) -> i32 {
        *self.core.warning_level.lock()
    }

    /// Number of shrieks this entity has started (Steel-side observable, Vanilla keeps no counter).
    #[must_use]
    pub fn shriek_count(&self) -> i32 {
        *self.core.shriek_count.lock()
    }

    /// Vanilla `SculkShriekerBlockEntity.tryShriek`.
    pub fn try_shriek(&self, world: &Arc<World>, source_uuid: Option<uuid::Uuid>) {
        let has_player = source_uuid.is_some();
        self.core.try_shriek(world, has_player);
    }

    /// Vanilla `tryRespond`, called from the block tick once `shrieking` expires.
    pub fn try_respond(&self, world: &Arc<World>) {
        self.core.try_respond(world);
    }

    /// Vanilla `preRemoveSideEffects`: respond when removed mid-shriek.
    fn respond_if_shrieking(&self, world: &Arc<World>) {
        if SculkShriekerBlock::is_shrieking(world.get_block_state(self.core.pos)) {
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
        // Vanilla also restores the `listener` compound here; Steel keeps only `warning_level`
        // because in-flight shrieker vibrations are always player-attributed on arrival.
        if let Some(warning) = nbt.int("warning_level") {
            *self.core.warning_level.lock() = warning;
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        nbt.insert("warning_level", *self.core.warning_level.lock());
    }

    fn tick(&self, world: &Arc<World>) {
        self.listener.tick(world);
    }

    fn game_event_listener(&self) -> Option<SharedGameEventListener> {
        Some(Arc::clone(&self.listener) as SharedGameEventListener)
    }

    fn pre_remove_side_effects(&self, _pos: BlockPos, _state: BlockStateId) {
        if let Some(world) = self.get_level() {
            self.respond_if_shrieking(&world);
        }
    }
}
