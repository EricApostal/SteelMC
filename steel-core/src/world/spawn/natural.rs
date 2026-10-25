//! Vanilla `NaturalSpawner`: the natural-spawn driver and per-category mob caps.
//!
//! The chunk tick mirrors `ServerChunkCache.tickChunks`: build a [`SpawnState`]
//! from the live entity set, filter the categories by the global and local caps,
//! then run `spawnForChunk` over the entity-ticking chunks that have a player
//! close enough to spawn for.

use std::cell::RefCell;
use std::ptr;
use std::sync::Arc;

use glam::DVec3;
use rustc_hash::FxHashMap;
use steel_registry::REGISTRY;
use steel_registry::RegistryExt as _;
use steel_registry::biome::{Biome, BiomeRef, SpawnCost, SpawnerData};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::entity_type::{EntityTypeRef, MobCategory};
use steel_registry::vanilla_biome_tags::BiomeTag;
use steel_registry::vanilla_game_rules::{SPAWN_MOBS, SPAWN_MONSTERS};

use steel_utils::random::{Random as _, RandomSource, legacy_random::LegacyRandom};
use steel_utils::{BlockPos, ChunkPos, WorldAabb};

use super::local_mob_cap::{LocalMobCapCalculator, players_close_for_spawning};
use super::placement::SpawnPlacements;
use super::structure_spawns::{
    FORTRESS_ENEMIES, is_in_nether_fortress_bounds, structure_spawns_at,
};
use crate::chunk::full_chunk::FullChunkRef;
use crate::chunk::heightmap::HeightmapType;
use crate::entity::Entity as _;
use crate::entity::Mob;
use crate::entity::SharedEntity;
use crate::entity::{EntitySpawnReason, create_entity_instance};
use crate::physics::CollisionWorld as _;
use crate::physics::collision::WorldCollisionProvider;
use crate::world::LevelReader as _;
use crate::world::World;
use crate::world::is_redstone_conductor;
use crate::worldgen::ChunkGeneratorType;
use crate::worldgen::region::WorldGenRegion;

/// Vanilla `NaturalSpawner.MIN_SPAWN_DISTANCE`.
const MIN_SPAWN_DISTANCE: f64 = 24.0;
/// Vanilla `NaturalSpawner.MAGIC_NUMBER`.
const MAGIC_NUMBER: i32 = 289;
/// Vanilla's per-attempt coordinate spread (`nextInt(6) - nextInt(6)`).
const SPAWN_ATTEMPT_SPREAD: i32 = 6;
/// Vanilla's outer spawn group loop bound.
const SPAWN_GROUP_ATTEMPTS: i32 = 3;
/// Vanilla's chunk-generation placement attempt count.
const CHUNK_GENERATION_ATTEMPTS: i32 = 4;
/// Vanilla `ServerChunkCache.tickChunks`: persistent categories every 400 ticks.
const PERSISTENT_SPAWN_INTERVAL: i64 = 400;

/// Vanilla `NaturalSpawner.SPAWNING_CATEGORIES`.
#[must_use]
pub const fn spawning_categories() -> [MobCategory; 7] {
    [
        MobCategory::Monster,
        MobCategory::Creature,
        MobCategory::Ambient,
        MobCategory::Axolotls,
        MobCategory::UndergroundWaterCreature,
        MobCategory::WaterCreature,
        MobCategory::WaterAmbient,
    ]
}

/// Vanilla `NaturalSpawner`. Steel has no per-instance natural spawner state, so
/// this is a namespace for the vanilla static driver methods.
pub struct NaturalSpawner;

/// Vanilla `PotentialCalculator`.
#[derive(Default)]
pub struct PotentialCalculator {
    charges: Vec<(BlockPos, f64)>,
}

impl PotentialCalculator {
    /// Creates an empty calculator.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Vanilla `PotentialCalculator.addCharge`.
    pub fn add_charge(&mut self, pos: BlockPos, charge: f64) {
        if charge != 0.0 {
            self.charges.push((pos, charge));
        }
    }

    /// Vanilla `PotentialCalculator.getPotentialEnergyChange`.
    #[must_use]
    pub fn potential_energy_change(&self, pos: BlockPos, charge: f64) -> f64 {
        if charge == 0.0 {
            return 0.0;
        }

        let mut potential_change = 0.0;
        for (charge_pos, point_charge) in &self.charges {
            let dx = f64::from(charge_pos.x() - pos.x());
            let dy = f64::from(charge_pos.y() - pos.y());
            let dz = f64::from(charge_pos.z() - pos.z());
            let dist_sqr = dx * dx + dy * dy + dz * dz;
            potential_change += if dist_sqr == 0.0 {
                f64::INFINITY
            } else {
                point_charge / dist_sqr.sqrt()
            };
        }

        potential_change * charge
    }
}

/// Vanilla `NaturalSpawner.SpawnState`.
pub struct SpawnState<'a> {
    spawnable_chunk_count: i32,
    mob_category_counts: FxHashMap<MobCategory, i32>,
    spawn_potential: PotentialCalculator,
    local_mob_cap_calculator: LocalMobCapCalculator<'a>,
    last_checked_pos: Option<BlockPos>,
    last_checked_type: Option<EntityTypeRef>,
    last_charge: f64,
}

impl<'a> SpawnState<'a> {
    const fn new(
        spawnable_chunk_count: i32,
        mob_category_counts: FxHashMap<MobCategory, i32>,
        spawn_potential: PotentialCalculator,
        local_mob_cap_calculator: LocalMobCapCalculator<'a>,
    ) -> Self {
        Self {
            spawnable_chunk_count,
            mob_category_counts,
            spawn_potential,
            local_mob_cap_calculator,
            last_checked_pos: None,
            last_checked_type: None,
            last_charge: 0.0,
        }
    }

    /// Vanilla `SpawnState.canSpawn`.
    fn can_spawn(&mut self, entity_type: EntityTypeRef, level: &World, pos: BlockPos) -> bool {
        self.last_checked_pos = Some(pos);
        self.last_checked_type = Some(entity_type);

        let Some(cost) = mob_spawn_cost(level, pos, entity_type) else {
            self.last_charge = 0.0;
            return true;
        };

        self.last_charge = cost.charge;
        self.spawn_potential
            .potential_energy_change(pos, cost.charge)
            <= cost.energy_budget
    }

    /// Vanilla `SpawnState.afterSpawn`.
    fn after_spawn(&mut self, mob: &SharedEntity, level: &World) {
        let entity_type = mob.entity_type();
        let pos = mob.block_position();
        let charge = if Some(pos) == self.last_checked_pos
            && self
                .last_checked_type
                .is_some_and(|checked| ptr::eq(checked, entity_type))
        {
            self.last_charge
        } else {
            mob_spawn_cost(level, pos, entity_type).map_or(0.0, |cost| cost.charge)
        };

        self.spawn_potential.add_charge(pos, charge);
        let category = entity_type.mob_category;
        *self.mob_category_counts.entry(category).or_insert(0) += 1;
        self.local_mob_cap_calculator
            .add_mob(ChunkPos::from_block_pos(pos), category);
    }

    /// Vanilla `SpawnState.canSpawnForCategoryGlobal`.
    #[must_use]
    pub fn can_spawn_for_category_global(&self, category: MobCategory) -> bool {
        let max_mob_count =
            category.max_instances_per_chunk() * self.spawnable_chunk_count / MAGIC_NUMBER;
        self.mob_category_counts
            .get(&category)
            .copied()
            .unwrap_or(0)
            < max_mob_count
    }

    /// Vanilla `SpawnState.canSpawnForCategoryLocal`.
    pub fn can_spawn_for_category_local(&mut self, category: MobCategory, pos: ChunkPos) -> bool {
        self.local_mob_cap_calculator.can_spawn(category, pos)
    }
}

impl NaturalSpawner {
    /// Vanilla `NaturalSpawner.createState`.
    pub fn create_state(world: &Arc<World>, spawnable_chunk_count: i32) -> SpawnState<'_> {
        let mut spawn_potential = PotentialCalculator::new();
        let mut mob_counts: FxHashMap<MobCategory, i32> = FxHashMap::default();
        let mut local_mob_cap_calculator = LocalMobCapCalculator::new(world);

        for entity in world.entity_manager.get_accessible_entities() {
            let category = entity.entity_type().mob_category;
            if category == MobCategory::Misc {
                continue;
            }

            if let Some(mob) = entity.as_mob()
                && (mob.is_persistence_required() || mob.requires_custom_persistence())
            {
                continue;
            }

            let pos = entity.block_position();
            if let Some(cost) = mob_spawn_cost(world, pos, entity.entity_type()) {
                spawn_potential.add_charge(pos, cost.charge);
            }

            if entity.as_mob().is_some() {
                local_mob_cap_calculator.add_mob(ChunkPos::from_block_pos(pos), category);
            }
            *mob_counts.entry(category).or_insert(0) += 1;
        }

        SpawnState::new(
            spawnable_chunk_count,
            mob_counts,
            spawn_potential,
            local_mob_cap_calculator,
        )
    }

    /// Vanilla `NaturalSpawner.getFilteredSpawningCategories`.
    #[must_use]
    pub fn filtered_spawning_categories(
        state: &SpawnState<'_>,
        spawn_enemies: bool,
        spawn_persistent: bool,
    ) -> Vec<MobCategory> {
        spawning_categories()
            .into_iter()
            .filter(|category| {
                (spawn_enemies || category.is_friendly())
                    && (spawn_persistent || !category.is_persistent())
                    && state.can_spawn_for_category_global(*category)
            })
            .collect()
    }

    /// Vanilla `NaturalSpawner.spawnForChunk`.
    pub fn spawn_for_chunk(
        world: &Arc<World>,
        chunk_pos: ChunkPos,
        chunk: FullChunkRef<'_>,
        state: &mut SpawnState<'_>,
        categories: &[MobCategory],
        random: &mut RandomSource,
    ) {
        for category in categories {
            if !state.can_spawn_for_category_local(*category, chunk_pos) {
                continue;
            }

            {
                // `can_spawn` and `after_spawn` both take `&mut SpawnState`, and both
                // callbacks stay alive for the whole spawn pass, so the state is shared
                // through a `RefCell` instead of being borrowed twice.
                let state_cell = RefCell::new(&mut *state);
                let mut extra_test = |entity_type, test_world: &World, pos| {
                    state_cell
                        .borrow_mut()
                        .can_spawn(entity_type, test_world, pos)
                };
                let mut spawn_callback = |mob: &SharedEntity, callback_world: &World| {
                    state_cell.borrow_mut().after_spawn(mob, callback_world);
                };
                Self::spawn_category_for_chunk(
                    *category,
                    world,
                    chunk_pos,
                    chunk,
                    &mut extra_test,
                    &mut spawn_callback,
                    random,
                );
            }
        }
    }

    /// Vanilla `NaturalSpawner.spawnCategoryForChunk`.
    pub fn spawn_category_for_chunk(
        category: MobCategory,
        world: &Arc<World>,
        chunk_pos: ChunkPos,
        chunk: FullChunkRef<'_>,
        extra_test: &mut impl FnMut(EntityTypeRef, &World, BlockPos) -> bool,
        spawn_callback: &mut impl FnMut(&SharedEntity, &World),
        random: &mut RandomSource,
    ) {
        let start = random_pos_within(world, chunk_pos, chunk, random);
        if start.y() < world.get_min_y() + 1 {
            return;
        }

        Self::spawn_category_for_position(
            category,
            world,
            chunk_pos,
            chunk,
            start,
            extra_test,
            spawn_callback,
            random,
        );
    }

    /// Vanilla `NaturalSpawner.spawnCategoryForPosition`.
    #[expect(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "mirrors vanilla NaturalSpawner.spawnCategoryForPosition one to one"
    )]
    pub fn spawn_category_for_position(
        category: MobCategory,
        world: &Arc<World>,
        chunk_pos: ChunkPos,
        chunk: FullChunkRef<'_>,
        start: BlockPos,
        extra_test: &mut impl FnMut(EntityTypeRef, &World, BlockPos) -> bool,
        spawn_callback: &mut impl FnMut(&SharedEntity, &World),
        random: &mut RandomSource,
    ) {
        let y_start = start.y();
        let start_state = chunk.get_block_state(start);
        if is_redstone_conductor(world, start_state, start) {
            return;
        }

        let mut cluster_size = 0;

        for _ in 0..SPAWN_GROUP_ATTEMPTS {
            let mut x = start.x();
            let mut z = start.z();
            let mut current_spawn_data: Option<EntityTypeRef> = None;
            let mut group_data = None;
            let mut max = mth_ceil(random.next_f32() * 4.0);
            let mut group_size = 0;
            let mut attempts = 0;

            // Vanilla reads the attempt bound fresh each iteration: the first valid spawn
            // replaces the `ceil` value with the sampled group size below, so the loop range
            // updates while iterating. A Rust `for` range would freeze the bound instead.
            loop {
                if attempts >= max {
                    break;
                }
                // For-update placement: increments before the body so `continue` still
                // consumes the attempt, exactly like the Java loop.
                attempts += 1;
                x += random.next_i32_bounded(SPAWN_ATTEMPT_SPREAD)
                    - random.next_i32_bounded(SPAWN_ATTEMPT_SPREAD);
                z += random.next_i32_bounded(SPAWN_ATTEMPT_SPREAD)
                    - random.next_i32_bounded(SPAWN_ATTEMPT_SPREAD);
                let pos = BlockPos::new(x, y_start, z);
                let center_x = f64::from(x) + 0.5;
                let center_z = f64::from(z) + 0.5;
                let center = DVec3::new(center_x, f64::from(y_start), center_z);

                let Some(player) =
                    world.nearest_player(center, f64::INFINITY, |player| !player.is_spectator())
                else {
                    continue;
                };

                let player_distance_sqr = player.position().distance_squared(center);
                if !is_right_distance_to_player_and_spawn_point(
                    world,
                    chunk_pos,
                    pos,
                    player_distance_sqr,
                ) {
                    continue;
                }

                if current_spawn_data.is_none() {
                    let Some(next) = random_spawn_mob_at(world, category, random, pos) else {
                        break;
                    };
                    current_spawn_data = Some(next.0);
                    max = sample_count(next.1, random);
                }

                let Some(entity_type) = current_spawn_data else {
                    break;
                };
                if !is_valid_spawn_position_for_type(
                    world,
                    category,
                    entity_type,
                    pos,
                    player_distance_sqr,
                    random,
                ) || !extra_test(entity_type, world, pos)
                {
                    continue;
                }

                let Some(mob) = mob_for_spawn(world, entity_type) else {
                    return;
                };

                let yaw = random.next_f32() * 360.0;
                mob.snap_to(center, yaw, 0.0);
                if let Some(mob_trait) = mob.as_mob() {
                    mob_trait.set_y_head_rot(yaw);
                    mob_trait.set_y_body_rot(yaw);

                    if !is_valid_position_for_mob(
                        world,
                        &mob,
                        player_distance_sqr,
                        EntitySpawnReason::Natural,
                    ) {
                        continue;
                    }

                    group_data =
                        mob_trait.finalize_spawn(world, EntitySpawnReason::Natural, group_data);
                } else {
                    continue;
                }

                cluster_size += 1;
                group_size += 1;
                if world.try_add_entity(Arc::clone(&mob)).is_err() {
                    return;
                }
                spawn_callback(&mob, world);

                if cluster_size >= max_spawn_cluster_size(&mob) {
                    return;
                }

                if is_max_group_size_reached(&mob, group_size) {
                    break;
                }
            }
        }
    }
}

/// Vanilla `Mth.ceil(float)`.
const fn mth_ceil(value: f32) -> i32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "vanilla narrows the ceiling result to an int"
    )]
    let result = value.ceil() as i32;
    result
}

/// The world's natural-spawn RNG.
///
/// Vanilla's `Level.random` is a `LegacyRandomSource` seeded from
/// `ThreadLocalRandom`, so natural spawning is not seed-deterministic; Steel
/// matches that with a runtime-seeded legacy source.
#[must_use]
pub fn runtime_random() -> RandomSource {
    RandomSource::Legacy(LegacyRandom::from_seed(rand::random::<u64>()))
}

/// Vanilla `NaturalSpawner.getRandomPosWithin`.
fn random_pos_within(
    world: &World,
    chunk_pos: ChunkPos,
    chunk: FullChunkRef<'_>,
    random: &mut RandomSource,
) -> BlockPos {
    let x = (chunk_pos.0.x << 4) + random.next_i32_bounded(16);
    let z = (chunk_pos.0.y << 4) + random.next_i32_bounded(16);
    // Vanilla adds one to the heightmap lookup on top of the heightmap's own
    // "first empty y" semantics.
    let top_empty_y = chunk.get_height(
        HeightmapType::WorldSurface,
        (x & 15) as usize,
        (z & 15) as usize,
    ) + 1;
    let y = random.next_i32_between(world.get_min_y(), top_empty_y);
    BlockPos::new(x, y, z)
}

/// Vanilla `NaturalSpawner.isRightDistanceToPlayerAndSpawnPoint`.
fn is_right_distance_to_player_and_spawn_point(
    world: &Arc<World>,
    chunk_pos: ChunkPos,
    pos: BlockPos,
    player_distance_sqr: f64,
) -> bool {
    if player_distance_sqr <= MIN_SPAWN_DISTANCE * MIN_SPAWN_DISTANCE {
        return false;
    }

    let respawn_data = world
        .level_data
        .read()
        .data()
        .respawn_data_or_local(&world.key);
    if respawn_data.global_pos.dimension == world.key {
        let respawn = respawn_data.global_pos.pos;
        let center = DVec3::new(
            f64::from(pos.x()) + 0.5,
            f64::from(pos.y()),
            f64::from(pos.z()) + 0.5,
        );
        let anchor = DVec3::new(
            f64::from(respawn.x()) + 0.5,
            f64::from(respawn.y()),
            f64::from(respawn.z()) + 0.5,
        );
        if anchor.distance_squared(center) < MIN_SPAWN_DISTANCE * MIN_SPAWN_DISTANCE {
            return false;
        }
    }

    let target = ChunkPos::from_block_pos(pos);
    target == chunk_pos || world.can_spawn_entities_in_chunk(target)
}

/// Vanilla `NaturalSpawner.isValidSpawnPostitionForType`.
fn is_valid_spawn_position_for_type(
    world: &Arc<World>,
    category: MobCategory,
    entity_type: EntityTypeRef,
    pos: BlockPos,
    player_distance_sqr: f64,
    random: &mut RandomSource,
) -> bool {
    if entity_type.mob_category == MobCategory::Misc {
        return false;
    }

    let despawn_distance = f64::from(category.despawn_distance());
    if !entity_type.can_spawn_far_from_player
        && player_distance_sqr > despawn_distance * despawn_distance
    {
        return false;
    }

    if !entity_type.summonable || !can_spawn_mob_at(world, category, entity_type, pos) {
        return false;
    }

    if !world.is_within_spawn_border(pos)
        || !SpawnPlacements::is_spawn_position_ok(entity_type, world, pos)
    {
        return false;
    }

    if !SpawnPlacements::check_spawn_rules(
        entity_type,
        world,
        EntitySpawnReason::Natural,
        pos,
        random,
    ) {
        return false;
    }

    world.no_collision(spawn_aabb_at(
        entity_type,
        f64::from(pos.x()) + 0.5,
        f64::from(pos.y()),
        f64::from(pos.z()) + 0.5,
    ))
}

/// Vanilla `NaturalSpawner.getMobForSpawn`.
fn mob_for_spawn(world: &Arc<World>, entity_type: EntityTypeRef) -> Option<SharedEntity> {
    match create_entity_instance(world, entity_type, DVec3::ZERO) {
        Ok(entity) if entity.as_mob().is_some() => Some(entity),
        Ok(_) => {
            log::warn!("Can't spawn entity of type: {}", entity_type.key);
            None
        }
        Err(error) => {
            log::warn!("Failed to create mob {}: {error:?}", entity_type.key);
            None
        }
    }
}

/// Vanilla `NaturalSpawner.isValidPositionForMob`.
fn is_valid_position_for_mob(
    world: &Arc<World>,
    mob: &SharedEntity,
    player_distance_sqr: f64,
    spawn_reason: EntitySpawnReason,
) -> bool {
    let despawn_distance = f64::from(mob.entity_type().mob_category.despawn_distance());
    let Some(mob_trait) = mob.as_mob() else {
        return false;
    };

    if player_distance_sqr > despawn_distance * despawn_distance
        && mob_trait.remove_when_far_away(player_distance_sqr)
    {
        return false;
    }

    check_instance_spawn_rules(mob, world, spawn_reason) && mob_trait.check_spawn_obstruction(world)
}

/// Vanilla `PathfinderMob.checkSpawnRules` layered on `Mob.checkSpawnRules`.
///
/// Vanilla overrides the `Mob` default in `PathfinderMob`; Steel's
/// `PathfinderMob` is a capability trait rather than a supertrait of the
/// object-safe `Mob` vtable, so the spawner dispatches on the capability.
fn check_instance_spawn_rules(
    mob: &SharedEntity,
    world: &Arc<World>,
    spawn_reason: EntitySpawnReason,
) -> bool {
    let Some(mob_trait) = mob.as_mob() else {
        return false;
    };
    if !mob_trait.check_spawn_rules(world, spawn_reason) {
        return false;
    }

    mob.as_pathfinder_mob()
        .is_none_or(|pathfinder| pathfinder.get_walk_target_value(mob.block_position()) >= 0.0)
}

/// Vanilla `Mob.getMaxSpawnClusterSize`.
fn max_spawn_cluster_size(mob: &SharedEntity) -> i32 {
    mob.as_mob().map_or(4, Mob::max_spawn_cluster_size)
}

/// Vanilla `Mob.isMaxGroupSizeReached`.
fn is_max_group_size_reached(mob: &SharedEntity, group_size: i32) -> bool {
    mob.as_mob()
        .is_some_and(|mob_trait| mob_trait.is_max_group_size_reached(group_size))
}

/// Vanilla `NaturalSpawner.mobsAt`.
///
/// Vanilla consults the nether-fortress enemy list first, then the structure
/// spawn overrides, and falls back to the biome's spawn list.
#[must_use]
pub fn mobs_at(
    world: &World,
    category: MobCategory,
    pos: BlockPos,
) -> Option<&'static [SpawnerData]> {
    if is_in_nether_fortress_bounds(world, pos, category) {
        return Some(FORTRESS_ENEMIES);
    }

    structure_spawns_at(world, category, pos).or_else(|| biome_mobs_at(world, category, pos))
}

/// Vanilla's `EnvironmentAttributes.NATURAL_MOB_SPAWNS` fallback: the biome's
/// spawn list for `category`.
fn biome_mobs_at(
    world: &World,
    category: MobCategory,
    pos: BlockPos,
) -> Option<&'static [SpawnerData]> {
    world
        .biome_at(pos)
        .and_then(|biome| biome.spawners.get(category.name()))
        .map(Vec::as_slice)
}

/// Vanilla `NaturalSpawner.canSpawnMobAt`.
fn can_spawn_mob_at(
    world: &World,
    category: MobCategory,
    entity_type: EntityTypeRef,
    pos: BlockPos,
) -> bool {
    mobs_at(world, category, pos)
        .is_some_and(|spawns| spawns.iter().any(|spawn| resolves_to(spawn, entity_type)))
}

fn resolves_to(spawn: &SpawnerData, entity_type: EntityTypeRef) -> bool {
    REGISTRY
        .entity_types
        .by_key(&spawn.entity_type)
        .is_some_and(|resolved| ptr::eq(resolved, entity_type))
}

/// Vanilla `NaturalSpawner.getRandomSpawnMobAt`.
fn random_spawn_mob_at<'b>(
    world: &'b World,
    category: MobCategory,
    random: &mut RandomSource,
    pos: BlockPos,
) -> Option<(EntityTypeRef, &'b SpawnerData)> {
    if category == MobCategory::WaterAmbient
        && world
            .biome_at(pos)
            .is_some_and(|biome| Biome::has_tag(biome, &BiomeTag::REDUCE_WATER_AMBIENT_SPAWNS))
        && random.next_f32() < 0.98
    {
        return None;
    }

    let spawns = mobs_at(world, category, pos)?;
    let references: Vec<&SpawnerData> = spawns.iter().collect();
    let spawn = sample_weighted(&references, random)?;
    let entity_type = REGISTRY.entity_types.by_key(&spawn.entity_type)?;
    Some((entity_type, spawn))
}

/// Vanilla `WeightedList.getRandom`.
fn sample_weighted<'b>(
    entries: &[&'b SpawnerData],
    random: &mut RandomSource,
) -> Option<&'b SpawnerData> {
    let total: i32 = entries.iter().map(|entry| entry.weight).sum();
    if total <= 0 {
        return None;
    }

    let mut roll = random.next_i32_bounded(total);
    for entry in entries {
        roll -= entry.weight;
        if roll < 0 {
            return Some(entry);
        }
    }

    entries.last().copied()
}

/// Vanilla `SpawnerData.count().sample`.
fn sample_count(spawn: &SpawnerData, random: &mut RandomSource) -> i32 {
    if spawn.min_count == spawn.max_count {
        spawn.min_count
    } else {
        random.next_i32_between(spawn.min_count, spawn.max_count)
    }
}

/// Vanilla `MobSpawnSettings.getMobSpawnCost`.
#[must_use]
pub fn mob_spawn_cost(
    world: &World,
    pos: BlockPos,
    entity_type: EntityTypeRef,
) -> Option<&'static SpawnCost> {
    world.biome_at(pos)?.spawn_costs.get(&entity_type.key)
}

impl NaturalSpawner {
    /// Vanilla `ChunkGenerator.spawnOriginalMobs` for the generation stage.
    ///
    /// `FlatLevelSource` and the debug generators spawn nothing during
    /// generation, exactly like their vanilla overrides.
    pub fn spawn_original_mobs(
        world: &Arc<World>,
        generator: &ChunkGeneratorType,
        region: &WorldGenRegion<'_>,
        random: &mut RandomSource,
    ) {
        if !matches!(
            generator,
            ChunkGeneratorType::Overworld(_)
                | ChunkGeneratorType::Nether(_)
                | ChunkGeneratorType::End(_)
        ) {
            return;
        }

        // Vanilla: `center.getWorldPosition().atY(worldGenRegion.getMaxY())`,
        // where `getMaxY()` is the inclusive top build height.
        let chunk_pos = region.center();
        let source_pos = BlockPos::new(
            chunk_pos.0.x << 4,
            region.max_y_exclusive() - 1,
            chunk_pos.0.y << 4,
        );
        Self::spawn_mobs_for_chunk_generation(world, region, source_pos, chunk_pos, random);
    }

    /// Vanilla `NaturalSpawner.spawnMobsForChunkGeneration`.
    pub fn spawn_mobs_for_chunk_generation(
        world: &Arc<World>,
        region: &WorldGenRegion<'_>,
        source_pos: BlockPos,
        chunk_pos: ChunkPos,
        random: &mut RandomSource,
    ) {
        if !world.get_game_rule(&SPAWN_MOBS) {
            return;
        }

        let Some(biome) = world.biome_at(source_pos) else {
            return;
        };
        let Some(spawns) = biome.spawners.get(MobCategory::Creature.name()) else {
            return;
        };

        let creature_probability = biome.creature_spawn_probability;
        let xo = chunk_pos.0.x << 4;
        let zo = chunk_pos.0.y << 4;

        while random.next_f32() < creature_probability {
            let references: Vec<&SpawnerData> = spawns.iter().collect();
            let Some(spawn) = sample_weighted(&references, random) else {
                break;
            };
            let Some(entity_type) = REGISTRY.entity_types.by_key(&spawn.entity_type) else {
                continue;
            };

            let count = sample_count(spawn, random);
            let mut group_data = None;
            let mut x = xo + random.next_i32_bounded(16);
            let mut z = zo + random.next_i32_bounded(16);
            let start_x = x;
            let start_z = z;

            for _ in 0..count {
                let mut spawned = false;
                for _ in 0..CHUNK_GENERATION_ATTEMPTS {
                    if spawned {
                        break;
                    }
                    let pos = top_non_colliding_pos(world, region, entity_type, x, z);
                    let width = f64::from(entity_type.dimensions.half_width());
                    let fx =
                        f64::from(x).clamp(f64::from(xo) + width, f64::from(xo) + 16.0 - width);
                    let fz =
                        f64::from(z).clamp(f64::from(zo) + width, f64::from(zo) + 16.0 - width);

                    if entity_type.summonable
                        && SpawnPlacements::is_spawn_position_ok(entity_type, region, pos)
                        && world.no_collision(spawn_aabb_at(
                            entity_type,
                            fx,
                            f64::from(pos.y()),
                            fz,
                        ))
                        && SpawnPlacements::check_spawn_rules(
                            entity_type,
                            world,
                            EntitySpawnReason::ChunkGeneration,
                            BlockPos::from(DVec3::new(fx, f64::from(pos.y()), fz)),
                            random,
                        )
                        && let Ok(entity) = create_entity_instance(
                            world,
                            entity_type,
                            DVec3::new(fx, f64::from(pos.y()), fz),
                        )
                    {
                        let yaw = random.next_f32() * 360.0;
                        entity.snap_to(DVec3::new(fx, f64::from(pos.y()), fz), yaw, 0.0);
                        if let Some(mob) = entity.as_mob()
                            && mob.check_spawn_obstruction(world)
                        {
                            group_data = mob.finalize_spawn(
                                world,
                                EntitySpawnReason::ChunkGeneration,
                                group_data,
                            );
                            let _ = region.add_fresh_entity(entity);
                            spawned = true;
                        }
                    }

                    // Vanilla's post-attempt block: jitter the candidate, then re-roll *both*
                    // coordinates from the attempt's start until the pair is back inside the
                    // chunk. The pair must stay inside because `top_non_colliding_pos` reads it
                    // through the worldgen region, whose dependency table only covers the chunk
                    // and its direct-dependency neighbours.
                    x += random.next_i32_bounded(5) - random.next_i32_bounded(5);
                    z += random.next_i32_bounded(5) - random.next_i32_bounded(5);
                    while x < xo || x >= xo + 16 || z < zo || z >= zo + 16 {
                        x = start_x + random.next_i32_bounded(5) - random.next_i32_bounded(5);
                        z = start_z + random.next_i32_bounded(5) - random.next_i32_bounded(5);
                    }
                }
            }
        }
    }
}

/// Vanilla `NaturalSpawner.getTopNonCollidingPos`.
fn top_non_colliding_pos(
    world: &World,
    level: &WorldGenRegion<'_>,
    entity_type: EntityTypeRef,
    x: i32,
    z: i32,
) -> BlockPos {
    let heightmap = SpawnPlacements::heightmap_type(entity_type);
    let mut pos = BlockPos::new(x, level.height_at(heightmap, x, z), z);
    if !world.dimension_type.has_ceiling {
        return SpawnPlacements::placement_type(entity_type).adjust_spawn_position(level, pos);
    }

    while !level.get_block_state(pos).is_air() {
        pos = pos.below();
    }
    while level.get_block_state(pos).is_air() && pos.y() > level.min_y() {
        pos = pos.below();
    }

    SpawnPlacements::placement_type(entity_type).adjust_spawn_position(level, pos)
}

/// Vanilla `EntityType.getSpawnAABB` at exact coordinates.
#[must_use]
pub fn spawn_aabb_at(entity_type: EntityTypeRef, x: f64, y: f64, z: f64) -> WorldAabb {
    let dimensions = entity_type.dimensions;
    WorldAabb::entity_box(
        x,
        y,
        z,
        f64::from(dimensions.width) / 2.0,
        f64::from(dimensions.height),
    )
}

/// Vanilla `Level.getWorldBorder().isWithinBounds` and the chunk-tick helpers.
impl World {
    /// Vanilla `Level.getWorldBorder().isWithinBounds(BlockPos)`.
    #[must_use]
    pub fn is_within_spawn_border(&self, pos: BlockPos) -> bool {
        self.is_block_within_world_border(pos)
    }

    /// Vanilla `ServerLevel.canSpawnEntitiesInChunk`.
    #[must_use]
    pub fn can_spawn_entities_in_chunk(&self, pos: ChunkPos) -> bool {
        self.chunk_map.is_entity_ticking_full_chunk_loaded(pos)
            && self.is_within_spawn_border(BlockPos::new(pos.0.x << 4, 0, pos.0.y << 4))
    }

    /// Vanilla `Level.noCollision(AABB)`.
    #[must_use]
    pub fn no_collision(self: &Arc<Self>, aabb: WorldAabb) -> bool {
        WorldCollisionProvider::new(self)
            .get_block_collisions(&aabb)
            .is_empty()
    }

    /// Vanilla `ServerLevel.isSpawningMonsters`, the `spawnEnemies` flag
    /// `ServerChunkCache` passes to the natural spawner and the custom spawners.
    #[must_use]
    pub fn is_spawning_monsters(&self) -> bool {
        self.get_game_rule(&SPAWN_MOBS) && self.get_game_rule(&SPAWN_MONSTERS)
    }

    /// Vanilla `ServerChunkCache.tickChunks`: the natural-spawn chunk pass.
    ///
    /// Vanilla also runs `level.tickChunk` (random ticks) and
    /// `tickCustomSpawners` inside this method; Steel runs the random-tick part
    /// in `ChunkMap::tick_game` and the custom spawners from `World::tick_game`,
    /// in the same order.
    pub fn tick_natural_spawning(self: &Arc<Self>, spawn_enemies: bool) {
        let chunk_positions = self.chunk_map.tickable_full_chunk_positions();
        let spawnable_chunk_count = i32::try_from(chunk_positions.len()).unwrap_or(i32::MAX);

        let do_mob_spawning = self.get_game_rule(&SPAWN_MOBS);
        let mut state = NaturalSpawner::create_state(self, spawnable_chunk_count);
        let spawning_categories = if do_mob_spawning {
            let spawn_persistent = self.game_time() % PERSISTENT_SPAWN_INTERVAL == 0;
            NaturalSpawner::filtered_spawning_categories(&state, spawn_enemies, spawn_persistent)
        } else {
            Vec::new()
        };

        let mut spawning_chunks: Vec<ChunkPos> = chunk_positions
            .into_iter()
            .filter(|pos| !players_close_for_spawning(self, *pos).is_empty())
            .collect();

        let mut random = runtime_random();
        for index in (1..spawning_chunks.len()).rev() {
            let swap = random.next_i32_bounded(index as i32 + 1) as usize;
            spawning_chunks.swap(index, swap);
        }

        for pos in spawning_chunks {
            let _ = self.chunk_map.with_full_chunk(pos, |chunk| {
                // Vanilla `ServerChunkCache.tickSpawningChunk` increments
                // inhabited time for every spawning chunk, before the spawn gate.
                chunk.common().increment_inhabited_time();

                if spawning_categories.is_empty() || !self.can_spawn_entities_in_chunk(pos) {
                    return;
                }

                NaturalSpawner::spawn_for_chunk(
                    self,
                    pos,
                    chunk,
                    &mut state,
                    &spawning_categories,
                    &mut random,
                );
            });
        }
    }
}

/// `MobCategory` helpers the spawner reads; the registry owns the values.
impl World {
    /// Unused placeholder retained for callers that need the biome at a
    /// position through the spawner's snapshot semantics.
    ///
    /// Returns the biome whose spawn list and costs apply at `pos`.
    #[must_use]
    pub fn spawn_biome_at(&self, pos: BlockPos) -> Option<BiomeRef> {
        self.biome_at(pos)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use steel_registry::entity_type::MobCategory;
    use steel_registry::vanilla_biomes;
    use steel_registry::{RegistryEntry as _, vanilla_blocks, vanilla_dimension_types};
    use steel_utils::random::legacy_random::LegacyRandom;
    use steel_utils::types::UpdateFlags;
    use steel_utils::{BlockPos, ChunkPos};

    use super::*;
    use crate::chunk::chunk_holder::TickingReadiness;
    use crate::chunk::chunk_ticket_manager::ChunkTicketLevel;
    use crate::player::ResetReason;
    use crate::test_support::{
        TestPlayerBuilder, fresh_test_world_with_dimension_type, insert_ready_full_chunk,
        insert_unready_full_chunk,
    };

    /// `nether_wastes` monster spawn list, in the registry entity-type order the
    /// vanilla biome JSON declares.
    const NETHER_WASTES_MONSTERS: [&str; 5] = [
        "enderman",
        "ghast",
        "magma_cube",
        "piglin",
        "zombified_piglin",
    ];

    /// Netherrack floor height. Vanilla's `BiomeManager` fuzzes the biome lookup by up to
    /// two blocks below the sampled position, so the floor has to sit well above the
    /// dimension's minimum Y — at `y = 3` the fuzzed quart cell falls below the world and
    /// reads a default biome. The tests keep air above it so `ON_GROUND` placement has a
    /// valid surface with two empty blocks above.
    const FLOOR_Y: i32 = 32;

    /// Entity-ticking spawn grid. Vanilla's global monster cap is
    /// `maxInstancesPerChunk * spawnableChunkCount / 289`, so a single chunk caps
    /// monsters at zero; this grid gives a cap of
    /// `70 * 30 / 289 = 7` monsters.
    const GRID_X: i32 = 6;
    const GRID_Z: i32 = 5;

    /// Writes the `nether_wastes` biome into every section of `pos`.
    fn write_nether_biome(world: &Arc<World>, pos: ChunkPos) {
        let biome_id = vanilla_biomes::NETHER_WASTES.id() as u16;
        world
            .chunk_map
            .with_full_chunk(pos, |chunk| {
                for section in chunk.sections().sections.iter() {
                    let mut section = section.write();
                    for quart_x in 0..4 {
                        for quart_y in 0..4 {
                            for quart_z in 0..4 {
                                section.set_biome(quart_x, quart_y, quart_z, biome_id);
                            }
                        }
                    }
                }
            })
            .expect("prepared chunk should be loaded");
    }

    /// Inserts one entity-ticking chunk whose whole column carries the
    /// `nether_wastes` biome and a netherrack floor.
    fn prepare_nether_chunk(world: &Arc<World>, pos: ChunkPos) {
        let holder = insert_ready_full_chunk(world, pos);
        holder.set_simulation_level(Some(ChunkTicketLevel::ENTITY_TICKING_CHUNK));
        holder.transition_ticking_readiness(TickingReadiness::EntityTicking);

        write_nether_biome(world, pos);

        let netherrack = vanilla_blocks::NETHERRACK.default_state();
        for local_x in 0..16 {
            for local_z in 0..16 {
                let block_pos =
                    BlockPos::new((pos.0.x << 4) + local_x, FLOOR_Y, (pos.0.y << 4) + local_z);
                assert!(
                    world.set_block(block_pos, netherrack, UpdateFlags::UPDATE_NONE),
                    "the netherrack floor should be placeable at {block_pos:?}"
                );
            }
        }
    }

    /// `nether_wastes` biome only, not entity ticking: vanilla's `BiomeManager` fuzzes the
    /// biome lookup by up to two quart cells, so the ring around the spawn grid has to
    /// carry the nether biome as well — otherwise a fuzzed lookup near the grid edge reads
    /// the neighbouring chunk's generated `plains` biome and spawns overworld monsters.
    fn prepare_nether_biome_ring(world: &Arc<World>, pos: ChunkPos) {
        insert_unready_full_chunk(world, pos);
        write_nether_biome(world, pos);
    }

    #[test]
    fn nether_wastes_resolves_vanilla_monster_spawn_list() {
        crate::bootstrap::init_globals();
        let fixture = fresh_test_world_with_dimension_type(
            "nether_biome_spawn_list",
            "nether_biome_spawn_list",
            &vanilla_dimension_types::THE_NETHER,
        );
        let world = &fixture.world;
        let chunk_pos = ChunkPos::new(0, 0);
        prepare_nether_chunk(world, chunk_pos);

        let pos = BlockPos::new(8, FLOOR_Y + 1, 8);
        assert_eq!(
            world
                .spawn_biome_at(pos)
                .map(|biome| biome.key.path.to_string()),
            Some("nether_wastes".to_owned()),
            "the prepared chunk should report the nether wastes biome"
        );

        let mut spawns: Vec<String> = mobs_at(world, MobCategory::Monster, pos)
            .expect("the nether wastes biome should carry a monster spawn list")
            .iter()
            .map(|data| data.entity_type.path.to_string())
            .collect();
        spawns.sort();
        assert_eq!(
            spawns, NETHER_WASTES_MONSTERS,
            "the nether monster list should match vanilla"
        );

        let mut random = RandomSource::Legacy(LegacyRandom::from_seed(4242));
        for _ in 0..32 {
            let (entity_type, _) =
                random_spawn_mob_at(world, MobCategory::Monster, &mut random, pos)
                    .expect("the weighted roll should resolve a nether monster");
            assert!(
                NETHER_WASTES_MONSTERS.contains(&entity_type.key.path.as_ref()),
                "rolled monster {} should come from the nether wastes list",
                entity_type.key
            );
        }
    }

    #[test]
    fn nether_world_spawns_monsters_around_a_player() {
        crate::bootstrap::init_globals();
        let fixture = fresh_test_world_with_dimension_type(
            "nether_natural_spawn",
            "nether_natural_spawn",
            &vanilla_dimension_types::THE_NETHER,
        );
        let world = &fixture.world;

        for chunk_x in -1..=GRID_X {
            for chunk_z in -1..=GRID_Z {
                let pos = ChunkPos::new(chunk_x, chunk_z);
                if (0..GRID_X).contains(&chunk_x) && (0..GRID_Z).contains(&chunk_z) {
                    prepare_nether_chunk(world, pos);
                } else {
                    prepare_nether_biome_ring(world, pos);
                }
            }
        }
        let player = TestPlayerBuilder::new(Arc::clone(world), "NetherSpawner", 4242).build();
        player.snap_to(DVec3::new(48.5, f64::from(FLOOR_Y + 1), 40.5), 0.0, 0.0);
        assert!(world.add_player(Arc::clone(&player), ResetReason::InitialJoin));

        world.chunk_map.rebuild_ticking_chunk_snapshot();
        assert_eq!(
            world.chunk_map.tickable_full_chunk_positions().len(),
            (GRID_X * GRID_Z) as usize,
            "every prepared chunk should stay entity ticking for the spawn pass"
        );

        for _ in 0..16 {
            world.tick_natural_spawning(true);
        }

        let spawned: Vec<SharedEntity> = world
            .entity_manager
            .get_accessible_entities()
            .into_iter()
            .filter(|entity| entity.entity_type().mob_category == MobCategory::Monster)
            .collect();
        assert!(
            !spawned.is_empty(),
            "a nether world with a nearby player should spawn monsters"
        );
        for mob in &spawned {
            let entity_type = mob.entity_type().key.path.as_ref();
            assert!(
                NETHER_WASTES_MONSTERS.contains(&entity_type),
                "nether spawning produced {entity_type}, which is not in the nether wastes list"
            );
            let distance = mob.position().distance(player.position());
            assert!(
                distance <= f64::from(MobCategory::Monster.despawn_distance()),
                "spawned {entity_type} {distance} blocks away, outside the monster despawn range"
            );
        }
    }
}
