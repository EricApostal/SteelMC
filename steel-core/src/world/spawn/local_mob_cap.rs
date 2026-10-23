//! Vanilla `LocalMobCapCalculator`: per-chunk mob caps shared between the
//! players close enough to spawn into that chunk.

use std::sync::Arc;

use rustc_hash::FxHashMap;
use steel_registry::entity_type::MobCategory;
use steel_utils::ChunkPos;

use crate::entity::Entity as _;
use crate::player::Player;
use crate::world::World;

/// Vanilla `ChunkMap.playerIsCloseEnoughForSpawning` squared radius.
const PLAYER_SPAWN_DISTANCE_SQR: f64 = 16384.0;

/// Vanilla `LocalMobCapCalculator.MobCounts`.
#[derive(Default)]
struct MobCounts {
    counts: FxHashMap<MobCategory, i32>,
}

impl MobCounts {
    /// Vanilla `MobCounts.add`.
    fn add(&mut self, category: MobCategory) {
        *self.counts.entry(category).or_insert(0) += 1;
    }

    /// Vanilla `MobCounts.canSpawn`.
    fn can_spawn(&self, category: MobCategory) -> bool {
        self.counts.get(&category).copied().unwrap_or(0) < category.max_instances_per_chunk()
    }
}

/// Vanilla `LocalMobCapCalculator`.
pub struct LocalMobCapCalculator<'a> {
    world: &'a Arc<World>,
    players_near_chunk: FxHashMap<ChunkPos, Vec<Arc<Player>>>,
    player_mob_counts: FxHashMap<i32, MobCounts>,
}

impl<'a> LocalMobCapCalculator<'a> {
    /// Vanilla `LocalMobCapCalculator(ChunkMap)`.
    #[must_use]
    pub fn new(world: &'a Arc<World>) -> Self {
        Self {
            world,
            players_near_chunk: FxHashMap::default(),
            player_mob_counts: FxHashMap::default(),
        }
    }

    /// Vanilla `LocalMobCapCalculator.getPlayersNear`, backed by
    /// `ChunkMap.getPlayersCloseForSpawning`.
    fn players_near(&mut self, pos: ChunkPos) -> Vec<Arc<Player>> {
        if let Some(cached) = self.players_near_chunk.get(&pos) {
            return cached.clone();
        }
        let players = players_close_for_spawning(self.world, pos);
        self.players_near_chunk.insert(pos, players.clone());
        players
    }

    /// Vanilla `LocalMobCapCalculator.addMob`.
    pub fn add_mob(&mut self, pos: ChunkPos, category: MobCategory) {
        let players = self.players_near(pos);
        for player in players {
            self.player_mob_counts
                .entry(player.id())
                .or_default()
                .add(category);
        }
    }

    /// Vanilla `LocalMobCapCalculator.canSpawn`.
    pub fn can_spawn(&mut self, category: MobCategory, pos: ChunkPos) -> bool {
        let players = self.players_near(pos);
        for player in players {
            match self.player_mob_counts.get(&player.id()) {
                Some(counts) if !counts.can_spawn(category) => {}
                _ => return true,
            }
        }

        false
    }
}

/// Vanilla `ChunkMap.getPlayersCloseForSpawning`.
#[must_use]
pub fn players_close_for_spawning(world: &World, pos: ChunkPos) -> Vec<Arc<Player>> {
    let center_x = f64::from((pos.0.x << 4) + 8);
    let center_z = f64::from((pos.0.y << 4) + 8);
    let mut players = Vec::new();
    world.players.iter_players(|_, player| {
        if player.is_spectator() {
            return true;
        }

        let player_pos = player.position();
        let dx = center_x - player_pos.x;
        let dz = center_z - player_pos.z;
        if dx * dx + dz * dz < PLAYER_SPAWN_DISTANCE_SQR {
            players.push(Arc::clone(player));
        }
        true
    });
    players
}
