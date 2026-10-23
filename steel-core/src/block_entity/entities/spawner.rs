//! `SpawnerBlockEntity` (`net.minecraft.world.level.block.entity.SpawnerBlockEntity`).
//!
//! Mirrors vanilla's `BaseSpawner` state: the next `SpawnData`, its `SpawnPotentials` table, the
//! delay between attempts, the spawn count/range and the player proximity requirements. The
//! server tick advances the delay and delegates the actual attempt to
//! [`crate::world::spawner`]-style helpers once the entity factory lane provides them.

use std::sync::Weak;

use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::{NbtCompound, NbtList};
use steel_registry::vanilla_block_entity_types;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, locks::SyncMutex};

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::world::World;

/// Vanilla `BaseSpawner.DEFAULT_MIN_SPAWN_DELAY`.
pub const SPAWNER_MIN_SPAWN_DELAY: i32 = 200;
/// Vanilla `BaseSpawner.DEFAULT_MAX_SPAWN_DELAY`.
pub const SPAWNER_MAX_SPAWN_DELAY: i32 = 800;
/// Vanilla `BaseSpawner.DEFAULT_SPAWN_COUNT`.
pub const SPAWNER_SPAWN_COUNT: i32 = 4;
/// Vanilla `BaseSpawner.DEFAULT_MAX_NEARBY_ENTITIES`.
pub const SPAWNER_MAX_NEARBY_ENTITIES: i32 = 6;
/// Vanilla `BaseSpawner.DEFAULT_SPAWN_RANGE`.
pub const SPAWNER_SPAWN_RANGE: i32 = 4;
/// Vanilla `BaseSpawner.DEFAULT_REQUIRED_PLAYER_RANGE`.
pub const SPAWNER_REQUIRED_PLAYER_RANGE: i32 = 16;

/// One entry of the `SpawnPotentials` weighted list.
#[derive(Clone, Debug)]
pub struct SpawnPotential {
    /// Weight of this entry in the weighted selection.
    pub weight: i32,
    /// Raw entity NBT for this entry.
    pub data: NbtCompound,
}

/// Vanilla `BaseSpawner` state.
#[derive(Clone, Debug)]
pub struct SpawnerData {
    /// Delay in ticks until the next spawn attempt.
    pub delay: i32,
    /// Minimum delay sampled after a successful attempt.
    pub min_spawn_delay: i32,
    /// Maximum delay sampled after a successful attempt.
    pub max_spawn_delay: i32,
    /// Entities spawned per attempt.
    pub spawn_count: i32,
    /// Nearby-entity cap for the spawned type.
    pub max_nearby_entities: i32,
    /// Horizontal/vertical spawn offset range.
    pub spawn_range: i32,
    /// Player distance that keeps the spawner running.
    pub required_player_range: i32,
    /// The next entity to spawn.
    pub spawn_data: NbtCompound,
    /// Weighted alternatives for the next spawn.
    pub spawn_potentials: Vec<SpawnPotential>,
}

impl Default for SpawnerData {
    fn default() -> Self {
        Self {
            delay: 20,
            min_spawn_delay: SPAWNER_MIN_SPAWN_DELAY,
            max_spawn_delay: SPAWNER_MAX_SPAWN_DELAY,
            spawn_count: SPAWNER_SPAWN_COUNT,
            max_nearby_entities: SPAWNER_MAX_NEARBY_ENTITIES,
            spawn_range: SPAWNER_SPAWN_RANGE,
            required_player_range: SPAWNER_REQUIRED_PLAYER_RANGE,
            spawn_data: NbtCompound::new(),
            spawn_potentials: Vec::new(),
        }
    }
}

/// `SpawnerBlockEntity` (`mob_spawner`).
pub struct SpawnerBlockEntity {
    base: BlockEntityBase,
    state: SyncMutex<SpawnerData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `SpawnerBlockEntity`.
unsafe impl DowncastType for SpawnerBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/spawner");
}

impl SpawnerBlockEntity {
    /// Creates a mob-spawner block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self {
            base: BlockEntityBase::new(&vanilla_block_entity_types::MOB_SPAWNER, level, pos, state),
            state: SyncMutex::new(SpawnerData::default()),
        }
    }

    /// Returns a snapshot of the spawner configuration.
    #[must_use]
    pub fn data(&self) -> SpawnerData {
        self.state.lock().clone()
    }

    /// Vanilla `BaseSpawner.setNextSpawnData`: replaces the next spawn and clears the potentials
    /// when no weighted list is supplied.
    pub fn set_next_spawn_data(&self, spawn_data: NbtCompound, potentials: Vec<SpawnPotential>) {
        let mut state = self.state.lock();
        state.spawn_data = spawn_data;
        state.spawn_potentials = potentials;
        drop(state);
        BlockEntity::set_changed(self);
    }

    /// Vanilla `BaseSpawner.setEntityId` (spawn egg / `/setblock` data): sets the entity id and
    /// clears the potentials.
    pub fn set_entity_id(&self, entity_id: &str) {
        let mut data = NbtCompound::new();
        data.insert("id", entity_id);
        let mut state = self.state.lock();
        state.spawn_data = data;
        state.spawn_potentials.clear();
        drop(state);
        BlockEntity::set_changed(self);
    }
}

impl BlockEntity for SpawnerBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let view: NbtCompoundView<'_, '_> = nbt.into();
        let mut state = self.state.lock();

        state.delay = view.short("Delay").map_or(20, i32::from);
        state.min_spawn_delay = view
            .short("MinSpawnDelay")
            .map_or(SPAWNER_MIN_SPAWN_DELAY, i32::from);
        state.max_spawn_delay = view
            .short("MaxSpawnDelay")
            .map_or(SPAWNER_MAX_SPAWN_DELAY, i32::from);
        state.spawn_count = view
            .short("SpawnCount")
            .map_or(SPAWNER_SPAWN_COUNT, i32::from);
        state.max_nearby_entities = view
            .short("MaxNearbyEntities")
            .map_or(SPAWNER_MAX_NEARBY_ENTITIES, i32::from);
        state.spawn_range = view
            .short("SpawnRange")
            .map_or(SPAWNER_SPAWN_RANGE, i32::from);
        state.required_player_range = view
            .short("RequiredPlayerRange")
            .map_or(SPAWNER_REQUIRED_PLAYER_RANGE, i32::from);

        // `SpawnData` is a compound holding the entity NBT directly.
        state.spawn_data = view
            .compound("SpawnData")
            .map(|compound| compound.to_owned())
            .unwrap_or_default();

        state.spawn_potentials.clear();
        if let Some(potentials) = view.list("SpawnPotentials")
            && let Some(entries) = potentials.compounds()
        {
            for compound in entries {
                let weight = compound.int("weight").unwrap_or(1);
                let data = compound
                    .compound("data")
                    .map(|data| data.to_owned())
                    .unwrap_or_default();
                state.spawn_potentials.push(SpawnPotential { weight, data });
            }
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let state = self.state.lock();
        nbt.insert("Delay", state.delay as i16);
        nbt.insert("MinSpawnDelay", state.min_spawn_delay as i16);
        nbt.insert("MaxSpawnDelay", state.max_spawn_delay as i16);
        nbt.insert("SpawnCount", state.spawn_count as i16);
        nbt.insert("MaxNearbyEntities", state.max_nearby_entities as i16);
        nbt.insert("SpawnRange", state.spawn_range as i16);
        nbt.insert("RequiredPlayerRange", state.required_player_range as i16);
        nbt.insert("SpawnData", state.spawn_data.clone());

        let potentials: Vec<NbtCompound> = state
            .spawn_potentials
            .iter()
            .map(|potential| {
                let mut entry = NbtCompound::new();
                entry.insert("weight", potential.weight);
                entry.insert("data", potential.data.clone());
                entry
            })
            .collect();
        nbt.insert("SpawnPotentials", NbtList::Compound(potentials));
    }

    fn get_update_tag(&self) -> Option<NbtCompound> {
        // Vanilla sends only `SpawnData` to clients.
        let state = self.state.lock();
        let mut nbt = NbtCompound::new();
        nbt.insert("SpawnData", state.spawn_data.clone());
        Some(nbt)
    }

    fn tick(&self, world: &std::sync::Arc<World>) {
        let pos = self.get_block_pos();
        let (required_range, delay) = {
            let mut state = self.state.lock();
            if state.delay > 0 {
                state.delay -= 1;
                return;
            }
            (state.required_player_range, state.delay)
        };
        let _ = delay;

        let range_sq = f64::from(required_range) * f64::from(required_range);
        let has_player = world
            .nearest_player_distance_sqr(glam::DVec3::new(
                f64::from(pos.x()) + 0.5,
                f64::from(pos.y()) + 0.5,
                f64::from(pos.z()) + 0.5,
            ))
            .is_some_and(|distance| distance <= range_sq);

        let mut state = self.state.lock();
        if !has_player {
            // Vanilla resets the delay to 20 ticks while no player is in range.
            state.delay = 20;
            return;
        }

        // TODO(Entities): perform the vanilla spawn attempt (`BaseSpawner.serverTick`): pick a
        // weighted `SpawnPotentials` entry, check `max_nearby_entities`, and spawn
        // `spawn_count` entities via the entity factory. Blocked on the entity lane exposing a
        // `EntityTypeRef` → NBT-spawn bridge (vanilla `EntityType.create(CompoundTag)`).
        let next_delay = rand::random_range(state.min_spawn_delay..=state.max_spawn_delay);
        state.delay = next_delay;
    }
}
