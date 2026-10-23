//! Spawner minecart entity (`net.minecraft.world.entity.vehicle.minecart.MinecartSpawner`).
//!
//! Persists the full vanilla `BaseSpawner` field set (`Delay`, `MinSpawnDelay`,
//! `MaxSpawnDelay`, `SpawnCount`, `MaxNearbyEntities`, `RequiredPlayerRange`,
//! `SpawnRange`, `SpawnData`, `SpawnPotentials`) and decrements the delay while a
//! player is inside `RequiredPlayerRange`.
//!
//! Deliberate difference: vanilla switches to the experimental
//! `NewMinecartBehavior` when `minecart_improvements` is enabled; Steel always
//! uses the stable `OldMinecartBehavior` physics from [`super::minecart_base`].
//!
//! Missing dependency: Steel has no `BaseSpawner` equivalent, so the actual
//! spawn step (`BaseSpawner.serverTick` entity creation, `setNextSpawnData`
//! weighted-potential selection, and the spin/particle display) is not performed
//! here; `SpawnData`/`SpawnPotentials` are round-tripped verbatim instead. See
//! `steel-core/src/block_entity/entities/` for the planned spawner block entity.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_macros::entity_behavior;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::items::ItemRef;
use steel_registry::vanilla_entity_data::{AbstractMinecartEntityData, SpawnerMinecartEntityData};
use steel_registry::{vanilla_blocks, vanilla_items};
use steel_utils::axis::Axis;
use steel_utils::block_util::FoundRectangle;
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::entity::damage::DamageSource;
use crate::entity::{Entity, EntityBase, EntityBaseLoad, EntitySyncedData};
use crate::world::World;

use super::minecart_base::{self, Minecart, MinecartState};

/// Vanilla `BaseSpawner` default spawn delay.
const DEFAULT_SPAWN_DELAY: i32 = 20;
/// Vanilla `BaseSpawner` default minimum delay.
const DEFAULT_MIN_SPAWN_DELAY: i32 = 200;
/// Vanilla `BaseSpawner` default maximum delay.
const DEFAULT_MAX_SPAWN_DELAY: i32 = 800;
/// Vanilla `BaseSpawner` default spawn count.
const DEFAULT_SPAWN_COUNT: i32 = 4;
/// Vanilla `BaseSpawner` default nearby entity cap.
const DEFAULT_MAX_NEARBY_ENTITIES: i32 = 6;
/// Vanilla `BaseSpawner` default required player range.
const DEFAULT_REQUIRED_PLAYER_RANGE: i32 = 16;
/// Vanilla `BaseSpawner` default spawn range.
const DEFAULT_SPAWN_RANGE: i32 = 4;

/// The persisted `BaseSpawner` state carried by a spawner minecart.
#[derive(Debug, Clone, PartialEq)]
struct SpawnerMinecartState {
    /// Vanilla `BaseSpawner.spawnDelay`.
    spawn_delay: i32,
    /// Vanilla `BaseSpawner.minSpawnDelay`.
    min_spawn_delay: i32,
    /// Vanilla `BaseSpawner.maxSpawnDelay`.
    max_spawn_delay: i32,
    /// Vanilla `BaseSpawner.spawnCount`.
    spawn_count: i32,
    /// Vanilla `BaseSpawner.maxNearbyEntities`.
    max_nearby_entities: i32,
    /// Vanilla `BaseSpawner.requiredPlayerRange`.
    required_player_range: i32,
    /// Vanilla `BaseSpawner.spawnRange`.
    spawn_range: i32,
    /// Vanilla `BaseSpawner.nextSpawnData.getEntityToSpawn()`, kept verbatim.
    spawn_data: Option<NbtCompound>,
    /// Vanilla `BaseSpawner.spawnPotentials`, kept verbatim.
    spawn_potentials: Option<NbtList>,
}

impl SpawnerMinecartState {
    const fn new() -> Self {
        Self {
            spawn_delay: DEFAULT_SPAWN_DELAY,
            min_spawn_delay: DEFAULT_MIN_SPAWN_DELAY,
            max_spawn_delay: DEFAULT_MAX_SPAWN_DELAY,
            spawn_count: DEFAULT_SPAWN_COUNT,
            max_nearby_entities: DEFAULT_MAX_NEARBY_ENTITIES,
            required_player_range: DEFAULT_REQUIRED_PLAYER_RANGE,
            spawn_range: DEFAULT_SPAWN_RANGE,
            spawn_data: None,
            spawn_potentials: None,
        }
    }
}

/// A spawner minecart.
#[entity_behavior(class = "MinecartSpawner")]
pub struct SpawnerMinecartEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<SpawnerMinecartEntityData>,
    state: SyncMutex<SpawnerMinecartState>,
    cart: SyncMutex<MinecartState>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `SpawnerMinecartEntity`.
unsafe impl DowncastType for SpawnerMinecartEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/spawner_minecart");
}

impl SpawnerMinecartEntity {
    /// Creates a new spawner minecart entity.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(SpawnerMinecartEntityData::new()),
            state: SyncMutex::new(SpawnerMinecartState::new()),
            cart: SyncMutex::new(MinecartState::new()),
        }
    }

    /// Creates a spawner minecart entity from saved data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(SpawnerMinecartEntityData::new()),
            state: SyncMutex::new(SpawnerMinecartState::new()),
            cart: SyncMutex::new(MinecartState::new()),
        }
    }

    /// Current spawn delay, vanilla `spawnDelay`.
    #[must_use]
    pub fn spawn_delay(&self) -> i32 {
        self.state.lock().spawn_delay
    }

    /// Sets the spawn delay, vanilla `spawnDelay` assignment.
    pub fn set_spawn_delay(&self, delay: i32) {
        self.state.lock().spawn_delay = delay;
    }

    /// Vanilla `BaseSpawner.delay`: pick a new random delay in range.
    pub fn delay(&self) {
        let mut state = self.state.lock();
        let min = state.min_spawn_delay;
        let max = state.max_spawn_delay;
        state.spawn_delay = if max <= min {
            min
        } else {
            rand::random_range(min..max)
        };
    }

    /// The verbatim `SpawnData` compound, as vanilla stores it.
    #[must_use]
    pub fn spawn_data(&self) -> Option<NbtCompound> {
        self.state.lock().spawn_data.clone()
    }

    /// Replaces the `SpawnData` compound.
    pub fn set_spawn_data(&self, spawn_data: Option<NbtCompound>) {
        self.state.lock().spawn_data = spawn_data;
    }
}

impl Minecart for SpawnerMinecartEntity {
    fn cart_state(&self) -> &SyncMutex<MinecartState> {
        &self.cart
    }

    fn with_minecart_data<R>(&self, f: impl FnOnce(&AbstractMinecartEntityData) -> R) -> R {
        f(self.entity_data.lock().abstract_minecart())
    }

    fn with_minecart_data_mut<R>(&self, f: impl FnOnce(&mut AbstractMinecartEntityData) -> R) -> R {
        f(self.entity_data.lock().abstract_minecart_mut())
    }

    fn default_display_block_state(&self) -> BlockStateId {
        vanilla_blocks::SPAWNER.default_state()
    }

    fn drop_item(&self) -> Option<ItemRef> {
        Some(&vanilla_items::MINECART)
    }
}

impl Entity for SpawnerMinecartEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn tick(self: Arc<Self>) {
        Arc::clone(&self).minecart_tick();

        let (delay, required_range) = {
            let state = self.state.lock();
            (state.spawn_delay, state.required_player_range)
        };
        let Some(world) = self.level() else {
            return;
        };

        // Vanilla `BaseSpawner.serverTick` (used by `MinecartSpawner`): no
        // attempts while the `spawner_blocks_work` game rule is disabled.
        if !world.get_game_rule(&steel_registry::vanilla_game_rules::SPAWNER_BLOCKS_WORK) {
            return;
        }

        // Vanilla `BaseSpawner.serverTick` distance gate.
        if world
            .nearest_player(self.position(), f64::from(required_range), |_| true)
            .is_none()
        {
            return;
        }

        if delay == -1 {
            self.delay();
        }

        let delay = self.spawn_delay();
        if delay > 0 {
            self.set_spawn_delay(delay - 1);
        } else {
            // MISSING API: the vanilla spawn step creates an entity from
            // `SpawnData`/`SpawnPotentials` and re-rolls the delay. Steel has no
            // `BaseSpawner`; the delay is re-rolled so the timer stays vanilla-shaped.
            self.delay();
        }
    }

    fn is_pickable(&self) -> bool {
        !self.is_removed()
    }

    fn is_pushable(&self) -> bool {
        true
    }

    fn blocks_building(&self) -> bool {
        true
    }

    fn is_on_rails(&self) -> bool {
        self.is_on_minecart_rails()
    }

    fn get_default_gravity(&self) -> f64 {
        self.minecart_default_gravity()
    }

    fn get_air_drag(&self) -> f64 {
        minecart_base::AIR_DRAG
    }

    fn dimension_changing_delay(&self) -> i32 {
        10
    }

    fn get_relative_portal_position(&self, axis: Axis, portal_area: FoundRectangle) -> DVec3 {
        minecart_base::minecart_relative_portal_position(self, axis, portal_area)
    }

    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        self.minecart_hurt(world, source, amount)
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_minecart_data(nbt);
        let state = self.state.lock();
        nbt.insert("Delay", state.spawn_delay as i16);
        nbt.insert("MinSpawnDelay", state.min_spawn_delay as i16);
        nbt.insert("MaxSpawnDelay", state.max_spawn_delay as i16);
        nbt.insert("SpawnCount", state.spawn_count as i16);
        nbt.insert("MaxNearbyEntities", state.max_nearby_entities as i16);
        nbt.insert("RequiredPlayerRange", state.required_player_range as i16);
        nbt.insert("SpawnRange", state.spawn_range as i16);
        if let Some(spawn_data) = &state.spawn_data {
            nbt.insert("SpawnData", NbtTag::Compound(spawn_data.clone()));
        }
        if let Some(potentials) = &state.spawn_potentials {
            nbt.insert("SpawnPotentials", potentials.clone());
        }
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_minecart_data(nbt);
        let mut state = self.state.lock();
        state.spawn_delay = i32::from(nbt.short("Delay").unwrap_or(DEFAULT_SPAWN_DELAY as i16));
        state.min_spawn_delay = i32::from(
            nbt.short("MinSpawnDelay")
                .unwrap_or(DEFAULT_MIN_SPAWN_DELAY as i16),
        );
        state.max_spawn_delay = i32::from(
            nbt.short("MaxSpawnDelay")
                .unwrap_or(DEFAULT_MAX_SPAWN_DELAY as i16),
        );
        state.spawn_count = i32::from(
            nbt.short("SpawnCount")
                .unwrap_or(DEFAULT_SPAWN_COUNT as i16),
        );
        state.max_nearby_entities = i32::from(
            nbt.short("MaxNearbyEntities")
                .unwrap_or(DEFAULT_MAX_NEARBY_ENTITIES as i16),
        );
        state.required_player_range = i32::from(
            nbt.short("RequiredPlayerRange")
                .unwrap_or(DEFAULT_REQUIRED_PLAYER_RANGE as i16),
        );
        state.spawn_range = i32::from(
            nbt.short("SpawnRange")
                .unwrap_or(DEFAULT_SPAWN_RANGE as i16),
        );
        state.spawn_data = nbt.compound("SpawnData").map(|data| data.to_owned());
        state.spawn_potentials = nbt.list("SpawnPotentials").map(|list| list.to_owned());
    }
}
