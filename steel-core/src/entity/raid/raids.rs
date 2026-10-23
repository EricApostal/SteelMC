//! Vanilla `Raids`: the per-level raid manager and its saved data.
//!
//! Vanilla stores this as a `SavedData` (`Raids.TYPE`, file id `raids`). Steel stores the
//! manager in the world behind a lock and persists the same fields through
//! [`SavedDataName`](steel_utils::saved_data::SavedDataName) `raids`.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use steel_registry::vanilla_game_rules::RAIDS;
use steel_utils::saved_data::{SavedDataManager, SavedDataName};
use steel_utils::{BlockPos, ChunkPos};
use uuid::Uuid;

use crate::entity::raid::raid::{
    DEFAULT_MAX_RAID_OMEN_LEVEL, MAX_NO_ACTION_TIME, Raid, RaidStatus, VALID_RAID_RADIUS_SQR,
    village_raid_center,
};
use crate::entity::raid::raider::Raider;
use crate::entity::{Entity, LivingEntity, Player, SharedEntity};
use crate::world::World;

/// Vanilla `Raids.RAID_FILE_ID`.
pub const RAID_FILE_ID: &str = "raids";

/// Vanilla `Raids` saved-data field count guard.
const SAVED_DIRTY_INTERVAL_TICKS: i32 = 200;

/// Vanilla `Raids.canJoinRaid`.
#[must_use]
pub fn can_join_raid(raider: &dyn Raider) -> bool {
    LivingEntity::is_alive(raider)
        && raider.can_join_raid()
        && LivingEntity::no_action_time(raider) <= MAX_NO_ACTION_TIME
}

/// A read-only view of one raid, handed to consumers that cannot hold the raid lock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RaidSnapshot {
    /// Vanilla `Raids.getId(raid)`.
    pub id: i32,
    /// Vanilla `Raid.getCenter`.
    pub center: BlockPos,
    /// Vanilla `Raid.getStatus`.
    pub status: RaidStatus,
    /// Vanilla `Raid.isActive`.
    pub active: bool,
    /// Vanilla `Raid.isStarted`.
    pub started: bool,
    /// Vanilla `Raid.getGroupsSpawned`.
    pub groups_spawned: i32,
    /// Vanilla `Raid.getNumGroups`.
    pub num_groups: i32,
    /// Vanilla `Raid.getRaidOmenLevel`.
    pub raid_omen_level: i32,
    /// Vanilla `Raid.getTotalRaidersAlive`.
    pub total_raiders_alive: i32,
    /// Vanilla `Raid.raidCooldownTicks`.
    pub raid_cooldown_ticks: i32,
    /// Vanilla `Raid.getEnchantOdds`.
    pub enchant_odds: f32,
}

impl RaidSnapshot {
    /// Vanilla `Raid.isActive`.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// Vanilla `Raid.isStarted`.
    #[must_use]
    pub const fn is_started(&self) -> bool {
        self.started
    }

    /// Vanilla `Raid.isStopped`.
    #[must_use]
    pub const fn is_stopped(&self) -> bool {
        matches!(self.status, RaidStatus::Stopped)
    }

    /// Vanilla `Raid.isVictory`.
    #[must_use]
    pub const fn is_victory(&self) -> bool {
        matches!(self.status, RaidStatus::Victory)
    }

    /// Vanilla `Raid.isLoss`.
    #[must_use]
    pub const fn is_loss(&self) -> bool {
        matches!(self.status, RaidStatus::Loss)
    }

    /// Vanilla `Raid.isOver`.
    #[must_use]
    pub const fn is_over(&self) -> bool {
        self.is_victory() || self.is_loss()
    }

    /// Vanilla `Raid.hasFirstWaveSpawned`.
    #[must_use]
    pub const fn has_first_wave_spawned(&self) -> bool {
        self.groups_spawned > 0
    }

    /// Vanilla `Raid.getMaxRaidOmenLevel`.
    #[must_use]
    pub const fn max_raid_omen_level(&self) -> i32 {
        DEFAULT_MAX_RAID_OMEN_LEVEL
    }

    /// Vanilla `Raid.isBetweenWaves`.
    #[must_use]
    pub const fn is_between_waves(&self) -> bool {
        self.has_first_wave_spawned()
            && self.total_raiders_alive == 0
            && self.raid_cooldown_ticks > 0
    }
}

/// Vanilla `Raids`.
#[derive(Default)]
pub struct Raids {
    raid_map: BTreeMap<i32, Raid>,
    next_id: i32,
    tick: i32,
    /// Whether the saved data changed since the last write (`SavedData.setDirty`).
    dirty: bool,
}

impl Raids {
    /// Vanilla `Raids()`: a fresh manager starts with no raids and `nextId = 1`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            raid_map: BTreeMap::new(),
            next_id: 1,
            tick: 0,
            dirty: true,
        }
    }

    /// Rebuilds the manager from saved data.
    #[must_use]
    pub fn from_saved(saved: RaidsData) -> Self {
        let mut raids = Self {
            raid_map: BTreeMap::new(),
            next_id: saved.next_id,
            tick: saved.tick,
            dirty: false,
        };

        for record in saved.raids {
            let center = BlockPos::new(record.center[0], record.center[1], record.center[2]);
            let status = RaidStatus::from_serialized_name(&record.status).unwrap_or_default();
            let raid = Raid::from_saved(
                record.started,
                record.active,
                record.ticks_active,
                record.raid_omen_level,
                record.groups_spawned,
                record.raid_cooldown_ticks,
                record.post_raid_ticks,
                record.total_health,
                record.group_count,
                status,
                center,
                record.heroes_of_the_village.into_iter().collect(),
            );
            raids.raid_map.insert(record.id, raid);
        }

        raids
    }

    /// Snapshot of the persisted state.
    #[must_use]
    pub fn to_saved(&self) -> RaidsData {
        RaidsData {
            raids: self
                .raid_map
                .iter()
                .map(|(id, raid)| RaidRecord::from_raid(*id, raid))
                .collect(),
            next_id: self.next_id,
            tick: self.tick,
        }
    }

    /// Whether this saved data is dirty.
    #[must_use]
    pub const fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Vanilla `SavedData.setDirty`.
    pub const fn set_dirty(&mut self) {
        self.dirty = true;
    }

    /// Marks the data as written.
    pub const fn mark_saved(&mut self) {
        self.dirty = false;
    }

    /// Vanilla `Raids.get`.
    #[must_use]
    pub fn get(&self, raid_id: i32) -> Option<&Raid> {
        self.raid_map.get(&raid_id)
    }

    /// Mutable access to one raid.
    pub fn get_mut(&mut self, raid_id: i32) -> Option<&mut Raid> {
        self.raid_map.get_mut(&raid_id)
    }

    /// Vanilla `Raids.getId(raid)` as a snapshot.
    #[must_use]
    pub fn snapshot(&self, raid_id: i32) -> Option<RaidSnapshot> {
        self.raid_map
            .get(&raid_id)
            .map(|raid| raid.snapshot(raid_id))
    }

    /// Vanilla `Raids.getNearbyRaid`.
    #[must_use]
    pub fn get_nearby_raid(&self, pos: BlockPos, max_dist_sqr: i32) -> Option<RaidSnapshot> {
        let mut closest: Option<(i32, i32)> = None;

        for (id, raid) in &self.raid_map {
            if !raid.is_active() {
                continue;
            }
            let distance = block_distance_sqr(raid.center(), pos);
            if distance >= max_dist_sqr {
                continue;
            }
            if closest.is_none_or(|(_, best)| distance < best) {
                closest = Some((*id, distance));
            }
        }

        closest.and_then(|(id, _)| self.snapshot(id))
    }

    /// Vanilla `ServerLevel.getRaidAt`.
    #[must_use]
    pub fn get_raid_at(&self, pos: BlockPos) -> Option<RaidSnapshot> {
        self.get_nearby_raid(pos, VALID_RAID_RADIUS_SQR)
    }

    /// Vanilla `ServerLevel.isRaided`.
    #[must_use]
    pub fn is_raided(&self, pos: BlockPos) -> bool {
        self.get_raid_at(pos).is_some()
    }

    /// Vanilla `Raids.getRaidCentersInChunk`.
    #[must_use]
    pub fn get_raid_centers_in_chunk(&self, chunk_pos: ChunkPos) -> Vec<BlockPos> {
        self.raid_map
            .values()
            .map(Raid::center)
            .filter(|center| ChunkPos::from_block_pos(*center) == chunk_pos)
            .collect()
    }

    /// Vanilla `Raids.tick`.
    pub fn tick(&mut self, world: &Arc<World>) {
        self.tick += 1;
        let raids_enabled = world.get_game_rule(&RAIDS);

        let ids: Vec<i32> = self.raid_map.keys().copied().collect();
        let mut stopped = Vec::new();
        for id in ids {
            let Some(raid) = self.raid_map.get_mut(&id) else {
                continue;
            };

            if !raids_enabled {
                raid.stop();
                raid.sync_bar(world);
            }

            if raid.is_stopped() {
                stopped.push(id);
                continue;
            }

            raid.tick(world, id);
        }

        for id in stopped {
            self.raid_map.remove(&id);
            self.dirty = true;
        }

        if self.tick % SAVED_DIRTY_INTERVAL_TICKS == 0 {
            self.dirty = true;
        }
    }

    /// Vanilla `Raids.createOrExtendRaid`.
    pub fn create_or_extend_raid(
        &mut self,
        world: &World,
        player: &Player,
        raid_position: BlockPos,
    ) -> Option<RaidSnapshot> {
        if player.is_spectator() || !world.get_game_rule(&RAIDS) {
            return None;
        }

        // Vanilla also requires `EnvironmentAttributes.CAN_START_RAID` at `raid_position`.
        // Steel has no per-position environment-attribute system, so the vanilla default
        // (true) applies; this is a reported gap.

        let raid_center = village_raid_center(world, raid_position);
        let raid_id = self.get_or_create_raid(world, raid_center);

        let raid = self.raid_map.get_mut(&raid_id)?;
        if !raid.is_started() || raid.raid_omen_level() < raid.max_raid_omen_level() {
            raid.absorb_raid_omen(player);
        }

        self.dirty = true;
        self.snapshot(raid_id)
    }

    /// Vanilla `Raids.getOrCreateRaid`, returning the manager id of the raid.
    fn get_or_create_raid(&mut self, world: &World, pos: BlockPos) -> i32 {
        if let Some(existing) = self.get_raid_at(pos) {
            return existing.id;
        }

        let next_id = self.unique_id();
        let difficulty = world.difficulty();
        self.raid_map.insert(next_id, Raid::new(pos, difficulty));
        self.dirty = true;
        next_id
    }

    /// Vanilla `Raids.getUniqueId`.
    fn unique_id(&mut self) -> i32 {
        self.next_id += 1;
        self.next_id
    }

    /// Vanilla `Raid.joinRaid` through the manager.
    pub fn join_raid(
        &mut self,
        world: &Arc<World>,
        raid_id: i32,
        raider: &SharedEntity,
        group_number: i32,
        pos: Option<BlockPos>,
        exists: bool,
    ) {
        if let Some(raid) = self.raid_map.get_mut(&raid_id) {
            raid.join_raid(world, raid_id, group_number, raider, pos, exists);
            self.dirty = true;
        }
    }

    /// Vanilla `Raid.removeFromRaid` through the manager.
    pub fn remove_raider(
        &mut self,
        world: &Arc<World>,
        raid_id: i32,
        uuid: Uuid,
        remove_from_total_health: bool,
    ) {
        if let Some(raid) = self.raid_map.get_mut(&raid_id) {
            raid.remove_from_raid(world, uuid, remove_from_total_health);
            self.dirty = true;
        }
    }

    /// Vanilla `Raid.setLeader` through the manager.
    pub fn set_leader(&mut self, raid_id: i32, wave: i32, raider: &SharedEntity) {
        if let Some(raid) = self.raid_map.get_mut(&raid_id) {
            raid.set_leader(wave, raider);
            self.dirty = true;
        }
    }

    /// Vanilla `Raid.removeLeader` through the manager.
    pub fn remove_leader(&mut self, raid_id: i32, wave: i32) {
        if let Some(raid) = self.raid_map.get_mut(&raid_id) {
            raid.remove_leader(wave);
            self.dirty = true;
        }
    }

    /// Vanilla `Raid.addHeroOfTheVillage` through the manager.
    pub fn add_hero_of_the_village(&mut self, raid_id: i32, killer: Uuid) {
        if let Some(raid) = self.raid_map.get_mut(&raid_id) {
            raid.add_hero_of_the_village(killer);
            self.dirty = true;
        }
    }

    /// Vanilla `Raider.readAdditionalSaveData` re-registration: a loaded raider rejoins its
    /// saved raid wave without being respawned.
    pub fn add_saved_raider(
        &mut self,
        raid_id: i32,
        wave: i32,
        world: &Arc<World>,
        raider: Option<SharedEntity>,
    ) {
        let Some(raider) = raider else {
            return;
        };
        let Some(raid) = self.raid_map.get_mut(&raid_id) else {
            return;
        };
        raid.add_wave_mob(world, wave, &raider, false);
        if raider.as_raider().is_some_and(Raider::is_patrol_leader) {
            raid.set_leader(wave, &raider);
        }
        self.dirty = true;
    }

    /// Vanilla `Raids.getId` for a raid currently registered.
    #[must_use]
    pub fn id_of_center(&self, center: BlockPos) -> Option<i32> {
        self.raid_map
            .iter()
            .find(|(_, raid)| raid.center() == center)
            .map(|(id, _)| *id)
    }

    /// Loads the manager from this world's saved data (vanilla `SavedData` load).
    pub async fn load(saved_data: &SavedDataManager, name: SavedDataName) -> Self {
        let data = match saved_data.load_or_default::<RaidsData>(name).await {
            Ok(data) => data,
            Err(error) => {
                log::error!("Failed to load raid saved data: {error}");
                RaidsData::default()
            }
        };
        Self::from_saved(data)
    }

    /// Writes the manager to this world's saved data.
    pub async fn save(&self, saved_data: &SavedDataManager, name: SavedDataName) {
        if let Err(error) = saved_data.save(name, &self.to_saved()).await {
            log::error!("Failed to save raid saved data: {error}");
        }
    }
}

fn block_distance_sqr(a: BlockPos, b: BlockPos) -> i32 {
    let dx = a.x() - b.x();
    let dy = a.y() - b.y();
    let dz = a.z() - b.z();
    dx * dx + dy * dy + dz * dz
}

/// Vanilla `Raids.CODEC` payload: the raid list plus the manager counters.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RaidsData {
    /// Vanilla `Raids.CODEC` field `raids`.
    pub raids: Vec<RaidRecord>,
    /// Vanilla `Raids.CODEC` field `next_id`.
    pub next_id: i32,
    /// Vanilla `Raids.CODEC` field `tick`.
    pub tick: i32,
}

/// Vanilla `Raids.RaidWithId`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RaidRecord {
    /// Vanilla `RaidWithId.CODEC` field `id`.
    pub id: i32,
    /// Vanilla `Raid.MAP_CODEC` field `started`.
    pub started: bool,
    /// Vanilla `Raid.MAP_CODEC` field `active`.
    pub active: bool,
    /// Vanilla `Raid.MAP_CODEC` field `ticks_active`.
    pub ticks_active: i64,
    /// Vanilla `Raid.MAP_CODEC` field `raid_omen_level`.
    pub raid_omen_level: i32,
    /// Vanilla `Raid.MAP_CODEC` field `groups_spawned`.
    pub groups_spawned: i32,
    /// Vanilla `Raid.MAP_CODEC` field `cooldown_ticks`.
    pub raid_cooldown_ticks: i32,
    /// Vanilla `Raid.MAP_CODEC` field `post_raid_ticks`.
    pub post_raid_ticks: i32,
    /// Vanilla `Raid.MAP_CODEC` field `total_health`.
    pub total_health: f32,
    /// Vanilla `Raid.MAP_CODEC` field `group_count`.
    pub group_count: i32,
    /// Vanilla `Raid.MAP_CODEC` field `status`.
    pub status: String,
    /// Vanilla `Raid.MAP_CODEC` field `center`, stored as `[x, y, z]`.
    pub center: [i32; 3],
    /// Vanilla `Raid.MAP_CODEC` field `heroes_of_the_village`.
    pub heroes_of_the_village: Vec<Uuid>,
}

impl RaidRecord {
    fn from_raid(id: i32, raid: &Raid) -> Self {
        let center = raid.center();
        Self {
            id,
            started: raid.is_started(),
            active: raid.is_active(),
            ticks_active: raid.ticks_active(),
            raid_omen_level: raid.raid_omen_level(),
            groups_spawned: raid.groups_spawned(),
            raid_cooldown_ticks: raid.raid_cooldown_ticks(),
            post_raid_ticks: raid.post_raid_ticks(),
            total_health: raid.total_health(),
            group_count: raid.group_count(),
            status: raid.status().serialized_name().to_owned(),
            center: [center.x(), center.y(), center.z()],
            heroes_of_the_village: raid.heroes_of_the_village().to_vec(),
        }
    }
}
