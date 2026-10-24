//! Spawner minecart entity (`net.minecraft.world.entity.vehicle.minecart.MinecartSpawner`).
//!
//! Persists the full vanilla `BaseSpawner` field set (`Delay`, `MinSpawnDelay`,
//! `MaxSpawnDelay`, `SpawnCount`, `MaxNearbyEntities`, `RequiredPlayerRange`,
//! `SpawnRange`, `SpawnData`, `SpawnPotentials`) and decrements the delay while an alive
//! non-spectator player is inside `RequiredPlayerRange` of the cart's block centre,
//! exactly like `BaseSpawner.serverTick`.
//!
//! Vanilla `MinecartSpawner` does not override `BaseSpawner`'s delay or the default
//! `spawnDelay = 20`; the constants here are `BaseSpawner`'s own defaults.
//!
//! Deliberate difference: vanilla switches to the experimental
//! `NewMinecartBehavior` when `minecart_improvements` is enabled; Steel always
//! uses the stable `OldMinecartBehavior` physics from [`super::minecart_base`].
//!
//! Missing dependency: the spawn step itself. Steel models `BaseSpawner` as
//! [`crate::block_entity::entities::SpawnerBlockEntity`] (typed `SpawnData`/`SpawnPotentials`,
//! `spawn_attempt`, weighted `delay`), but that logic is a method on the block entity and needs
//! its own base, position and typed state, so a cart cannot drive it; `SpawnData`/`SpawnPotentials`
//! are round-tripped verbatim instead.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_macros::entity_behavior;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::items::ItemRef;
use steel_registry::vanilla_entity_data::{AbstractMinecartEntityData, SpawnerMinecartEntityData};
use steel_registry::vanilla_game_rules::SPAWNER_BLOCKS_WORK;
use steel_registry::{vanilla_blocks, vanilla_items};
use steel_utils::axis::Axis;
use steel_utils::block_util::FoundRectangle;
use steel_utils::locks::SyncMutex;
use steel_utils::nbt::NbtNumeric as _;
use steel_utils::{BlockStateId, DowncastType, DowncastTypeKey};

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

        // Vanilla `BaseSpawner.serverTick` (used by `MinecartSpawner`): no attempts while
        // the `spawner_blocks_work` game rule is disabled.
        if !world.get_game_rule(&SPAWNER_BLOCKS_WORK) {
            return;
        }

        // Vanilla `BaseSpawner.isNearPlayer` / `Level.hasNearbyAlivePlayer`: an alive,
        // non-spectator player within `RequiredPlayerRange` of the spawner's block centre.
        let pos = self.block_position();
        let near_player = world
            .nearest_player(
                DVec3::new(
                    f64::from(pos.x()) + 0.5,
                    f64::from(pos.y()) + 0.5,
                    f64::from(pos.z()) + 0.5,
                ),
                f64::from(required_range),
                |player| !player.is_spectator() && player.is_alive(),
            )
            .is_some();
        if !near_player {
            return;
        }

        if delay == -1 {
            self.delay();
        }

        let delay = self.spawn_delay();
        if delay > 0 {
            self.set_spawn_delay(delay - 1);
        } else {
            // MISSING API: the vanilla spawn step builds an entity from
            // `SpawnData`/`SpawnPotentials`, spawns up to `SpawnCount` of them and re-rolls the
            // delay when one was placed. Steel's equivalent lives on
            // `crate::block_entity::entities::SpawnerBlockEntity` (`spawn_attempt`/`delay`,
            // which need the block entity's own base, position and typed `SpawnData`), so it
            // cannot be reached from a cart without refactoring that block entity. The delay is
            // re-rolled here, which is what vanilla does for a cart whose `SpawnData` carries no
            // entity id.
            self.delay();
        }
    }

    fn push_entity(&self, entity: &dyn Entity) {
        // Vanilla `AbstractMinecart.push`.
        self.minecart_push(entity);
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
        state.spawn_delay = read_int_or(nbt, "Delay", DEFAULT_SPAWN_DELAY);
        state.min_spawn_delay = read_int_or(nbt, "MinSpawnDelay", DEFAULT_MIN_SPAWN_DELAY);
        state.max_spawn_delay = read_int_or(nbt, "MaxSpawnDelay", DEFAULT_MAX_SPAWN_DELAY);
        state.spawn_count = read_int_or(nbt, "SpawnCount", DEFAULT_SPAWN_COUNT);
        state.max_nearby_entities =
            read_int_or(nbt, "MaxNearbyEntities", DEFAULT_MAX_NEARBY_ENTITIES);
        state.required_player_range =
            read_int_or(nbt, "RequiredPlayerRange", DEFAULT_REQUIRED_PLAYER_RANGE);
        state.spawn_range = read_int_or(nbt, "SpawnRange", DEFAULT_SPAWN_RANGE);
        state.spawn_data = nbt.compound("SpawnData").map(|data| data.to_owned());
        state.spawn_potentials = nbt.list("SpawnPotentials").map(|list| list.to_owned());
    }
}

/// Vanilla `ValueInput.getShortOr`/`getIntOr`: any numeric tag is accepted, so an `Int`
/// written by `/data merge` reads back exactly like the `Short` vanilla itself saves.
fn read_int_or(nbt: BorrowedNbtCompoundView<'_, '_>, key: &str, default: i32) -> i32 {
    nbt.get(key)
        .and_then(|tag| tag.codec_i32())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::Weak;

    use simdnbt::borrow::read_compound;
    use simdnbt::owned::{NbtCompound, NbtList};
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::*;

    fn spawner_cart() -> SpawnerMinecartEntity {
        SpawnerMinecartEntity::new(
            &vanilla_entities::SPAWNER_MINECART,
            1,
            DVec3::new(8.5, 64.0, 8.5),
            Weak::new(),
        )
    }

    /// Re-parses saved NBT and hands the borrowed view to `load_additional`, mirroring the
    /// entity reload path.
    fn reload(nbt: &NbtCompound, load: impl FnOnce(BorrowedNbtCompoundView<'_, '_>)) {
        let mut bytes = Vec::new();
        nbt.write(&mut bytes);
        let borrowed = read_compound(&mut Cursor::new(&bytes)).expect("test NBT should reborrow");
        load((&borrowed).into());
    }

    #[test]
    fn delay_defaults_and_inverted_ranges_match_base_spawner() {
        init_vanilla_registry();
        let cart = spawner_cart();

        // Vanilla `MinecartSpawner` uses a plain `BaseSpawner`: `spawnDelay = 20`,
        // `minSpawnDelay = 200`, `maxSpawnDelay = 800`.
        assert_eq!(cart.spawn_delay(), DEFAULT_SPAWN_DELAY);
        assert_eq!(
            spawner_cart().spawn_data(),
            None,
            "a fresh cart carries no SpawnData"
        );

        for _ in 0..32 {
            cart.delay();
            let delay = cart.spawn_delay();
            assert!(
                (DEFAULT_MIN_SPAWN_DELAY..DEFAULT_MAX_SPAWN_DELAY).contains(&delay),
                "vanilla rolls `min + random.nextInt(max - min)`"
            );
        }

        // Vanilla `BaseSpawner.delay`: `maxSpawnDelay <= minSpawnDelay` pins the minimum.
        {
            let mut state = cart.state.lock();
            state.min_spawn_delay = 300;
            state.max_spawn_delay = 300;
        }
        cart.delay();
        assert_eq!(cart.spawn_delay(), 300);
    }

    #[test]
    fn nbt_load_accepts_short_and_int_vanilla_style_fields() {
        init_vanilla_registry();
        let mut nbt = NbtCompound::new();
        // Vanilla saves these as shorts but reads them through `Codec.INT`, so an int written by
        // `/data merge` must read back too.
        nbt.insert("Delay", 7i16);
        nbt.insert("MinSpawnDelay", 300i32);
        nbt.insert("SpawnCount", 9i16);

        let cart = spawner_cart();
        reload(&nbt, |view| cart.load_additional(view));

        assert_eq!(cart.spawn_delay(), 7);
        assert_eq!(cart.state.lock().min_spawn_delay, 300);
        assert_eq!(cart.state.lock().spawn_count, 9);
        // Absent keys keep the `BaseSpawner` defaults.
        assert_eq!(cart.state.lock().max_spawn_delay, DEFAULT_MAX_SPAWN_DELAY);
        assert_eq!(cart.state.lock().spawn_range, DEFAULT_SPAWN_RANGE);
    }

    #[test]
    fn base_spawner_fields_and_potentials_round_trip() {
        init_vanilla_registry();
        let source = spawner_cart();
        {
            let mut state = source.state.lock();
            state.spawn_delay = -1;
            state.min_spawn_delay = 40;
            state.max_spawn_delay = 60;
            state.spawn_count = 2;
            state.max_nearby_entities = 3;
            state.required_player_range = 12;
            state.spawn_range = 5;
            let mut spawn_data = NbtCompound::new();
            spawn_data.insert("id", "minecraft:pig");
            state.spawn_data = Some(spawn_data);
            state.spawn_potentials = Some(NbtList::Compound(vec![NbtCompound::new()]));
        }

        let mut nbt = NbtCompound::new();
        source.save_additional(&mut nbt);

        let loaded = spawner_cart();
        reload(&nbt, |view| loaded.load_additional(view));

        assert_eq!(loaded.spawn_delay(), -1);
        {
            let state = loaded.state.lock();
            assert_eq!(state.min_spawn_delay, 40);
            assert_eq!(state.max_spawn_delay, 60);
            assert_eq!(state.spawn_count, 2);
            assert_eq!(state.max_nearby_entities, 3);
            assert_eq!(state.required_player_range, 12);
            assert_eq!(state.spawn_range, 5);
        }
        assert_eq!(
            loaded
                .spawn_data()
                .and_then(|data| data.string("id").map(ToString::to_string)),
            Some("minecraft:pig".to_owned())
        );
        assert!(loaded.state.lock().spawn_potentials.is_some());
    }
}
