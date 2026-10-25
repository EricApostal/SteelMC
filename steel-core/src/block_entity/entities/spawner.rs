//! `SpawnerBlockEntity` (`net.minecraft.world.level.block.entity.SpawnerBlockEntity`).
//!
//! Mirrors vanilla's `BaseSpawner` state: the next `SpawnData`, its `SpawnPotentials` weighted
//! list, the delay between attempts, the spawn count/range and the player proximity
//! requirements. The server tick advances the delay and runs the vanilla spawn attempt.

use std::ptr;
use std::str::FromStr;
use std::sync::{Arc, Weak};

use glam::DVec3;
use rand::RngExt as _;
use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::{NbtCompound, NbtList};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::vanilla_game_rules;
use steel_registry::{REGISTRY, RegistryExt as _, vanilla_block_entity_types, vanilla_game_events};
use steel_utils::entity_events::EntityStatus;
use steel_utils::nbt::NbtNumeric as _;
use steel_utils::random::{RandomSource, legacy_random::LegacyRandom};
use steel_utils::types::Difficulty;
use steel_utils::{
    BlockPos, BlockStateId, DowncastType, DowncastTypeKey, Identifier, WorldAabb, locks::SyncMutex,
};

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::chunk::light::LightLayer;
use crate::entity::{Entity as _, EntitySpawnReason, create_entity_instance};
use crate::world::World;
use crate::world::game_event::GameEventContext;
use crate::world::spawn::SpawnPlacements;

/// Vanilla `BaseSpawner.DEFAULT_SPAWN_DELAY`.
pub const SPAWNER_DEFAULT_SPAWN_DELAY: i32 = 20;
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
/// Vanilla `BaseSpawner.EVENT_SPAWN` (block event that re-rolls the delay on clients).
pub const SPAWNER_EVENT_SPAWN: i32 = 1;
/// Vanilla `LevelEvent.PARTICLES_MOBSPAWNER_SPAWN` (`levelEvent(2004, pos, 0)`).
pub const SPAWNER_LEVEL_EVENT_SPAWN: i32 = 2004;

/// Vanilla `SpawnData.CustomSpawnRules`: inclusive light limits that gate a spawn position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CustomSpawnRules {
    /// Inclusive block-light range (`block_light_limit`).
    pub block_light_limit: (i32, i32),
    /// Inclusive sky-light range (`sky_light_limit`).
    pub sky_light_limit: (i32, i32),
}

impl CustomSpawnRules {
    /// Vanilla `CustomSpawnRules.LIGHT_RANGE`, the default for both limits.
    pub const LIGHT_RANGE: (i32, i32) = (0, 15);

    /// Vanilla `SpawnData.CustomSpawnRules.isValidPosition`.
    #[must_use]
    pub fn is_valid_position(self, world: &World, pos: BlockPos) -> bool {
        Self::contains(
            self.block_light_limit,
            world.light_value_at(LightLayer::Block, pos),
        ) && Self::contains(self.sky_light_limit, world.effective_sky_brightness(pos))
    }

    fn contains((min, max): (i32, i32), value: u8) -> bool {
        i32::from(value) >= min && i32::from(value) <= max
    }

    /// Reads one `InclusiveRange<Integer>` field, defaulting to `0..=15` like vanilla's lenient
    /// `lightLimit` field.
    fn read(view: &NbtCompoundView<'_, '_>, name: &str) -> (i32, i32) {
        let Some(range) = view.compound(name) else {
            return Self::LIGHT_RANGE;
        };
        let min = range
            .get("min_inclusive")
            .and_then(|tag| tag.codec_i32())
            .unwrap_or(Self::LIGHT_RANGE.0);
        let max = range
            .get("max_inclusive")
            .and_then(|tag| tag.codec_i32())
            .unwrap_or(Self::LIGHT_RANGE.1);
        (min, max)
    }

    fn write_range(nbt: &mut NbtCompound, name: &str, (min, max): (i32, i32)) {
        let mut range = NbtCompound::new();
        range.insert("min_inclusive", min);
        range.insert("max_inclusive", max);
        nbt.insert(name, range);
    }
}

/// Vanilla `SpawnData`: the entity to spawn plus its optional spawn rules and equipment.
#[derive(Clone, Debug, Default)]
pub struct SpawnData {
    /// Vanilla `SpawnData.entityToSpawn` (the `entity` compound).
    pub entity_to_spawn: NbtCompound,
    /// Vanilla `SpawnData.customSpawnRules` (`custom_spawn_rules`).
    pub custom_spawn_rules: Option<CustomSpawnRules>,
    /// Vanilla `SpawnData.equipment` (`equipment`), kept verbatim for NBT round-trips.
    pub equipment: Option<NbtCompound>,
}

impl SpawnData {
    /// Vanilla `SpawnData.CODEC` decoding: requires the `entity` field.
    pub(super) fn from_view(view: &NbtCompoundView<'_, '_>) -> Option<Self> {
        let mut entity_to_spawn = view.compound("entity")?.to_owned();
        normalize_entity_id(&mut entity_to_spawn);
        let custom_spawn_rules =
            view.compound("custom_spawn_rules")
                .map(|rules| CustomSpawnRules {
                    block_light_limit: CustomSpawnRules::read(&rules, "block_light_limit"),
                    sky_light_limit: CustomSpawnRules::read(&rules, "sky_light_limit"),
                });
        Some(Self {
            entity_to_spawn,
            custom_spawn_rules,
            equipment: view.compound("equipment").map(|data| data.to_owned()),
        })
    }

    /// Builds `SpawnData` from an entity compound, as `setEntityData`/`setEntityId` do.
    #[must_use]
    pub fn from_entity(entity: NbtCompound) -> Self {
        let mut entity_to_spawn = entity;
        normalize_entity_id(&mut entity_to_spawn);
        Self {
            entity_to_spawn,
            custom_spawn_rules: None,
            equipment: None,
        }
    }

    /// Vanilla `SpawnData.CODEC` encoding.
    pub(super) fn write(&self, nbt: &mut NbtCompound) {
        nbt.insert("entity", self.entity_to_spawn.clone());
        if let Some(rules) = self.custom_spawn_rules {
            let mut compound = NbtCompound::new();
            CustomSpawnRules::write_range(
                &mut compound,
                "block_light_limit",
                rules.block_light_limit,
            );
            CustomSpawnRules::write_range(&mut compound, "sky_light_limit", rules.sky_light_limit);
            nbt.insert("custom_spawn_rules", compound);
        }
        if let Some(equipment) = &self.equipment {
            nbt.insert("equipment", equipment.clone());
        }
    }

    /// Returns the entity type referenced by the `id` string, if it resolves.
    #[must_use]
    pub fn entity_type(&self) -> Option<EntityTypeRef> {
        let id = self.entity_to_spawn.string("id")?;
        let id = Identifier::from_str(id.to_str().as_ref()).ok()?;
        REGISTRY.entity_types.by_key(&id)
    }
}

/// Vanilla `SpawnData`'s compact constructor: canonicalise the `id` string, dropping it when it
/// does not parse as an identifier.
fn normalize_entity_id(entity: &mut NbtCompound) {
    let Some(id) = entity.string("id").map(|id| id.to_str().into_owned()) else {
        return;
    };
    match Identifier::from_str(&id) {
        Ok(identifier) => {
            entity.insert("id", identifier.to_string());
        }
        Err(_) => {
            entity.remove("id");
        }
    }
}

/// One entry of the `SpawnPotentials` weighted list (`WeightedList.Weighted<SpawnData>`).
#[derive(Clone, Debug)]
pub struct SpawnPotential {
    /// Weight of this entry in the weighted selection.
    pub weight: i32,
    /// The `SpawnData` value (`data` in vanilla's `Weighted.codec`).
    pub data: SpawnData,
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
    /// The next entity to spawn (vanilla's nullable `nextSpawnData`).
    pub next_spawn_data: Option<SpawnData>,
    /// Weighted alternatives for the next spawn.
    pub spawn_potentials: Vec<SpawnPotential>,
}

impl Default for SpawnerData {
    fn default() -> Self {
        Self {
            delay: SPAWNER_DEFAULT_SPAWN_DELAY,
            min_spawn_delay: SPAWNER_MIN_SPAWN_DELAY,
            max_spawn_delay: SPAWNER_MAX_SPAWN_DELAY,
            spawn_count: SPAWNER_SPAWN_COUNT,
            max_nearby_entities: SPAWNER_MAX_NEARBY_ENTITIES,
            spawn_range: SPAWNER_SPAWN_RANGE,
            required_player_range: SPAWNER_REQUIRED_PLAYER_RANGE,
            next_spawn_data: None,
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

    /// Vanilla `BaseSpawner.setNextSpawnData`: replaces the next spawn and, when a non-empty
    /// weighted list is supplied, the `SpawnPotentials` table.
    ///
    /// Vanilla's spawn-egg path (`SpawnerBlockEntity.setEntityData`) only replaces the next
    /// spawn data and keeps the existing potentials, so an empty list leaves them untouched.
    pub fn set_next_spawn_data(&self, entity_data: NbtCompound, potentials: Vec<SpawnPotential>) {
        let mut state = self.state.lock();
        state.next_spawn_data = Some(SpawnData::from_entity(entity_data));
        if !potentials.is_empty() {
            state.spawn_potentials = potentials;
        }
        drop(state);
        self.mark_updated();
    }

    /// Vanilla `BaseSpawner.setEntityId` (spawn egg / `/setblock` data): sets the entity id of
    /// the next spawn data, keeping the weighted list.
    pub fn set_entity_id(&self, entity_id: &str) {
        let mut state = self.state.lock();
        let spawn_data = state.next_spawn_data.get_or_insert_with(SpawnData::default);
        spawn_data.entity_to_spawn.insert("id", entity_id);
        drop(state);
        self.mark_updated();
    }

    /// Vanilla `SpawnerBlockEntity`'s `setNextSpawnData` override: `setChanged` plus
    /// `sendBlockUpdated(pos, state, state, 260)`.
    fn mark_updated(&self) {
        BlockEntity::set_changed(self);
        if let Some(world) = self.get_level() {
            world.send_block_updated(self.get_block_pos());
        }
    }

    /// Vanilla `BaseSpawner.isNearPlayer` / `Level.hasNearbyAlivePlayer`.
    fn is_near_player(&self, world: &Arc<World>, pos: BlockPos) -> bool {
        let range = f64::from(self.state.lock().required_player_range);
        world
            .nearest_player(
                DVec3::new(
                    f64::from(pos.x()) + 0.5,
                    f64::from(pos.y()) + 0.5,
                    f64::from(pos.z()) + 0.5,
                ),
                range,
                |player| !player.is_spectator() && player.is_alive(),
            )
            .is_some()
    }

    /// Vanilla `BaseSpawner.getOrCreateNextSpawnData`.
    fn next_spawn_data(&self, rng: &mut impl rand::Rng) -> SpawnData {
        let mut state = self.state.lock();
        if let Some(data) = &state.next_spawn_data {
            return data.clone();
        }
        let selected = Self::pick_weighted(&state.spawn_potentials, rng).unwrap_or_default();
        state.next_spawn_data = Some(selected.clone());
        selected
    }

    /// Vanilla `WeightedList.getRandom`.
    fn pick_weighted(potentials: &[SpawnPotential], rng: &mut impl rand::Rng) -> Option<SpawnData> {
        let total: i32 = potentials.iter().map(|entry| entry.weight).sum();
        if total <= 0 {
            return None;
        }
        let mut roll = rng.random_range(0..total);
        for entry in potentials {
            roll -= entry.weight;
            if roll < 0 {
                return Some(entry.data.clone());
            }
        }
        None
    }

    /// Vanilla `BaseSpawner.delay`: re-rolls the delay, advances to the next weighted entry and
    /// broadcasts block event 1.
    fn delay(&self, world: &Arc<World>) {
        let pos = self.get_block_pos();
        let mut rng = rand::rng();
        let selected = {
            let mut state = self.state.lock();
            state.delay = if state.max_spawn_delay <= state.min_spawn_delay {
                state.min_spawn_delay
            } else {
                state.min_spawn_delay
                    + rng.random_range(0..state.max_spawn_delay - state.min_spawn_delay)
            };
            let selected = Self::pick_weighted(&state.spawn_potentials, &mut rng);
            if let Some(data) = &selected {
                state.next_spawn_data = Some(data.clone());
            }
            selected
        };
        if selected.is_some() {
            // Vanilla's `SpawnerBlockEntity.setNextSpawnData` override pushes the new data.
            world.send_block_updated(pos);
        }
        world.block_event(
            pos,
            self.get_block_state().get_block(),
            SPAWNER_EVENT_SPAWN,
            0,
        );
    }

    /// Vanilla `BaseSpawner.serverTick`'s spawn branch.
    #[expect(
        clippy::too_many_lines,
        reason = "mirrors vanilla BaseSpawner.serverTick"
    )]
    fn spawn_attempt(&self, world: &Arc<World>) {
        let pos = self.get_block_pos();
        let mut rng = rand::rng();
        let next_spawn_data = self.next_spawn_data(&mut rng);
        let (spawn_count, spawn_range, max_nearby) = {
            let state = self.state.lock();
            (
                state.spawn_count,
                state.spawn_range,
                state.max_nearby_entities,
            )
        };
        let mut spawned = false;

        for _ in 0..spawn_count {
            let Some(entity_type) = next_spawn_data.entity_type() else {
                self.delay(world);
                return;
            };

            let spawn_pos = Self::read_pos(&next_spawn_data.entity_to_spawn).unwrap_or_else(|| {
                DVec3::new(
                    f64::from(pos.x())
                        + (rng.random::<f64>() - rng.random::<f64>()) * f64::from(spawn_range)
                        + 0.5,
                    f64::from(pos.y()) + f64::from(rng.random_range(0..3)) - 1.0,
                    f64::from(pos.z())
                        + (rng.random::<f64>() - rng.random::<f64>()) * f64::from(spawn_range)
                        + 0.5,
                )
            });

            if !world.no_collision(spawn_aabb(entity_type, spawn_pos)) {
                continue;
            }

            let spawn_block_pos = BlockPos::from(spawn_pos);
            if let Some(rules) = next_spawn_data.custom_spawn_rules {
                // Vanilla also skips friendly categories in Peaceful; Steel exposes the
                // per-type `allowed_in_peaceful` flag instead of the category check.
                if !entity_type.allowed_in_peaceful && world.difficulty() == Difficulty::Peaceful {
                    continue;
                }
                if !rules.is_valid_position(world, spawn_block_pos) {
                    continue;
                }
            } else {
                let mut random = RandomSource::Legacy(LegacyRandom::from_seed(rng.random::<u64>()));
                if !SpawnPlacements::check_spawn_rules(
                    entity_type,
                    world,
                    EntitySpawnReason::Spawner,
                    spawn_block_pos,
                    &mut random,
                ) {
                    continue;
                }
            }

            let Ok(entity) = create_entity_instance(world, entity_type, spawn_pos) else {
                self.delay(world);
                return;
            };
            if entity
                .load_with_owned_components(&next_spawn_data.entity_to_spawn)
                .is_err()
            {
                self.delay(world);
                return;
            }
            entity.base().set_position_local(spawn_pos);
            let yaw = rng.random::<f32>() * 360.0;
            entity.set_rotation((yaw, 0.0));
            entity.set_old_position_to_current();
            entity.base().set_old_rotation_to_current();

            let nearby_aabb = WorldAabb::new(
                f64::from(pos.x()),
                f64::from(pos.y()),
                f64::from(pos.z()),
                f64::from(pos.x()) + 1.0,
                f64::from(pos.y()) + 1.0,
                f64::from(pos.z()) + 1.0,
            )
            .inflate(f64::from(spawn_range));
            let nearby = world
                .get_entities_in_aabb(&nearby_aabb)
                .into_iter()
                .filter(|other| ptr::eq(other.entity_type(), entity_type) && !other.is_spectator())
                .count();
            if i32::try_from(nearby).unwrap_or(i32::MAX) >= max_nearby {
                self.delay(world);
                return;
            }

            if let Some(mob) = entity.as_mob() {
                if !mob.check_spawn_obstruction(world) {
                    continue;
                }
                let has_no_configuration = next_spawn_data.entity_to_spawn.len() == 1
                    && next_spawn_data.entity_to_spawn.string("id").is_some();
                if has_no_configuration {
                    mob.finalize_spawn(world, EntitySpawnReason::Spawner, None);
                }
            }

            if world.try_add_entity(Arc::clone(&entity)).is_err() {
                self.delay(world);
                return;
            }

            world.level_event(SPAWNER_LEVEL_EVENT_SPAWN, pos, 0, None);
            world.game_event(
                &vanilla_game_events::ENTITY_PLACE,
                spawn_block_pos,
                &GameEventContext::new(Some(entity.as_ref()), None),
            );
            if entity.as_mob().is_some() {
                // Vanilla `Mob.spawnAnim` broadcasts entity status byte 20.
                entity.broadcast_entity_event(EntityStatus::SilverfishMergeAnim);
            }
            spawned = true;
        }

        if spawned {
            self.delay(world);
        }
    }

    /// Vanilla `ValueInput.read("Pos", Vec3.CODEC)`: an optional `[x, y, z]` double list.
    fn read_pos(entity: &NbtCompound) -> Option<DVec3> {
        let values = entity.list("Pos")?.doubles()?;
        if values.len() != 3 {
            return None;
        }
        let pos = DVec3::new(values[0], values[1], values[2]);
        pos.is_finite().then_some(pos)
    }
}

/// Vanilla `EntityType.getSpawnAABB`: the type's dimensions centred on `pos`.
pub(super) fn spawn_aabb(entity_type: EntityTypeRef, pos: DVec3) -> WorldAabb {
    let half_width = f64::from(entity_type.dimensions.half_width());
    let height = f64::from(entity_type.dimensions.height);
    WorldAabb::new(
        pos.x - half_width,
        pos.y,
        pos.z - half_width,
        pos.x + half_width,
        pos.y + height,
        pos.z + half_width,
    )
}

/// Vanilla `ValueInput.getShortOr`: reads any numeric tag's value (vanilla's `shortValue()`).
fn read_short_or(view: &NbtCompoundView<'_, '_>, key: &str, default: i32) -> i32 {
    view.get(key)
        .and_then(|tag| tag.codec_i32())
        .unwrap_or(default)
}

/// Vanilla `ValueInput.getIntOr`.
fn read_int_or(view: &NbtCompoundView<'_, '_>, key: &str, default: i32) -> i32 {
    view.get(key)
        .and_then(|tag| tag.codec_i32())
        .unwrap_or(default)
}

impl BlockEntity for SpawnerBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let view: NbtCompoundView<'_, '_> = nbt.into();
        let mut state = self.state.lock();

        state.delay = read_short_or(&view, "Delay", SPAWNER_DEFAULT_SPAWN_DELAY);
        state.next_spawn_data = view
            .compound("SpawnData")
            .and_then(|data| SpawnData::from_view(&data));
        state.min_spawn_delay = read_int_or(&view, "MinSpawnDelay", SPAWNER_MIN_SPAWN_DELAY);
        state.max_spawn_delay = read_int_or(&view, "MaxSpawnDelay", SPAWNER_MAX_SPAWN_DELAY);
        state.spawn_count = read_int_or(&view, "SpawnCount", SPAWNER_SPAWN_COUNT);
        state.max_nearby_entities =
            read_int_or(&view, "MaxNearbyEntities", SPAWNER_MAX_NEARBY_ENTITIES);
        state.required_player_range =
            read_int_or(&view, "RequiredPlayerRange", SPAWNER_REQUIRED_PLAYER_RANGE);
        state.spawn_range = read_int_or(&view, "SpawnRange", SPAWNER_SPAWN_RANGE);

        state.spawn_potentials.clear();
        let potentials = view
            .list("SpawnPotentials")
            .and_then(|list| list.compounds());
        if let Some(entries) = potentials {
            for compound in entries {
                let weight = compound
                    .get("weight")
                    .and_then(|tag| tag.codec_i32())
                    .unwrap_or(1);
                let Some(data) = compound
                    .compound("data")
                    .and_then(|data| SpawnData::from_view(&data))
                else {
                    continue;
                };
                state.spawn_potentials.push(SpawnPotential { weight, data });
            }
        } else {
            // Vanilla falls back to a single-entry list holding the current spawn data.
            let data = state.next_spawn_data.clone().unwrap_or_default();
            state.spawn_potentials = vec![SpawnPotential { weight: 1, data }];
        }
        drop(state);

        // Vanilla `BaseSpawner.load` runs through `setNextSpawnData`, whose block-entity
        // override pushes a block update whenever a level is attached.
        if let Some(world) = self.get_level() {
            world.send_block_updated(self.get_block_pos());
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let state = self.state.lock();
        nbt.insert("Delay", state.delay as i16);
        nbt.insert("MinSpawnDelay", state.min_spawn_delay as i16);
        nbt.insert("MaxSpawnDelay", state.max_spawn_delay as i16);
        nbt.insert("SpawnCount", state.spawn_count as i16);
        nbt.insert("MaxNearbyEntities", state.max_nearby_entities as i16);
        nbt.insert("RequiredPlayerRange", state.required_player_range as i16);
        nbt.insert("SpawnRange", state.spawn_range as i16);

        if let Some(data) = &state.next_spawn_data {
            let mut compound = NbtCompound::new();
            data.write(&mut compound);
            nbt.insert("SpawnData", compound);
        }

        let potentials: Vec<NbtCompound> = state
            .spawn_potentials
            .iter()
            .map(|potential| {
                let mut entry = NbtCompound::new();
                entry.insert("weight", potential.weight);
                let mut data = NbtCompound::new();
                potential.data.write(&mut data);
                entry.insert("data", data);
                entry
            })
            .collect();
        nbt.insert("SpawnPotentials", NbtList::Compound(potentials));
    }

    fn get_update_tag(&self) -> Option<NbtCompound> {
        // Vanilla `SpawnerBlockEntity.getUpdateTag`: `saveCustomOnly` minus `SpawnPotentials`.
        let mut nbt = NbtCompound::new();
        self.save_additional(&mut nbt);
        while nbt.remove("SpawnPotentials").is_some() {}
        Some(nbt)
    }

    fn trigger_event(&self, param_a: i32, param_b: i32) -> bool {
        let _ = param_b;
        // Vanilla `BaseSpawner.onEventTriggered`: only event 1 is handled.
        param_a == SPAWNER_EVENT_SPAWN
    }

    fn tick(&self, world: &Arc<World>) {
        // Vanilla `BaseSpawner.serverTick` gates everything on the player check and the
        // `spawner_blocks_work` game rule (`ServerLevel.isSpawnerBlockEnabled`).
        if !self.is_near_player(world, self.get_block_pos())
            || !world.get_game_rule(&vanilla_game_rules::SPAWNER_BLOCKS_WORK)
        {
            return;
        }

        let needs_delay = self.state.lock().delay == -1;
        if needs_delay {
            self.delay(world);
        }

        let counting_down = {
            let mut state = self.state.lock();
            if state.delay > 0 {
                state.delay -= 1;
                true
            } else {
                false
            }
        };
        if counting_down {
            return;
        }

        self.spawn_attempt(world);
    }
}
