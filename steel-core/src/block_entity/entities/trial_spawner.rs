//! `TrialSpawnerBlockEntity` (`net.minecraft.world.level.block.entity.TrialSpawnerBlockEntity`).
//!
//! Vanilla wraps a `TrialSpawner` state machine plus a `TrialSpawner.FullConfig`. The block
//! entity persists the packed state data (detected players, tracked mobs, cooldown and next
//! spawn timestamps, the pending spawn data and the ejecting loot table) and the full config;
//! the per-tick state machine lives in [`TrialSpawnerBlockEntity::tick`].

use std::str::FromStr;
use std::sync::{Arc, Weak};

use glam::DVec3;
use rand::RngExt as _;
use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::{NbtCompound, NbtList};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, TrialSpawnerState};
use steel_registry::item_stack::ItemStack;
use steel_registry::loot_table::LootContext;
use steel_registry::sound_events;
use steel_registry::vanilla_level_events;
use steel_registry::vanilla_mob_effects;
use steel_registry::{REGISTRY, RegistryExt as _, vanilla_block_entity_types, vanilla_game_events};
use steel_protocol::packets::game::SoundSource;
use steel_utils::entity_events::EntityStatus;
use steel_utils::nbt::NbtNumeric as _;
use steel_utils::types::{Difficulty, GameType, UpdateFlags};
use steel_utils::uuid_ext::UuidExt as _;
use steel_utils::{
    BlockPos, BlockStateId, DowncastType, DowncastTypeKey, Identifier, PackedBlockPos,
    locks::SyncMutex,
};

use super::spawner::{SpawnData, SpawnPotential, spawn_aabb};
use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::entity::{
    Entity as _, EntitySpawnReason, LivingEntity as _, Mob as _, RemovalReason,
    create_entity_instance,
};
use crate::world::game_event::GameEventContext;
use crate::world::spawn::SpawnPlacements;
use crate::world::{ClipBlockShape, ClipFluid, World};

/// Vanilla `TrialSpawner.DETECT_PLAYER_SPAWN_BUFFER`.
pub const DETECT_PLAYER_SPAWN_BUFFER: i64 = 40;
/// Vanilla `TrialSpawner.DEFAULT_TARGET_COOLDOWN_LENGTH`.
pub const DEFAULT_TARGET_COOLDOWN_LENGTH: i32 = 36000;
/// Vanilla `TrialSpawner.DEFAULT_PLAYER_SCAN_RANGE`.
pub const DEFAULT_PLAYER_SCAN_RANGE: i32 = 14;
/// Vanilla `TrialSpawner.MAX_MOB_TRACKING_DISTANCE_SQ`.
const MAX_MOB_TRACKING_DISTANCE_SQR: i64 = 47 * 47;
/// Vanilla `TrialSpawnerStateData.DELAY_BETWEEN_PLAYER_SCANS`.
const DELAY_BETWEEN_PLAYER_SCANS: i64 = 20;
/// Vanilla `TrialSpawnerState.DELAY_BEFORE_EJECT_AFTER_KILLING_LAST_MOB`.
const DELAY_BEFORE_EJECT_AFTER_KILLING_LAST_MOB: f32 = 40.0;
/// Vanilla `TrialSpawnerState.TIME_BETWEEN_EACH_EJECTION`.
const TIME_BETWEEN_EACH_EJECTION: f32 = 30.0;
/// Vanilla `TrialSpawnerConfig.ticksBetweenItemSpawners`.
const TICKS_BETWEEN_ITEM_SPAWNERS: i64 = 160;

/// Vanilla `TrialSpawner.FlameParticle`: the level-event data byte for normal/ominous effects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlameParticle {
    /// `FlameParticle.NORMAL` (data `0`).
    Normal,
    /// `FlameParticle.OMINOUS` (data `1`).
    Ominous,
}

impl FlameParticle {
    /// Vanilla `FlameParticle.encode`.
    #[must_use]
    pub const fn encode(self) -> i32 {
        match self {
            Self::Normal => 0,
            Self::Ominous => 1,
        }
    }
}

/// Vanilla `TrialSpawnerConfig`: the per-difficulty spawn parameters.
#[derive(Clone, Debug)]
pub struct TrialSpawnerConfig {
    /// `spawn_range`: horizontal/vertical spawn offset range.
    pub spawn_range: i32,
    /// `total_mobs`: mobs needed to finish the trial.
    pub total_mobs: f32,
    /// `simultaneous_mobs`: mobs alive at once.
    pub simultaneous_mobs: f32,
    /// `total_mobs_added_per_player`.
    pub total_mobs_added_per_player: f32,
    /// `simultaneous_mobs_added_per_player`.
    pub simultaneous_mobs_added_per_player: f32,
    /// `ticks_between_spawn`: ticks between two spawn attempts.
    pub ticks_between_spawn: i32,
    /// `spawn_potentials`: weighted spawn table.
    pub spawn_potentials: Vec<SpawnPotential>,
    /// `loot_tables_to_eject`: weighted reward tables.
    pub loot_tables_to_eject: Vec<(Identifier, i32)>,
    /// `items_to_drop_when_ominous`.
    pub items_to_drop_when_ominous: Identifier,
}

impl Default for TrialSpawnerConfig {
    fn default() -> Self {
        // Vanilla `TrialSpawnerConfig.Builder` defaults.
        Self {
            spawn_range: 4,
            total_mobs: 6.0,
            simultaneous_mobs: 2.0,
            total_mobs_added_per_player: 2.0,
            simultaneous_mobs_added_per_player: 1.0,
            ticks_between_spawn: 40,
            spawn_potentials: Vec::new(),
            loot_tables_to_eject: vec![
                (
                    Identifier::vanilla_static("spawners/trial_chamber/consumables"),
                    1,
                ),
                (Identifier::vanilla_static("spawners/trial_chamber/key"), 1),
            ],
            items_to_drop_when_ominous: Identifier::vanilla_static(
                "spawners/trial_chamber/items_to_drop_when_ominous",
            ),
        }
    }
}

impl TrialSpawnerConfig {
    /// Vanilla `TrialSpawnerConfig.calculateTargetTotalMobs`.
    #[must_use]
    pub fn calculate_target_total_mobs(&self, additional_players: i32) -> i32 {
        (self.total_mobs + self.total_mobs_added_per_player * additional_players as f32).floor()
            as i32
    }

    /// Vanilla `TrialSpawnerConfig.calculateTargetSimultaneousMobs`.
    #[must_use]
    pub fn calculate_target_simultaneous_mobs(&self, additional_players: i32) -> i32 {
        (self.simultaneous_mobs
            + self.simultaneous_mobs_added_per_player * additional_players as f32)
            .floor() as i32
    }

    /// Vanilla `TrialSpawnerConfig.DIRECT_CODEC` decoding (`optionalFieldOf` defaults).
    fn read(view: &NbtCompoundView<'_, '_>) -> Self {
        let default = Self::default();
        let spawn_potentials = view
            .list("spawn_potentials")
            .and_then(|list| list.compounds())
            .map(|entries| {
                entries
                    .into_iter()
                    .filter_map(|entry| {
                        let weight = entry
                            .get("weight")
                            .and_then(|tag| tag.codec_i32())
                            .unwrap_or(1);
                        let data = entry.compound("data")?;
                        Some(SpawnPotential {
                            weight,
                            data: SpawnData::from_view(&data)?,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let loot_tables_to_eject = view
            .list("loot_tables_to_eject")
            .and_then(|list| list.compounds())
            .map(|entries| {
                entries
                    .into_iter()
                    .filter_map(|entry| {
                        let weight = entry
                            .get("weight")
                            .and_then(|tag| tag.codec_i32())
                            .unwrap_or(1);
                        let id = entry.string("data")?;
                        let id = Identifier::from_str(id.to_str().as_ref()).ok()?;
                        Some((id, weight))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or(default.loot_tables_to_eject);
        let items_to_drop_when_ominous = view
            .string("items_to_drop_when_ominous")
            .and_then(|id| Identifier::from_str(id.to_str().as_ref()).ok())
            .unwrap_or(default.items_to_drop_when_ominous);

        Self {
            spawn_range: read_int_or(view, "spawn_range", default.spawn_range),
            total_mobs: read_float_or(view, "total_mobs", default.total_mobs),
            simultaneous_mobs: read_float_or(view, "simultaneous_mobs", default.simultaneous_mobs),
            total_mobs_added_per_player: read_float_or(
                view,
                "total_mobs_added_per_player",
                default.total_mobs_added_per_player,
            ),
            simultaneous_mobs_added_per_player: read_float_or(
                view,
                "simultaneous_mobs_added_per_player",
                default.simultaneous_mobs_added_per_player,
            ),
            ticks_between_spawn: read_int_or(
                view,
                "ticks_between_spawn",
                default.ticks_between_spawn,
            ),
            spawn_potentials,
            loot_tables_to_eject,
            items_to_drop_when_ominous,
        }
    }

    /// Vanilla `TrialSpawnerConfig.DIRECT_CODEC` encoding.
    fn write(&self, nbt: &mut NbtCompound) {
        nbt.insert("spawn_range", self.spawn_range);
        nbt.insert("total_mobs", self.total_mobs);
        nbt.insert("simultaneous_mobs", self.simultaneous_mobs);
        nbt.insert(
            "total_mobs_added_per_player",
            self.total_mobs_added_per_player,
        );
        nbt.insert(
            "simultaneous_mobs_added_per_player",
            self.simultaneous_mobs_added_per_player,
        );
        nbt.insert("ticks_between_spawn", self.ticks_between_spawn);

        let potentials: Vec<NbtCompound> = self
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
        nbt.insert("spawn_potentials", NbtList::Compound(potentials));

        let loot_tables: Vec<NbtCompound> = self
            .loot_tables_to_eject
            .iter()
            .map(|(id, weight)| {
                let mut entry = NbtCompound::new();
                entry.insert("weight", *weight);
                entry.insert("data", id.to_string());
                entry
            })
            .collect();
        nbt.insert("loot_tables_to_eject", NbtList::Compound(loot_tables));
        nbt.insert(
            "items_to_drop_when_ominous",
            self.items_to_drop_when_ominous.to_string(),
        );
    }

    /// Vanilla `TrialSpawnerConfig` resolved from a `trial_spawner` registry entry.
    ///
    /// Steel's extracted entry keeps the entity compound as JSON, which this lane cannot decode;
    /// the entity id is enough for every builtin entry, so the `entity` compound is rebuilt from
    /// it. Registry entries with extra entity NBT or `equipment` lose those fields.
    fn from_registry(entry: &steel_registry::trial_spawner::TrialSpawnerConfig) -> Self {
        let default = Self::default();
        Self {
            spawn_range: entry.spawn_range,
            total_mobs: entry.total_mobs,
            simultaneous_mobs: entry.simultaneous_mobs,
            total_mobs_added_per_player: entry.total_mobs_added_per_player,
            simultaneous_mobs_added_per_player: entry.simultaneous_mobs_added_per_player,
            ticks_between_spawn: entry.ticks_between_spawn,
            spawn_potentials: entry
                .spawn_potentials
                .iter()
                .map(|weighted| {
                    let mut entity = NbtCompound::new();
                    entity.insert("id", weighted.data.entity_type.to_string());
                    SpawnPotential {
                        weight: weighted.weight,
                        data: SpawnData::from_entity(entity),
                    }
                })
                .collect(),
            loot_tables_to_eject: entry
                .loot_tables_to_eject
                .iter()
                .map(|weighted| (weighted.data.clone(), weighted.weight))
                .collect(),
            items_to_drop_when_ominous: entry
                .items_to_drop_when_ominous
                .clone()
                .unwrap_or(default.items_to_drop_when_ominous),
        }
    }
}

/// One side of vanilla's `Holder<TrialSpawnerConfig>`: a registry reference or an inline value.
#[derive(Clone, Debug)]
struct ConfigSource {
    /// Registry id when the config was referenced by name, so it round-trips as a string.
    id: Option<Identifier>,
    config: TrialSpawnerConfig,
}

impl ConfigSource {
    fn read(view: &NbtCompoundView<'_, '_>, key: &str) -> Option<Self> {
        if let Some(id) = view.string(key) {
            let id = Identifier::from_str(id.to_str().as_ref()).ok()?;
            let entry = REGISTRY.trial_spawner_configs.by_key(&id)?;
            return Some(Self {
                id: Some(id),
                config: TrialSpawnerConfig::from_registry(entry),
            });
        }
        let compound = view.compound(key)?;
        Some(Self {
            id: None,
            config: TrialSpawnerConfig::read(&compound),
        })
    }

    fn write(&self, nbt: &mut NbtCompound, key: &str) {
        match &self.id {
            Some(id) => {
                nbt.insert(key, id.to_string());
            }
            None => {
                let mut compound = NbtCompound::new();
                self.config.write(&mut compound);
                nbt.insert(key, compound);
            }
        }
    }
}

/// Mutable `TrialSpawner` state (vanilla `TrialSpawner` + `TrialSpawnerStateData`).
struct TrialSpawnerData {
    normal: ConfigSource,
    ominous_config: ConfigSource,
    target_cooldown_length: i32,
    required_player_range: i32,
    is_ominous: bool,
    detected_players: Vec<uuid::Uuid>,
    current_mobs: Vec<uuid::Uuid>,
    cooldown_ends_at: i64,
    next_mob_spawns_at: i64,
    total_mobs_spawned: i32,
    next_spawn_data: Option<SpawnData>,
    ejecting_loot_table: Option<Identifier>,
}

impl TrialSpawnerData {
    fn new() -> Self {
        Self {
            normal: ConfigSource {
                id: None,
                config: TrialSpawnerConfig::default(),
            },
            ominous_config: ConfigSource {
                id: None,
                config: TrialSpawnerConfig::default(),
            },
            target_cooldown_length: DEFAULT_TARGET_COOLDOWN_LENGTH,
            required_player_range: DEFAULT_PLAYER_SCAN_RANGE,
            is_ominous: false,
            detected_players: Vec::new(),
            current_mobs: Vec::new(),
            cooldown_ends_at: 0,
            next_mob_spawns_at: 0,
            total_mobs_spawned: 0,
            next_spawn_data: None,
            ejecting_loot_table: None,
        }
    }

    fn active_config(&self) -> &TrialSpawnerConfig {
        if self.is_ominous {
            &self.ominous_config.config
        } else {
            &self.normal.config
        }
    }

    /// Vanilla `TrialSpawnerStateData.reset`.
    fn reset(&mut self) {
        self.current_mobs.clear();
        self.next_spawn_data = None;
        self.reset_statistics();
    }

    /// Vanilla `TrialSpawnerStateData.resetStatistics`.
    fn reset_statistics(&mut self) {
        self.detected_players.clear();
        self.total_mobs_spawned = 0;
        self.next_mob_spawns_at = 0;
        self.cooldown_ends_at = 0;
    }
}

/// `TrialSpawnerBlockEntity`.
pub struct TrialSpawnerBlockEntity {
    base: BlockEntityBase,
    state: SyncMutex<TrialSpawnerData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `TrialSpawnerBlockEntity`.
unsafe impl DowncastType for TrialSpawnerBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/trial_spawner");
}

impl TrialSpawnerBlockEntity {
    /// Creates a trial-spawner block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self {
            base: BlockEntityBase::new(
                &vanilla_block_entity_types::TRIAL_SPAWNER,
                level,
                pos,
                state,
            ),
            state: SyncMutex::new(TrialSpawnerData::new()),
        }
    }

    /// Returns whether this spawner is the ominous variant.
    #[must_use]
    pub fn is_ominous(&self) -> bool {
        self.state.lock().is_ominous
    }

    /// Returns a snapshot of the active (ominous or normal) configuration.
    #[must_use]
    pub fn config(&self) -> TrialSpawnerConfig {
        self.state.lock().active_config().clone()
    }

    /// Vanilla `TrialSpawner.canSpawnInLevel`.
    fn can_spawn_in_level(world: &Arc<World>) -> bool {
        if !world.get_game_rule(&steel_registry::vanilla_game_rules::SPAWNER_BLOCKS_WORK) {
            return false;
        }
        world.difficulty() != Difficulty::Peaceful
            && world.get_game_rule(&steel_registry::vanilla_game_rules::SPAWN_MOBS)
    }

    /// Vanilla `TrialSpawnerStateData.hasMobToSpawn`.
    fn has_mob_to_spawn(&self, rng: &mut impl rand::Rng) -> bool {
        let config = self.state.lock().active_config().clone();
        self.get_or_create_next_spawn_data(rng, &config)
            .entity_to_spawn
            .string("id")
            .is_some()
            || !config.spawn_potentials.is_empty()
    }

    /// Vanilla `TrialSpawnerStateData.getOrCreateNextSpawnData`.
    fn get_or_create_next_spawn_data(
        &self,
        rng: &mut impl rand::Rng,
        config: &TrialSpawnerConfig,
    ) -> SpawnData {
        let mut state = self.state.lock();
        if let Some(data) = &state.next_spawn_data {
            return data.clone();
        }
        let selected = if config.spawn_potentials.is_empty() {
            None
        } else {
            pick_weighted(&config.spawn_potentials, rng)
        };
        let data = selected.unwrap_or_default();
        state.next_spawn_data = Some(data.clone());
        drop(state);
        self.mark_updated();
        data
    }

    /// Vanilla `TrialSpawnerStateData.countAdditionalPlayers`.
    fn count_additional_players(&self) -> i32 {
        let detected = self.state.lock().detected_players.len();
        i32::try_from(detected.saturating_sub(1)).unwrap_or(0).max(0)
    }

    /// Vanilla `TrialSpawnerStateData.tryDetectPlayers`.
    fn try_detect_players(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        state: TrialSpawnerState,
        is_ominous: bool,
    ) {
        let throttled = (PackedBlockPos::from(pos).as_raw() + world.game_time())
            % DELAY_BETWEEN_PLAYER_SCANS
            != 0;
        if throttled {
            return;
        }
        if state == TrialSpawnerState::Cooldown && is_ominous {
            return;
        }

        let in_line_of_sight = self.detect_players(
            world,
            pos,
            f64::from(self.state.lock().required_player_range),
            true,
        );
        let mut became_ominous = false;
        if !is_ominous && !in_line_of_sight.is_empty() {
            if let Some((player, bad_omen)) = find_player_with_ominous_effect(world, &in_line_of_sight)
            {
                if bad_omen {
                    transform_bad_omen_into_trial_omen(&player);
                }
                let eye_pos = DVec3::new(
                    player.position().x,
                    player.get_eye_y(),
                    player.position().z,
                );
                world.level_event(
                    vanilla_level_events::PARTICLES_TRIAL_SPAWNER_BECOME_OMINOUS,
                    BlockPos::from(eye_pos),
                    0,
                    None,
                );
                self.apply_ominous(world, pos);
                became_ominous = true;
            }
        }

        let current_state = self
            .get_block_state()
            .get_value(&BlockStateProperties::TRIAL_SPAWNER_STATE);
        if current_state == TrialSpawnerState::Cooldown && !became_ominous {
            return;
        }

        let searching_for_first_player = self.state.lock().detected_players.is_empty();
        let found = if searching_for_first_player {
            in_line_of_sight
        } else {
            self.detect_players(
                world,
                pos,
                f64::from(self.state.lock().required_player_range),
                false,
            )
        };

        let mut state_lock = self.state.lock();
        let mut added = false;
        for uuid in found {
            if !state_lock.detected_players.contains(&uuid) {
                state_lock.detected_players.push(uuid);
                added = true;
            }
        }
        if added {
            state_lock.next_mob_spawns_at = state_lock
                .next_mob_spawns_at
                .max(world.game_time() + DETECT_PLAYER_SPAWN_BUFFER);
            let detected = state_lock.detected_players.len() as i32;
            let event = if is_ominous {
                vanilla_level_events::PARTICLES_TRIAL_SPAWNER_DETECT_PLAYER_OMINOUS
            } else {
                vanilla_level_events::PARTICLES_TRIAL_SPAWNER_DETECT_PLAYER
            };
            drop(state_lock);
            if !became_ominous {
                world.level_event(event, pos, detected, None);
            }
        }
    }

    /// Vanilla `PlayerDetector.NO_CREATIVE_PLAYERS` (`PlayerDetector.EntitySelector.SELECT_FROM_LEVEL`).
    fn detect_players(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        range: f64,
        require_line_of_sight: bool,
    ) -> Vec<uuid::Uuid> {
        let mut found = Vec::new();
        world.players.iter_players(|uuid, player| {
            let block_pos = player.block_position();
            let dx = f64::from(block_pos.x() - pos.x());
            let dy = f64::from(block_pos.y() - pos.y());
            let dz = f64::from(block_pos.z() - pos.z());
            let in_range = dx * dx + dy * dy + dz * dz < range * range;
            if in_range
                && player.game_mode() != GameType::Creative
                && !player.is_spectator()
                && (!require_line_of_sight || in_line_of_sight(world, pos, player.get_eye_y(), player.position()))
            {
                found.push(*uuid);
            }
            true
        });
        found
    }

    /// Vanilla `TrialSpawner.applyOminous`.
    fn apply_ominous(&self, world: &Arc<World>, pos: BlockPos) {
        let block_state = self.get_block_state();
        world.set_block(
            pos,
            block_state.set_value(&BlockStateProperties::OMINOUS, true),
            UpdateFlags::UPDATE_ALL,
        );
        world.level_event(
            vanilla_level_events::PARTICLES_TRIAL_SPAWNER_BECOME_OMINOUS,
            pos,
            1,
            None,
        );
        {
            let mut state = self.state.lock();
            state.is_ominous = true;
        }
        self.reset_after_becoming_ominous(world, pos);
    }

    /// Vanilla `TrialSpawnerStateData.resetAfterBecomingOminous`.
    fn reset_after_becoming_ominous(&self, world: &Arc<World>, pos: BlockPos) {
        let ominous_config = self.state.lock().ominous_config.config.clone();
        let mobs: Vec<uuid::Uuid> = self.state.lock().current_mobs.clone();
        for uuid in mobs {
            if let Some(entity) = world.get_entity_by_uuid(&uuid) {
                world.level_event(
                    vanilla_level_events::PARTICLES_TRIAL_SPAWNER_SPAWN_MOB_AT,
                    entity.block_position(),
                    FlameParticle::Normal.encode(),
                    None,
                );
                entity.set_removed(RemovalReason::Discarded);
            }
        }

        let mut state = self.state.lock();
        if !ominous_config.spawn_potentials.is_empty() {
            state.next_spawn_data = None;
        }
        state.total_mobs_spawned = 0;
        state.current_mobs.clear();
        state.next_mob_spawns_at = world.game_time() + i64::from(ominous_config.ticks_between_spawn);
        state.cooldown_ends_at = world.game_time() + TICKS_BETWEEN_ITEM_SPAWNERS;
        drop(state);
        self.mark_updated();
    }

    /// Vanilla `TrialSpawner.removeOminous`.
    fn remove_ominous(&self, world: &Arc<World>, pos: BlockPos) {
        let block_state = self.get_block_state();
        world.set_block(
            pos,
            block_state.set_value(&BlockStateProperties::OMINOUS, false),
            UpdateFlags::UPDATE_ALL,
        );
        self.state.lock().is_ominous = false;
    }

    /// Vanilla `TrialSpawner.markUpdated` (`TrialSpawnerBlockEntity.markUpdated`).
    fn mark_updated(&self) {
        BlockEntity::set_changed(self);
        if let Some(world) = self.get_level() {
            world.send_block_updated(self.get_block_pos());
        }
    }

    /// Vanilla `TrialSpawner.setState` (`TrialSpawnerBlockEntity.setState`).
    fn set_state(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        block_state: BlockStateId,
        next: TrialSpawnerState,
    ) {
        BlockEntity::set_changed(self);
        world.set_block(
            pos,
            block_state.set_value(&BlockStateProperties::TRIAL_SPAWNER_STATE, next),
            UpdateFlags::UPDATE_ALL,
        );
    }

    /// Vanilla `TrialSpawnerState.tickAndGetNext`.
    fn tick_and_get_next(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        current: TrialSpawnerState,
        is_ominous: bool,
    ) -> TrialSpawnerState {
        let config = {
            let state = self.state.lock();
            state.active_config().clone()
        };
        let mut rng = rand::rng();

        match current {
            TrialSpawnerState::Inactive => {
                let has_entity = self
                    .get_or_create_next_spawn_data(&mut rng, &config)
                    .entity_to_spawn
                    .string("id")
                    .is_some();
                if has_entity {
                    TrialSpawnerState::WaitingForPlayers
                } else {
                    TrialSpawnerState::Inactive
                }
            }
            TrialSpawnerState::WaitingForPlayers => {
                if !Self::can_spawn_in_level(world) {
                    self.state.lock().reset_statistics();
                    return TrialSpawnerState::WaitingForPlayers;
                }
                if !self.has_mob_to_spawn(&mut rng) {
                    return TrialSpawnerState::Inactive;
                }
                self.try_detect_players(
                    world,
                    pos,
                    TrialSpawnerState::WaitingForPlayers,
                    is_ominous,
                );
                if self.state.lock().detected_players.is_empty() {
                    TrialSpawnerState::WaitingForPlayers
                } else {
                    TrialSpawnerState::Active
                }
            }
            TrialSpawnerState::Active => {
                if !Self::can_spawn_in_level(world) {
                    self.state.lock().reset_statistics();
                    return TrialSpawnerState::WaitingForPlayers;
                }
                if !self.has_mob_to_spawn(&mut rng) {
                    return TrialSpawnerState::Inactive;
                }
                let additional_players = self.count_additional_players();
                self.try_detect_players(
                    world,
                    pos,
                    TrialSpawnerState::Active,
                    is_ominous,
                );

                let finished = self.state.lock().total_mobs_spawned
                    >= config.calculate_target_total_mobs(additional_players);
                if finished {
                    if self.state.lock().current_mobs.is_empty() {
                        let target_cooldown = self.state.lock().target_cooldown_length;
                        let mut state = self.state.lock();
                        state.cooldown_ends_at =
                            world.game_time() + i64::from(target_cooldown);
                        state.total_mobs_spawned = 0;
                        state.next_mob_spawns_at = 0;
                        return TrialSpawnerState::WaitingForRewardEjection;
                    }
                } else if self.is_ready_to_spawn_next_mob(world, &config, additional_players) {
                    if let Some(mob_id) = self.spawn_mob(world, pos, &config) {
                        {
                            let mut state = self.state.lock();
                            state.current_mobs.push(mob_id);
                            state.total_mobs_spawned += 1;
                            state.next_mob_spawns_at =
                                world.game_time() + i64::from(config.ticks_between_spawn);
                        }
                        if let Some(entry) = pick_weighted(&config.spawn_potentials, &mut rng) {
                            self.state.lock().next_spawn_data = Some(entry);
                            self.mark_updated();
                        }
                    }
                }
                TrialSpawnerState::Active
            }
            TrialSpawnerState::WaitingForRewardEjection => {
                let target_cooldown = self.state.lock().target_cooldown_length;
                let cooldown_ends_at = self.state.lock().cooldown_ends_at;
                let cooldown_started_at = cooldown_ends_at - i64::from(target_cooldown);
                if (world.game_time() as f32)
                    >= (cooldown_started_at as f32 + DELAY_BEFORE_EJECT_AFTER_KILLING_LAST_MOB)
                {
                    world.play_sound(
                        &sound_events::BLOCK_TRIAL_SPAWNER_OPEN_SHUTTER,
                        SoundSource::Blocks,
                        pos,
                        1.0,
                        1.0,
                        None,
                    );
                    TrialSpawnerState::EjectingReward
                } else {
                    TrialSpawnerState::WaitingForRewardEjection
                }
            }
            TrialSpawnerState::EjectingReward => {
                let (target_cooldown, cooldown_ends_at) = {
                    let state = self.state.lock();
                    (state.target_cooldown_length, state.cooldown_ends_at)
                };
                let cooldown_started_at = cooldown_ends_at - i64::from(target_cooldown);
                let ready = ((world.game_time() - cooldown_started_at) as f32)
                    % TIME_BETWEEN_EACH_EJECTION
                    == 0.0;
                if !ready {
                    return TrialSpawnerState::EjectingReward;
                }
                let no_players = self.state.lock().detected_players.is_empty();
                if no_players {
                    world.play_sound(
                        &sound_events::BLOCK_TRIAL_SPAWNER_CLOSE_SHUTTER,
                        SoundSource::Blocks,
                        pos,
                        1.0,
                        1.0,
                        None,
                    );
                    self.state.lock().ejecting_loot_table = None;
                    return TrialSpawnerState::Cooldown;
                }

                let loot_table = {
                    let mut state = self.state.lock();
                    if state.ejecting_loot_table.is_none() {
                        state.ejecting_loot_table =
                            pick_weighted_identifier(&config.loot_tables_to_eject, &mut rng);
                    }
                    state.ejecting_loot_table.clone()
                };
                if let Some(loot_table) = loot_table {
                    self.eject_reward(world, pos, &loot_table);
                }
                let mut state = self.state.lock();
                if !state.detected_players.is_empty() {
                    state.detected_players.remove(0);
                }
                TrialSpawnerState::EjectingReward
            }
            TrialSpawnerState::Cooldown => {
                self.try_detect_players(
                    world,
                    pos,
                    TrialSpawnerState::Cooldown,
                    is_ominous,
                );
                if !self.state.lock().detected_players.is_empty() {
                    let mut state = self.state.lock();
                    state.total_mobs_spawned = 0;
                    state.next_mob_spawns_at = 0;
                    return TrialSpawnerState::Active;
                }
                let cooldown_finished = world.game_time() >= self.state.lock().cooldown_ends_at;
                if cooldown_finished {
                    self.remove_ominous(world, pos);
                    self.state.lock().reset();
                    TrialSpawnerState::WaitingForPlayers
                } else {
                    TrialSpawnerState::Cooldown
                }
            }
        }
    }

    /// Vanilla `TrialSpawnerStateData.isReadyToSpawnNextMob`.
    fn is_ready_to_spawn_next_mob(
        &self,
        world: &Arc<World>,
        config: &TrialSpawnerConfig,
        additional_players: i32,
    ) -> bool {
        let state = self.state.lock();
        world.game_time() >= state.next_mob_spawns_at
            && (state.current_mobs.len() as i32)
                < config.calculate_target_simultaneous_mobs(additional_players)
    }

    /// Vanilla `TrialSpawner.spawnMob`.
    fn spawn_mob(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        config: &TrialSpawnerConfig,
    ) -> Option<uuid::Uuid> {
        let mut rng = rand::rng();
        let next_spawn_data = self.get_or_create_next_spawn_data(&mut rng, config);
        let entity_type = next_spawn_data.entity_type()?;

        let spawn_pos = Self::read_pos(&next_spawn_data.entity_to_spawn).unwrap_or_else(|| {
            DVec3::new(
                f64::from(pos.x())
                    + (rng.random::<f64>() - rng.random::<f64>()) * f64::from(config.spawn_range)
                    + 0.5,
                f64::from(pos.y()) + f64::from(rng.random_range(0..3)) - 1.0,
                f64::from(pos.z())
                    + (rng.random::<f64>() - rng.random::<f64>()) * f64::from(config.spawn_range)
                    + 0.5,
            )
        });

        if !world.no_collision(spawn_aabb(entity_type, spawn_pos)) {
            return None;
        }
        if !line_of_sight_to_pos(world, pos, spawn_pos) {
            return None;
        }

        let spawn_block_pos = BlockPos::from(spawn_pos);
        let mut random = steel_utils::random::RandomSource::Legacy(
            steel_utils::random::legacy_random::LegacyRandom::from_seed(rng.random::<u64>()),
        );
        if !SpawnPlacements::check_spawn_rules(
            entity_type,
            world,
            EntitySpawnReason::TrialSpawner,
            spawn_block_pos,
            &mut random,
        ) {
            return None;
        }
        if let Some(rules) = next_spawn_data.custom_spawn_rules
            && !rules.is_valid_position(world, spawn_block_pos)
        {
            return None;
        }

        let entity = create_entity_instance(world, entity_type, spawn_pos).ok()?;
        if entity
            .load_with_owned_components(&next_spawn_data.entity_to_spawn)
            .is_err()
        {
            return None;
        }
        entity.base().set_position_local(spawn_pos);
        let yaw = rng.random::<f32>() * 360.0;
        entity.set_rotation((yaw, 0.0));
        entity.set_old_position_to_current();
        entity.base().set_old_rotation_to_current();

        if let Some(mob) = entity.as_mob() {
            if !mob.check_spawn_obstruction(world) {
                return None;
            }
            let has_no_configuration = next_spawn_data.entity_to_spawn.len() == 1
                && next_spawn_data.entity_to_spawn.string("id").is_some();
            if has_no_configuration {
                mob.finalize_spawn(world, EntitySpawnReason::TrialSpawner, None);
            }
            mob.set_persistence_required();
        }

        if world.try_add_entity(Arc::clone(&entity)).is_err() {
            return None;
        }

        let flame = if self.is_ominous() {
            FlameParticle::Ominous
        } else {
            FlameParticle::Normal
        };
        world.level_event(
            vanilla_level_events::PARTICLES_TRIAL_SPAWNER_SPAWN,
            pos,
            flame.encode(),
            None,
        );
        world.level_event(
            vanilla_level_events::PARTICLES_TRIAL_SPAWNER_SPAWN_MOB_AT,
            spawn_block_pos,
            flame.encode(),
            None,
        );
        world.game_event(
            &vanilla_game_events::ENTITY_PLACE,
            spawn_block_pos,
            &GameEventContext::new(Some(entity.as_ref()), None),
        );
        if entity.as_mob().is_some() {
            // Vanilla `Mob.spawnAnim` broadcasts entity status byte 20.
            entity.broadcast_entity_event(EntityStatus::SilverfishMergeAnim);
        }
        Some(entity.uuid())
    }

    /// Vanilla `TrialSpawner.ejectReward`.
    fn eject_reward(&self, world: &Arc<World>, pos: BlockPos, loot_table: &Identifier) {
        let Some(table) = REGISTRY.loot_tables.by_key(loot_table) else {
            return;
        };
        let mut rng = rand::rng();
        let mut ctx = LootContext::new(&mut rng);
        let drops = table.get_random_items(&mut ctx);
        if drops.is_empty() {
            return;
        }
        for item in drops {
            dispense_item(world, item, pos);
        }
        world.level_event(
            vanilla_level_events::ANIMATION_TRIAL_SPAWNER_EJECT_ITEM,
            pos,
            0,
            None,
        );
    }

    /// Vanilla `ValueInput.read("Pos", Vec3.CODEC)`.
    fn read_pos(entity: &NbtCompound) -> Option<DVec3> {
        let values = entity.list("Pos")?.doubles()?;
        if values.len() != 3 {
            return None;
        }
        let pos = DVec3::new(values[0], values[1], values[2]);
        pos.is_finite().then_some(pos)
    }
}

/// Vanilla `DefaultDispenseItemBehavior.spawnItem(level, stack, 2, Direction.UP, pos.above(1.2))`.
fn dispense_item(world: &Arc<World>, item: ItemStack, pos: BlockPos) {
    let mut rng = rand::rng();
    let spawn_pos = DVec3::new(
        f64::from(pos.x()) + 0.5,
        f64::from(pos.y()) + 1.2 - 0.125,
        f64::from(pos.z()) + 0.5,
    );
    let velocity = DVec3::new(
        triangle(&mut rng, 0.0, 0.017_227_5 * 2.0),
        triangle(&mut rng, 0.2, 0.017_227_5 * 2.0),
        triangle(&mut rng, 0.0, 0.017_227_5 * 2.0),
    );
    world.spawn_item_with_velocity(spawn_pos, item, velocity);
}

/// Vanilla `RandomSource.triangle`.
fn triangle(rng: &mut impl rand::Rng, min: f64, max: f64) -> f64 {
    (rng.random::<f64>() - rng.random::<f64>()) * (max - min) + min
}

/// Vanilla `WeightedList.getRandom` for a `SpawnData` list.
fn pick_weighted(
    potentials: &[SpawnPotential],
    rng: &mut impl rand::Rng,
) -> Option<SpawnData> {
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

/// Vanilla `WeightedList.getRandom` for a loot-table list.
fn pick_weighted_identifier(
    entries: &[(Identifier, i32)],
    rng: &mut impl rand::Rng,
) -> Option<Identifier> {
    let total: i32 = entries.iter().map(|(_, weight)| *weight).sum();
    if total <= 0 {
        return None;
    }
    let mut roll = rng.random_range(0..total);
    for (id, weight) in entries {
        roll -= *weight;
        if roll < 0 {
            return Some(id.clone());
        }
    }
    None
}

/// Vanilla `PlayerDetector.inLineOfSight`.
fn in_line_of_sight(world: &Arc<World>, pos: BlockPos, eye_y: f64, player_pos: DVec3) -> bool {
    let origin = DVec3::new(
        f64::from(pos.x()) + 0.5,
        f64::from(pos.y()) + 0.5,
        f64::from(pos.z()) + 0.5,
    );
    let dest = DVec3::new(player_pos.x, eye_y, player_pos.z);
    let hit = world.clip(dest, origin, ClipBlockShape::Visual, ClipFluid::None);
    hit.block_pos == BlockPos::from(origin) || hit.is_miss()
}

/// Vanilla `TrialSpawner.inLineOfSight` (spawn position instead of an eye position).
fn line_of_sight_to_pos(world: &Arc<World>, pos: BlockPos, target: DVec3) -> bool {
    let origin = DVec3::new(
        f64::from(pos.x()) + 0.5,
        f64::from(pos.y()) + 0.5,
        f64::from(pos.z()) + 0.5,
    );
    let hit = world.clip(target, origin, ClipBlockShape::Visual, ClipFluid::None);
    hit.block_pos == BlockPos::from(origin) || hit.is_miss()
}

/// Vanilla `TrialSpawnerStateData.findPlayerWithOminousEffect`: returns the first player holding
/// trial omen, otherwise the last player holding bad omen (`true` means bad omen).
fn find_player_with_ominous_effect(
    world: &Arc<World>,
    players: &[uuid::Uuid],
) -> Option<(Arc<crate::player::Player>, bool)> {
    let mut bad_omen_player = None;
    for uuid in players {
        let Some(player) = world.players.get_by_uuid(uuid) else {
            continue;
        };
        if player.has_mob_effect(&vanilla_mob_effects::TRIAL_OMEN) {
            return Some((player, false));
        }
        if player.has_mob_effect(&vanilla_mob_effects::BAD_OMEN) {
            bad_omen_player = Some(player);
        }
    }
    bad_omen_player.map(|player| (player, true))
}

/// Vanilla `TrialSpawnerStateData.transformBadOmenIntoTrialOmen`.
fn transform_bad_omen_into_trial_omen(player: &crate::player::Player) {
    let Some(bad_omen) = player.mob_effect(&vanilla_mob_effects::BAD_OMEN) else {
        return;
    };
    let amplifier = bad_omen.amplifier() + 1;
    let duration = 18_000 * amplifier;
    player.remove_mob_effect(&vanilla_mob_effects::BAD_OMEN);
    player.add_mob_effect(crate::entity::MobEffectInstance::with_duration(
        &vanilla_mob_effects::TRIAL_OMEN,
        duration,
        0,
    ));
}

/// Vanilla `ValueInput.getIntOr`.
fn read_int_or(view: &NbtCompoundView<'_, '_>, key: &str, default: i32) -> i32 {
    view.get(key)
        .and_then(|tag| tag.codec_i32())
        .unwrap_or(default)
}

/// Vanilla `ValueInput.getFloatOr`.
fn read_float_or(view: &NbtCompoundView<'_, '_>, key: &str, default: f32) -> f32 {
    view.get(key)
        .and_then(|tag| tag.codec_f32())
        .unwrap_or(default)
}

/// Vanilla `UUIDUtil.CODEC` list encoding (int arrays).
fn write_uuid_list(ids: &[uuid::Uuid]) -> NbtList {
    NbtList::IntArray(
        ids.iter()
            .map(|uuid| uuid.to_int_array().to_vec())
            .collect::<Vec<_>>(),
    )
}

/// Vanilla `UUIDUtil.CODEC` list decoding.
fn read_uuid_list(view: &NbtCompoundView<'_, '_>, key: &str) -> Vec<uuid::Uuid> {
    let Some(list) = view.list(key) else {
        return Vec::new();
    };
    let Some(arrays) = list.int_arrays() else {
        return Vec::new();
    };
    arrays
        .iter()
        .filter_map(|raw| uuid::Uuid::from_int_array(&raw.to_vec()))
        .collect()
}

impl BlockEntity for TrialSpawnerBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let view: NbtCompoundView<'_, '_> = nbt.into();
        let mut state = self.state.lock();

        // Vanilla `TrialSpawnerStateData.Packed.MAP_CODEC`: fields live at the root.
        state.detected_players = read_uuid_list(&view, "registered_players");
        state.current_mobs = read_uuid_list(&view, "current_mobs");
        state.cooldown_ends_at = view.long("cooldown_ends_at").unwrap_or(0);
        state.next_mob_spawns_at = view.long("next_mob_spawns_at").unwrap_or(0);
        state.total_mobs_spawned = read_int_or(&view, "total_mobs_spawned", 0);
        state.next_spawn_data = view
            .compound("spawn_data")
            .and_then(|data| SpawnData::from_view(&data));
        state.ejecting_loot_table = view
            .string("ejecting_loot_table")
            .and_then(|id| Identifier::from_str(id.to_str().as_ref()).ok());

        // Vanilla `TrialSpawner.FullConfig.MAP_CODEC`.
        if let Some(source) = ConfigSource::read(&view, "normal_config") {
            state.normal = source;
        }
        if let Some(source) = ConfigSource::read(&view, "ominous_config") {
            state.ominous_config = source;
        }
        state.target_cooldown_length = read_int_or(
            &view,
            "target_cooldown_length",
            DEFAULT_TARGET_COOLDOWN_LENGTH,
        );
        state.required_player_range =
            read_int_or(&view, "required_player_range", DEFAULT_PLAYER_SCAN_RANGE);
        drop(state);

        // Vanilla `TrialSpawnerBlockEntity.loadAdditional` calls `markUpdated`.
        if self.get_level().is_some() {
            self.mark_updated();
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let state = self.state.lock();
        nbt.insert("registered_players", write_uuid_list(&state.detected_players));
        nbt.insert("current_mobs", write_uuid_list(&state.current_mobs));
        nbt.insert("cooldown_ends_at", state.cooldown_ends_at);
        nbt.insert("next_mob_spawns_at", state.next_mob_spawns_at);
        nbt.insert("total_mobs_spawned", state.total_mobs_spawned);
        if let Some(data) = &state.next_spawn_data {
            let mut compound = NbtCompound::new();
            data.write(&mut compound);
            nbt.insert("spawn_data", compound);
        }
        if let Some(loot_table) = &state.ejecting_loot_table {
            nbt.insert("ejecting_loot_table", loot_table.to_string());
        }

        state.normal.write(nbt, "normal_config");
        state.ominous_config.write(nbt, "ominous_config");
        nbt.insert("target_cooldown_length", state.target_cooldown_length);
        nbt.insert("required_player_range", state.required_player_range);
    }

    fn get_update_tag(&self) -> Option<NbtCompound> {
        // Vanilla `TrialSpawnerStateData.getUpdateTag`.
        let state = self.state.lock();
        let current = self
            .get_block_state()
            .get_value(&BlockStateProperties::TRIAL_SPAWNER_STATE);
        let mut nbt = NbtCompound::new();
        if current == TrialSpawnerState::Active {
            nbt.insert("next_mob_spawns_at", state.next_mob_spawns_at);
        }
        if let Some(data) = &state.next_spawn_data {
            let mut compound = NbtCompound::new();
            data.write(&mut compound);
            nbt.insert("spawn_data", compound);
        }
        Some(nbt)
    }

    fn tick(&self, world: &Arc<World>) {
        let pos = self.get_block_pos();
        let block_state = self.get_block_state();
        let is_ominous = block_state.get_value(&BlockStateProperties::OMINOUS);
        let current = block_state.get_value(&BlockStateProperties::TRIAL_SPAWNER_STATE);
        self.state.lock().is_ominous = is_ominous;

        {
            let mut state = self.state.lock();
            let config_ticks = state.active_config().ticks_between_spawn;
            let before = state.current_mobs.len();
            state
                .current_mobs
                .retain(|id| !should_mob_be_untracked(world, pos, id));
            if state.current_mobs.len() != before {
                state.next_mob_spawns_at = world.game_time() + i64::from(config_ticks);
            }
        }

        let next = self.tick_and_get_next(world, pos, current, is_ominous);
        if next != current {
            self.set_state(world, pos, block_state, next);
        }
    }
}

/// Vanilla `TrialSpawner.shouldMobBeUntracked`.
fn should_mob_be_untracked(world: &Arc<World>, pos: BlockPos, id: &uuid::Uuid) -> bool {
    // Steel keeps one entity manager per dimension, so the dimension comparison is implied.
    let Some(entity) = world.get_entity_by_uuid(id) else {
        return true;
    };
    if !entity.is_alive() {
        return true;
    }
    let entity_pos = entity.block_position();
    let dx = i64::from(entity_pos.x() - pos.x());
    let dy = i64::from(entity_pos.y() - pos.y());
    let dz = i64::from(entity_pos.z() - pos.z());
    dx * dx + dy * dy + dz * dz > MAX_MOB_TRACKING_DISTANCE_SQR
}
