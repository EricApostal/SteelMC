//! Ominous item spawner entity implementation.
//!
//! Mirrors vanilla `OminousItemSpawner`: an invisible, no-physics entity created by an
//! ominous trial spawner that waits a randomized delay and then ejects its item.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::ToNbtTag;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtTag};
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::level_events::PARTICLES_TRIAL_SPAWNER_SPAWN_ITEM;
use steel_registry::particle_type::ParticleData;
use steel_registry::sound_events::BLOCK_TRIAL_SPAWNER_ABOUT_TO_SPAWN_ITEM;
use steel_registry::vanilla_entity_data::OminousItemSpawnerEntityData;
use steel_registry::vanilla_game_events::ENTITY_PLACE;
use steel_registry::vanilla_particle_types::OMINOUS_SPAWNING;
use steel_utils::locks::SyncMutex;
use steel_utils::{DowncastType, DowncastTypeKey};

use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityEventSource as _, EntityMovementEmission,
    EntitySyncedData,
};
use crate::world::World;
use crate::world::game_event::GameEventContext;

/// Vanilla `OminousItemSpawner.SPAWN_ITEM_DELAY_MIN`.
const SPAWN_ITEM_DELAY_MIN: i32 = 60;

/// Vanilla `OminousItemSpawner.SPAWN_ITEM_DELAY_MAX`.
const SPAWN_ITEM_DELAY_MAX: i32 = 120;

/// Vanilla `OminousItemSpawner.TICKS_BEFORE_ABOUT_TO_SPAWN_SOUND`.
const TICKS_BEFORE_ABOUT_TO_SPAWN_SOUND: i64 = 36;

/// Vanilla `OminousItemSpawner.addParticles` particle offset radius.
const PARTICLE_OFFSET_RADIUS: f64 = 0.4;

/// An ominous item spawner.
///
/// Vanilla creates this entity above an ominous trial spawner; it plays the
/// about-to-spawn sound shortly before ejecting its synchronized item, then removes
/// itself. The trial spawner keeps the linkage by entity UUID, so this entity itself
/// only persists the item and its spawn delay.
#[entity_behavior(class = "OminousItemSpawner")]
pub struct OminousItemSpawnerEntity {
    /// Common entity fields (id, uuid, position, etc.).
    base: EntityBase,

    /// Vanilla entity type registered for this implementation.
    entity_type: EntityTypeRef,

    /// Synchronized item stack rendered by the client.
    entity_data: SyncMutex<OminousItemSpawnerEntityData>,

    /// Tick count at which the item is ejected (vanilla `spawnItemAfterTicks`).
    spawn_item_after_ticks: SyncMutex<i64>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `OminousItemSpawnerEntity`.
unsafe impl DowncastType for OminousItemSpawnerEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/ominous_item_spawner");
}

impl OminousItemSpawnerEntity {
    /// Creates an ominous item spawner entity.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        let base = EntityBase::new(id, position, entity_type.dimensions, world);
        base.set_no_physics(true);
        Self {
            base,
            entity_type,
            entity_data: SyncMutex::new(OminousItemSpawnerEntityData::new()),
            spawn_item_after_ticks: SyncMutex::new(0),
        }
    }

    /// Creates an ominous item spawner the way vanilla `OminousItemSpawner.create` does:
    /// a randomized spawn delay in `60..=120` ticks and the given item.
    ///
    /// # Arguments
    /// * `entity_type` - The vanilla entity type registered for this implementation.
    /// * `id` - The runtime entity id.
    /// * `position` - The spawn position.
    /// * `item` - The item to eject after the delay.
    /// * `world` - The world this entity belongs to.
    #[must_use]
    pub fn create(
        entity_type: EntityTypeRef,
        id: i32,
        position: DVec3,
        item: ItemStack,
        world: Weak<World>,
    ) -> Self {
        let spawner = Self::new(entity_type, id, position, world);
        spawner.set_spawn_item_after_ticks(i64::from(rand::random_range(
            SPAWN_ITEM_DELAY_MIN..=SPAWN_ITEM_DELAY_MAX,
        )));
        spawner.set_item(item);
        spawner
    }

    /// Creates an ominous item spawner from saved data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        let base = EntityBase::from_load(load, entity_type.dimensions);
        base.set_no_physics(true);
        Self {
            base,
            entity_type,
            entity_data: SyncMutex::new(OminousItemSpawnerEntityData::new()),
            spawn_item_after_ticks: SyncMutex::new(0),
        }
    }

    /// Returns the item that will be ejected.
    #[must_use]
    pub fn item(&self) -> ItemStack {
        self.entity_data.lock().item.get().clone()
    }

    /// Sets the item that will be ejected.
    pub fn set_item(&self, item: ItemStack) {
        self.entity_data.lock().item.set(item);
    }

    /// Returns the tick count at which the item is ejected.
    #[must_use]
    pub fn spawn_item_after_ticks(&self) -> i64 {
        *self.spawn_item_after_ticks.lock()
    }

    /// Sets the tick count at which the item is ejected.
    pub fn set_spawn_item_after_ticks(&self, ticks: i64) {
        *self.spawn_item_after_ticks.lock() = ticks;
    }

    /// Vanilla `OminousItemSpawner.tickServer`.
    fn tick_server(self: &Arc<Self>, world: &Arc<World>) {
        let tick_count = i64::from(self.tick_count());
        let spawn_after = self.spawn_item_after_ticks();

        if tick_count == spawn_after - TICKS_BEFORE_ABOUT_TO_SPAWN_SOUND {
            world.play_sound(
                &BLOCK_TRIAL_SPAWNER_ABOUT_TO_SPAWN_ITEM,
                SoundSource::Neutral,
                self.block_position(),
                1.0,
                1.0,
                None,
            );
        }

        if tick_count >= spawn_after {
            self.spawn_item(world);
            self.kill(world);
        }
    }

    /// Vanilla `OminousItemSpawner.spawnItem`.
    ///
    /// Vanilla shoots `ProjectileItem` stacks downwards instead of dropping them. Steel
    /// has no `ProjectileItem`/dispense-config dispatch yet (see the TODOs in
    /// `behavior/items/throwable_potion.rs` and `behavior/items/tipped_arrow.rs`), so
    /// projectile stacks currently take the item-entity path.
    fn spawn_item(self: &Arc<Self>, world: &Arc<World>) {
        let item = self.item();
        if item.is_empty() {
            return;
        }

        let spawned = world.spawn_item(self.position(), item);
        world.level_event(
            PARTICLES_TRIAL_SPAWNER_SPAWN_ITEM,
            self.block_position(),
            1,
            None,
        );
        if let Some(spawned) = spawned {
            world.game_event_at(
                &ENTITY_PLACE,
                self.position(),
                &GameEventContext::new(Some(spawned.as_entity_event_source()), None),
            );
        }
        self.set_item(ItemStack::empty());
    }

    /// Vanilla `OminousItemSpawner.addParticles`.
    fn add_particles(&self, world: &Arc<World>) {
        let fly_towards = self.position();
        let particle_count = rand::random_range(1..=3);

        for _ in 0..particle_count {
            let fly_from = DVec3::new(
                self.position().x + PARTICLE_OFFSET_RADIUS * (gaussian() - gaussian()),
                self.position().y + PARTICLE_OFFSET_RADIUS * (gaussian() - gaussian()),
                self.position().z + PARTICLE_OFFSET_RADIUS * (gaussian() - gaussian()),
            );
            let direction = fly_from - fly_towards;
            world.send_particles(
                ParticleData::simple(&OMINOUS_SPAWNING),
                fly_towards,
                0,
                direction,
                1.0,
            );
        }
    }
}

/// Vanilla `Random.nextGaussian` (Marsaglia polar method), matching
/// `steel_utils::random::gaussian::MarsagliaPolarGaussian` for the thread-local RNG.
fn gaussian() -> f64 {
    loop {
        let d = 2.0 * rand::random::<f64>() - 1.0;
        let e = 2.0 * rand::random::<f64>() - 1.0;
        let f = d * d + e * e;

        if f < 1.0 && f != 0.0 {
            return d * (-2.0 * f.ln() / f).sqrt();
        }
    }
}

impl Entity for OminousItemSpawnerEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn tick(self: Arc<Self>) {
        // Vanilla `OminousItemSpawner.tick` runs `super.tick()` first, then the
        // server or client branch. Steel's base tick advances movement bookkeeping;
        // the particle burst is gated on game time the same way.
        self.base().advance_base_tick_state();

        let Some(world) = self.level() else {
            return;
        };

        self.tick_server(&world);

        // Vanilla runs the particle burst in the client tick; Steel drives client
        // visuals from the server, so it mirrors the same game-time cadence.
        if world.game_time() % 5 == 0 {
            self.add_particles(&world);
        }
    }

    fn is_pickable(&self) -> bool {
        false
    }

    fn attackable(&self) -> bool {
        false
    }

    fn movement_emission(&self) -> EntityMovementEmission {
        EntityMovementEmission::None
    }

    fn is_ignoring_block_triggers(&self) -> bool {
        true
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn hurt(&self, _world: &World, _source: &DamageSource, _amount: f32) -> bool {
        // Vanilla `OminousItemSpawner.hurtServer` is final and always ignores damage.
        false
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        // Match vanilla's `OminousItemSpawner.addAdditionalSaveData`.
        let item = self.item();
        if !item.is_empty() {
            nbt.insert("item", item.to_nbt_tag());
        }

        nbt.insert(
            "spawn_item_after_ticks",
            NbtTag::Long(self.spawn_item_after_ticks()),
        );
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        // Match vanilla's `OminousItemSpawner.readAdditionalSaveData`.
        if let Some(item_tag) = nbt.compound("item")
            && let Some(item) = ItemStack::from_borrowed_compound(&item_tag)
        {
            self.set_item(item);
        }
        self.set_spawn_item_after_ticks(nbt.long("spawn_item_after_ticks").unwrap_or(0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use simdnbt::borrow::read_compound;

    use steel_registry::{vanilla_entities, vanilla_items};

    #[test]
    fn ominous_item_spawner_defaults_to_no_item_and_zero_delay() {
        let spawner = OminousItemSpawnerEntity::new(
            &vanilla_entities::OMINOUS_ITEM_SPAWNER,
            1,
            DVec3::ZERO,
            Weak::new(),
        );

        assert!(spawner.item().is_empty());
        assert_eq!(spawner.spawn_item_after_ticks(), 0);
        assert!(spawner.no_physics());
        assert!(!spawner.is_pickable());
        assert!(spawner.is_ignoring_block_triggers());
    }

    #[test]
    fn ominous_item_spawner_create_uses_vanilla_delay_range() {
        let spawner = OminousItemSpawnerEntity::create(
            &vanilla_entities::OMINOUS_ITEM_SPAWNER,
            1,
            DVec3::ZERO,
            ItemStack::new(&vanilla_items::ARROW),
            Weak::new(),
        );

        let delay = spawner.spawn_item_after_ticks();
        assert!(
            (i64::from(SPAWN_ITEM_DELAY_MIN)..=i64::from(SPAWN_ITEM_DELAY_MAX)).contains(&delay)
        );
        assert!(!spawner.item().is_empty());
    }

    #[test]
    fn ominous_item_spawner_round_trips_saved_fields() {
        use std::io::Cursor;

        // Every registry-backed type is resolved at entity construction, so seed the
        // registry even when the filter excludes the tests that normally win the race.
        steel_registry::init_vanilla_registry();

        let spawner = OminousItemSpawnerEntity::new(
            &vanilla_entities::OMINOUS_ITEM_SPAWNER,
            1,
            DVec3::ZERO,
            Weak::new(),
        );
        spawner.set_item(ItemStack::new(&vanilla_items::ARROW));
        spawner.set_spawn_item_after_ticks(77);

        let mut nbt = NbtCompound::new();
        spawner.save_additional(&mut nbt);
        assert_eq!(nbt.long("spawn_item_after_ticks"), Some(77));
        assert!(nbt.compound("item").is_some());

        let loaded = OminousItemSpawnerEntity::new(
            &vanilla_entities::OMINOUS_ITEM_SPAWNER,
            2,
            DVec3::ZERO,
            Weak::new(),
        );
        let mut bytes = Vec::new();
        nbt.write(&mut bytes);
        let Ok(borrowed) = read_compound(&mut Cursor::new(bytes.as_slice())) else {
            panic!("saved ominous item spawner NBT should reborrow");
        };
        loaded.load_additional((&borrowed).into());

        assert_eq!(loaded.spawn_item_after_ticks(), 77);
        assert!(!loaded.item().is_empty());
    }

    #[test]
    fn ominous_item_spawner_omits_empty_item_from_saved_data() {
        let spawner = OminousItemSpawnerEntity::new(
            &vanilla_entities::OMINOUS_ITEM_SPAWNER,
            1,
            DVec3::ZERO,
            Weak::new(),
        );

        let mut nbt = NbtCompound::new();
        spawner.save_additional(&mut nbt);

        assert!(nbt.compound("item").is_none());
        assert_eq!(nbt.long("spawn_item_after_ticks"), Some(0));
    }
}
