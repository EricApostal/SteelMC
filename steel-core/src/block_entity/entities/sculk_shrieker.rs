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
use steel_registry::level_events;
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
use crate::entity::entities::mobs::hostile::warden::{WardenEntity, WardenSpawnTracker};
use crate::entity::projectile::Projectile;
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

/// Vanilla `SculkShriekerBlockEntity.tryRespond` darkness radius
/// (`Warden.applyDarknessAround(level, center, null, 40)`).
const SHRIEK_DARKNESS_RADIUS: i32 = 40;

const SHRIEKING: &BoolProperty = &BlockStateProperties::SHRIEKING;
const CAN_SUMMON: &BoolProperty = &BlockStateProperties::CAN_SUMMON;

/// Vanilla `Vec3.atCenterOf(pos)`.
fn pos_center(pos: BlockPos) -> DVec3 {
    DVec3::new(
        f64::from(pos.x()) + 0.5,
        f64::from(pos.y()) + 0.5,
        f64::from(pos.z()) + 0.5,
    )
}

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
    if let Some(owner) = source.as_projectile().and_then(Projectile::get_owner)
        && owner.as_player().is_some()
    {
        return world.players.get_by_entity_id(owner.id());
    }
    // Vanilla `ItemEntity.getOwner()` is a UUID, resolved through the level.
    let item_owner = source
        .downcast_ref::<ItemEntity>()
        .and_then(ItemEntity::get_owner)
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
        world.level_event(level_events::PARTICLES_SCULK_SHRIEK, self.pos, 0, None);
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
        // Vanilla `tryRespond`: `Warden.applyDarknessAround(level, Vec3.atCenterOf(pos), null, 40)`
        // after every escalated reply (the summon branch included, exactly as vanilla does).
        WardenEntity::apply_darkness_around_at(world, pos_center(self.pos), SHRIEK_DARKNESS_RADIUS);
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::DVec3;
    use steel_registry::entity_data::EntityPose;
    use steel_registry::{vanilla_blocks, vanilla_mob_effects};
    use steel_utils::types::UpdateFlags;
    use steel_utils::{ChunkPos, WorldAabb};

    use super::*;
    use crate::block_entity::SharedBlockEntity;
    use crate::bootstrap::init_globals;
    use crate::entity::ai::behavior::warden as warden_ai;
    use crate::entity::ai::brain::memory::module_type as mem;
    use crate::entity::entities::mobs::hostile::warden::WardenEntity;
    use crate::entity::{Entity, LivingEntity, Mob, SharedEntity};
    use crate::player::ResetReason;
    use crate::test_support::{
        TestPlayerBuilder, TestWorld, fresh_test_world, insert_ready_full_chunk,
    };
    use crate::world::World;

    /// The shrieker sits at the center of chunk `(0, 0)`.
    const SHRIEKER_POS: BlockPos = BlockPos::new(8, 64, 8);
    const SHRIEKER_CENTER: DVec3 = DVec3::new(8.5, 64.5, 8.5);

    fn shrieker_world(key: &'static str) -> TestWorld {
        init_globals();
        let world = fresh_test_world(key);
        insert_ready_full_chunk(&world.world, ChunkPos::from_block_pos(SHRIEKER_POS));
        let state = vanilla_blocks::SCULK_SHRIEKER
            .default_state()
            .set_value(CAN_SUMMON, true);
        assert!(
            world
                .world
                .set_block(SHRIEKER_POS, state, UpdateFlags::UPDATE_NONE)
        );
        world
    }

    fn shrieker(world: &Arc<World>) -> SharedBlockEntity {
        let entity = world
            .get_block_entity(SHRIEKER_POS)
            .expect("placing a sculk shrieker must create its block entity");
        assert!(
            entity.downcast_ref::<SculkShriekerBlockEntity>().is_some(),
            "the shrieker block entity must keep its concrete type"
        );
        entity
    }

    fn shrieker_core(placed: &SharedBlockEntity) -> &SculkShriekerBlockEntity {
        placed
            .downcast_ref::<SculkShriekerBlockEntity>()
            .expect("checked concrete shrieker type")
    }

    fn observer(world: &Arc<World>, name: &str, id: i32, pos: DVec3) -> Arc<Player> {
        let player = TestPlayerBuilder::new(Arc::clone(world), name, id).build();
        player.set_client_loaded(true);
        player.base().set_position_local(pos);
        assert!(world.add_player(Arc::clone(&player), ResetReason::InitialJoin));
        player
    }

    #[test]
    fn response_applies_darkness_at_the_vanilla_duration_and_radius() {
        let fixture = shrieker_world("shrieker_response_darkness");
        let world = &fixture.world;

        let inside = observer(world, "ShriekerInside", 1, SHRIEKER_CENTER);
        // 41 blocks above the block center: just outside `SHRIEK_DARKNESS_RADIUS`.
        let outside = observer(
            world,
            "ShriekerOutside",
            2,
            SHRIEKER_CENTER + DVec3::Y * 41.0,
        );

        let placed = shrieker(world);
        // Below `WARDEN_SUMMON_WARNING_LEVEL`: the reply branch, which vanilla follows
        // with `applyDarknessAround`.
        *shrieker_core(&placed).core.warning_level.lock() = 1;
        shrieker_core(&placed).try_respond(world);

        let effect = inside
            .mob_effect(vanilla_mob_effects::DARKNESS)
            .expect("the escalated shriek must darken a nearby survival player");
        assert_eq!(effect.duration(), 260);
        assert_eq!(effect.amplifier(), 0);
        assert!(
            outside.mob_effect(vanilla_mob_effects::DARKNESS).is_none(),
            "a player beyond the 40-block radius must stay unaffected"
        );
    }

    #[test]
    fn summoning_a_warden_puts_it_in_the_emerging_state() {
        let fixture = shrieker_world("shrieker_summons_emerging_warden");
        let world = &fixture.world;

        let placed = shrieker(world);
        *shrieker_core(&placed).core.warning_level.lock() = WARDEN_SUMMON_WARNING_LEVEL;
        shrieker_core(&placed).try_respond(world);

        let search = WorldAabb::of_size(SHRIEKER_CENTER + DVec3::Y * 0.5, 16.0, 16.0, 16.0);
        let summoned: Vec<SharedEntity> = world
            .get_entities_in_aabb_matching(&search, |entity| {
                entity.downcast_ref::<WardenEntity>().is_some()
            })
            .into_iter()
            .collect();
        assert_eq!(
            summoned.len(),
            1,
            "the shrieker must summon exactly one warden"
        );

        let warden = summoned[0]
            .downcast_ref::<WardenEntity>()
            .expect("the summoned entity must be a WardenEntity");
        assert_eq!(
            Entity::pose(warden),
            EntityPose::Emerging,
            "a TRIGGERED warden must spawn in the EMERGING pose"
        );

        let brain = Mob::brain(warden).expect("a warden owns a brain").lock();
        assert!(brain.has_memory_value(&mem::IS_EMERGING));
        assert_eq!(
            brain.time_until_expiry(&mem::IS_EMERGING),
            i64::from(warden_ai::EMERGE_DURATION),
            "the emerge memory must run for the vanilla EMERGE_DURATION"
        );
    }
}
