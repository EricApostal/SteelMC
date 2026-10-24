//! Vanilla `CatSpawner`: periodically spawns a stray cat near a village or a
//! swamp-hut/witch-hut structure.

use std::sync::Arc;

use steel_registry::vanilla_entities;
use steel_registry::vanilla_poi_types;
use steel_registry::{REGISTRY, RegistryExt as _};
use steel_utils::random::Random as _;
use steel_utils::{BlockPos, WorldAabb};

use crate::entity::Entity as _;
use crate::entity::{EntitySpawnReason, create_entity_instance};
use crate::poi::OccupationStatus;
use crate::world::World;
use crate::world::custom_spawner::CustomSpawner;
use crate::world::spawn::natural::runtime_random;
use crate::world::spawn::placement::SpawnPlacements;

/// Vanilla `CatSpawner.TICK_DELAY`.
const TICK_DELAY: i32 = 1200;
/// Vanilla `CatSpawner`'s chunk pre-check radius.
const CHUNK_CHECK_RADIUS: i32 = 10;
/// Vanilla `CatSpawner.spawnInVillage` radius.
const VILLAGE_RADIUS: i32 = 48;
/// Vanilla `CatSpawner.spawnInVillage` occupied-home threshold.
const VILLAGE_HOME_THRESHOLD: i64 = 4;
/// Vanilla `CatSpawner.spawnInVillage` nearby-cat limit.
const VILLAGE_CAT_LIMIT: usize = 5;
/// Vanilla `CatSpawner.spawnInHut` radius.
const HUT_RADIUS: f64 = 16.0;
/// Vanilla `CatSpawner` cross-radius for the cat search box.
const CAT_SEARCH_RADIUS_Y: f64 = 8.0;

/// Vanilla `CatSpawner`.
pub struct CatSpawner {
    next_tick: i32,
}

impl Default for CatSpawner {
    fn default() -> Self {
        Self::new()
    }
}

impl CatSpawner {
    /// Creates the spawner with vanilla's default delay.
    #[must_use]
    pub const fn new() -> Self {
        Self { next_tick: 0 }
    }

    /// Vanilla `CatSpawner.tick`.
    pub fn run_tick(&mut self, world: &Arc<World>) {
        self.next_tick -= 1;
        if self.next_tick > 0 {
            return;
        }
        self.next_tick = TICK_DELAY;

        let mut players = Vec::new();
        world.players.iter_players(|_, player| {
            players.push(Arc::clone(player));
            true
        });
        if players.is_empty() {
            return;
        }

        let mut random = runtime_random();
        let index = random.next_i32_bounded(players.len() as i32) as usize;
        let player_pos = players[index].block_position();
        let x = (8 + random.next_i32_bounded(24)) * if random.next_bool() { -1 } else { 1 };
        let z = (8 + random.next_i32_bounded(24)) * if random.next_bool() { -1 } else { 1 };
        let spawn_pos = player_pos.offset(x, 0, z);

        if !world.has_chunks_at(
            spawn_pos.x() - CHUNK_CHECK_RADIUS,
            spawn_pos.z() - CHUNK_CHECK_RADIUS,
            spawn_pos.x() + CHUNK_CHECK_RADIUS,
            spawn_pos.z() + CHUNK_CHECK_RADIUS,
        ) {
            return;
        }

        if !SpawnPlacements::is_spawn_position_ok(&vanilla_entities::CAT, world, spawn_pos) {
            return;
        }

        if world.is_close_to_village(spawn_pos, 2) {
            self.spawn_in_village(world, spawn_pos);
        }
    }

    /// Vanilla `CatSpawner.spawnInVillage`.
    fn spawn_in_village(&self, world: &Arc<World>, spawn_pos: BlockPos) {
        let Some(home_type) = REGISTRY.poi_types.id_from_key(&vanilla_poi_types::HOME.key) else {
            return;
        };
        let occupied_homes = world
            .poi_storage
            .lock()
            .get_in_square(
                &|type_id| type_id == home_type,
                spawn_pos,
                VILLAGE_RADIUS,
                OccupationStatus::Occupied,
            )
            .len() as i64;
        if occupied_homes <= VILLAGE_HOME_THRESHOLD {
            return;
        }

        let search = WorldAabb::new(
            f64::from(spawn_pos.x()) - VILLAGE_RADIUS as f64,
            f64::from(spawn_pos.y()) - CAT_SEARCH_RADIUS_Y,
            f64::from(spawn_pos.z()) - VILLAGE_RADIUS as f64,
            f64::from(spawn_pos.x()) + f64::from(VILLAGE_RADIUS) + 1.0,
            f64::from(spawn_pos.y()) + CAT_SEARCH_RADIUS_Y + 1.0,
            f64::from(spawn_pos.z()) + f64::from(VILLAGE_RADIUS) + 1.0,
        );
        let cats = world
            .get_entities_in_aabb(&search)
            .into_iter()
            .filter(|entity| std::ptr::eq(entity.entity_type(), &vanilla_entities::CAT))
            .count();
        if cats < VILLAGE_CAT_LIMIT {
            self.spawn_cat(world, spawn_pos, false);
        }
    }

    /// Vanilla `CatSpawner.spawnCat`.
    fn spawn_cat(&self, world: &Arc<World>, spawn_pos: BlockPos, make_persistent: bool) {
        let position = glam::DVec3::new(
            f64::from(spawn_pos.x()) + 0.5,
            f64::from(spawn_pos.y()),
            f64::from(spawn_pos.z()) + 0.5,
        );
        let Ok(cat) = create_entity_instance(world, &vanilla_entities::CAT, position) else {
            return;
        };

        let Some(mob) = cat.as_mob() else {
            return;
        };
        mob.finalize_spawn(world, EntitySpawnReason::Natural, None);
        if make_persistent {
            mob.set_persistence_required();
        }

        cat.snap_to(position, 0.0, 0.0);
        let _ = world.try_add_entity(cat);
    }
}

impl CustomSpawner for CatSpawner {
    fn tick(&mut self, world: &Arc<World>) {
        self.run_tick(world);
    }
}
