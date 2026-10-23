//! Vanilla `WanderingTraderSpawner` + `WanderingTraderData`.
//!
//! Vanilla registers this as a `ServerLevel` `CustomSpawner` and stores its state
//! in per-level saved data (`minecraft:wandering_trader`). Steel has no custom
//! spawner hook on `World::tick_game` yet, so this module owns the vanilla tick
//! rule and the persisted counters; the world integration only has to call
//! [`WanderingTraderSpawner::tick`] once per world tick and to load/save the
//! counters through [`WanderingTraderSpawner::load`] / [`WanderingTraderSpawner::save`].

use std::sync::Arc;

use glam::DVec3;
use serde::{Deserialize, Serialize};
use steel_registry::biome::Biome;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::vanilla_biome_tags::BiomeTag;
use steel_registry::vanilla_game_rules::SPAWN_WANDERING_TRADERS;
use steel_registry::{REGISTRY, RegistryExt as _, vanilla_entities};
use steel_utils::saved_data::names as saved_data_names;
use steel_utils::{BlockPos, Downcast, Identifier, WorldAabb};

use super::WanderingTraderEntity;
use crate::chunk::heightmap::HeightmapType;
use crate::entity::entities::mobs::passive::llama::TraderLlamaEntity;
use crate::entity::{
    Entity, EntitySpawnPlacement, EntitySpawnReason, EntitySpawnRequest, Mob, SharedEntity,
    spawn_entity,
};
use crate::physics::{WorldCollisionProvider, has_block_collision};
use crate::player::Player;
use crate::poi::OccupationStatus;
use crate::world::{LevelReader, World};

/// Vanilla `WanderingTraderSpawner.DEFAULT_TICK_DELAY`.
pub const DEFAULT_TICK_DELAY: i32 = 1200;
/// Vanilla `WanderingTraderSpawner.DEFAULT_SPAWN_DELAY`.
pub const DEFAULT_SPAWN_DELAY: i32 = 24000;
/// Vanilla `WanderingTraderSpawner.MIN_SPAWN_CHANCE`.
pub const MIN_SPAWN_CHANCE: i32 = 25;
/// Vanilla `WanderingTraderSpawner.MAX_SPAWN_CHANCE`.
const MAX_SPAWN_CHANCE: i32 = 75;
/// Vanilla `WanderingTraderSpawner.SPAWN_CHANCE_INCREASE`.
const SPAWN_CHANCE_INCREASE: i32 = 25;
/// Vanilla `WanderingTraderSpawner.SPAWN_ONE_IN_X_CHANCE`.
const SPAWN_ONE_IN_X_CHANCE: i32 = 10;
/// Vanilla `WanderingTraderSpawner.NUMBER_OF_SPAWN_ATTEMPTS`.
const NUMBER_OF_SPAWN_ATTEMPTS: i32 = 10;
/// Vanilla `WanderingTraderSpawner.spawn` search radius around the reference position.
const SPAWN_RADIUS: i32 = 48;
/// Vanilla `WanderingTraderSpawner.spawn` llama search radius.
const LLAMA_SPAWN_RADIUS: i32 = 4;
/// Vanilla `WanderingTrader.setDespawnDelay(48000)` after a spawner spawn.
pub const WANDERING_TRADER_DESPAWN_DELAY: i32 = 48000;
/// Vanilla `WanderingTraderSpawner.spawn` home radius (`setHomeTo(referencePos, 16)`).
const TRADER_HOME_RADIUS: i32 = 16;

/// Vanilla `WanderingTraderData`: the persisted spawner counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WanderingTraderData {
    /// Remaining ticks until the next spawn evaluation.
    pub spawn_delay: i32,
    /// Percentage chance used by the next evaluation.
    pub spawn_chance: i32,
}

impl Default for WanderingTraderData {
    fn default() -> Self {
        Self {
            spawn_delay: DEFAULT_SPAWN_DELAY,
            spawn_chance: MIN_SPAWN_CHANCE,
        }
    }
}

impl WanderingTraderData {
    /// Vanilla `WanderingTraderData.setSpawnDelay`.
    pub fn set_spawn_delay(&mut self, spawn_delay: i32) {
        self.spawn_delay = spawn_delay;
    }

    /// Vanilla `WanderingTraderData.setSpawnChance`.
    pub fn set_spawn_chance(&mut self, spawn_chance: i32) {
        self.spawn_chance = spawn_chance;
    }
}

/// Vanilla `WanderingTraderSpawner`.
pub struct WanderingTraderSpawner {
    tick_delay: i32,
    data: WanderingTraderData,
}

impl Default for WanderingTraderSpawner {
    fn default() -> Self {
        Self::new()
    }
}

impl WanderingTraderSpawner {
    /// Creates the spawner with vanilla's default delay and spawn chance.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            tick_delay: DEFAULT_TICK_DELAY,
            data: WanderingTraderData {
                spawn_delay: DEFAULT_SPAWN_DELAY,
                spawn_chance: MIN_SPAWN_CHANCE,
            },
        }
    }

    /// Rebuilds the spawner from persisted [`WanderingTraderData`].
    #[must_use]
    pub const fn with_data(data: WanderingTraderData) -> Self {
        Self {
            tick_delay: DEFAULT_TICK_DELAY,
            data,
        }
    }

    /// Returns the persisted counters for saving.
    #[must_use]
    pub const fn data(&self) -> WanderingTraderData {
        self.data
    }

    /// Loads the counters from this world's `wandering_trader` saved data.
    ///
    /// # Errors
    /// Propagates the saved-data I/O error.
    pub async fn load(world: &World) -> std::io::Result<Self> {
        let data = world
            .saved_data
            .load_or_default::<WanderingTraderData>(saved_data_names::WANDERING_TRADER)
            .await?;
        Ok(Self::with_data(data))
    }

    /// Persists the counters to this world's `wandering_trader` saved data.
    ///
    /// # Errors
    /// Propagates the saved-data I/O error.
    pub async fn save(&self, world: &World) -> std::io::Result<()> {
        world
            .saved_data
            .save(saved_data_names::WANDERING_TRADER, &self.data)
            .await
    }

    /// Vanilla `WanderingTraderSpawner.tick`.
    pub fn tick(&mut self, world: &Arc<World>) {
        if !world.get_game_rule(&SPAWN_WANDERING_TRADERS) {
            return;
        }

        self.tick_delay -= 1;
        if self.tick_delay > 0 {
            return;
        }
        self.tick_delay = DEFAULT_TICK_DELAY;

        self.data.spawn_delay -= DEFAULT_TICK_DELAY;
        if self.data.spawn_delay > 0 {
            return;
        }

        self.data.spawn_delay = DEFAULT_SPAWN_DELAY;
        let chance_to_spawn = self.data.spawn_chance;
        self.data.spawn_chance =
            (chance_to_spawn + SPAWN_CHANCE_INCREASE).clamp(MIN_SPAWN_CHANCE, MAX_SPAWN_CHANCE);

        // Vanilla `random.nextInt(100) <= chanceToSpawn`.
        if rand::random_range(0..100) <= chance_to_spawn && self.spawn(world) {
            self.data.spawn_chance = MIN_SPAWN_CHANCE;
        }
    }

    /// Vanilla `WanderingTraderSpawner.spawn`.
    fn spawn(&self, world: &Arc<World>) -> bool {
        let Some(player) = random_player(world) else {
            // Vanilla treats "no player" as a successful attempt.
            return true;
        };

        if rand::random_range(0..SPAWN_ONE_IN_X_CHANCE) != 0 {
            return false;
        }

        let player_pos = player.block_position();
        let reference_pos = meeting_poi_near(world, player_pos).unwrap_or(player_pos);
        let Some(spawn_position) = find_spawn_position_near(world, reference_pos, SPAWN_RADIUS)
        else {
            return false;
        };
        if !has_enough_space(world, spawn_position) {
            return false;
        }
        if biome_without_wandering_trader_spawns(world, spawn_position) {
            return false;
        }

        let Some(trader) =
            spawn_entity_at(world, &vanilla_entities::WANDERING_TRADER, spawn_position)
        else {
            return false;
        };

        for _ in 0..2 {
            try_to_spawn_llama_for(world, &trader, LLAMA_SPAWN_RADIUS);
        }

        let Some(wandering_trader) = trader.downcast_ref::<WanderingTraderEntity>() else {
            return false;
        };
        wandering_trader.set_despawn_delay(WANDERING_TRADER_DESPAWN_DELAY);
        wandering_trader.set_wander_target(Some(reference_pos));
        wandering_trader.set_home_to(reference_pos, TRADER_HOME_RADIUS);
        true
    }
}

/// Vanilla `ServerLevel.getRandomPlayer`.
fn random_player(world: &Arc<World>) -> Option<Arc<Player>> {
    let count = world.players.len();
    if count == 0 {
        return None;
    }

    let index = rand::random_range(0..count);
    let mut players = Vec::with_capacity(count);
    world.players.iter_players(|_, player| {
        players.push(Arc::clone(player));
        true
    });
    players.get(index).cloned()
}

/// Vanilla `PoiManager.find(MEETING, ANY, playerPos, 48)`: nearest meeting POI.
fn meeting_poi_near(world: &Arc<World>, center: BlockPos) -> Option<BlockPos> {
    let meeting = REGISTRY
        .poi_types
        .id_from_key(&Identifier::vanilla_static("meeting"))?;

    let storage = world.poi_storage.lock();
    let mut nearest: Option<(BlockPos, f64)> = None;
    for (pos, _poi_type) in storage.get_in_horizontal_square(
        &|id| id == meeting,
        center,
        SPAWN_RADIUS,
        OccupationStatus::Any,
    ) {
        let (x, y, z) = pos.get_center();
        let (cx, cy, cz) = center.get_center();
        let distance = DVec3::new(x - cx, y - cy, z - cz).length_squared();
        if nearest.is_none_or(|(_, best)| distance < best) {
            nearest = Some((pos, distance));
        }
    }
    nearest.map(|(pos, _)| pos)
}

/// Vanilla `WanderingTraderSpawner.findSpawnPositionNear`.
fn find_spawn_position_near(
    world: &Arc<World>,
    reference_position: BlockPos,
    radius: i32,
) -> Option<BlockPos> {
    let entity_type = &vanilla_entities::WANDERING_TRADER;
    for _ in 0..NUMBER_OF_SPAWN_ATTEMPTS {
        let x = reference_position.x() + rand::random_range(0..radius * 2) - radius;
        let z = reference_position.z() + rand::random_range(0..radius * 2) - radius;
        let y = world.height_at(HeightmapType::MotionBlockingNoLeaves, x, z);
        let spawn_pos = BlockPos::new(x, y, z);
        if on_ground_spawn_position_ok(world, spawn_pos, entity_type) {
            return Some(spawn_pos);
        }
    }

    None
}

/// Vanilla `SpawnPlacementTypes.ON_GROUND.isSpawnPositionOk` for one entity type.
///
/// The world-border gate of the vanilla method is not applied: Steel's border
/// bounds check is not reachable from this module.
fn on_ground_spawn_position_ok(
    world: &Arc<World>,
    block_pos: BlockPos,
    entity_type: EntityTypeRef,
) -> bool {
    let above = block_pos.above();
    let below = block_pos.below();

    if !is_valid_spawn_block(world, below, entity_type) {
        return false;
    }

    is_valid_empty_spawn_block(world, block_pos) && is_valid_empty_spawn_block(world, above)
}

/// Vanilla `BlockState.isValidSpawn` for the block below a spawn position.
fn is_valid_spawn_block(world: &Arc<World>, pos: BlockPos, entity_type: EntityTypeRef) -> bool {
    let state = world.get_block_state(pos);
    crate::behavior::BLOCK_BEHAVIORS
        .get_behavior(state.get_block())
        .is_valid_spawn(state, world.as_ref(), pos, entity_type)
}

/// Vanilla `NaturalSpawner.isValidEmptySpawnBlock` without the per-entity
/// `isBlockDangerous` and `PREVENT_MOB_SPAWNING_INSIDE` checks, which Steel's
/// block behavior layer does not expose yet.
fn is_valid_empty_spawn_block(world: &Arc<World>, pos: BlockPos) -> bool {
    let state = world.get_block_state(pos);
    !state.is_solid() && state.get_fluid_state().is_empty()
}

/// Vanilla `WanderingTraderSpawner.hasEnoughSpace`.
fn has_enough_space(world: &Arc<World>, spawn_pos: BlockPos) -> bool {
    let (min_x, min_y, min_z) = (
        f64::from(spawn_pos.x()),
        f64::from(spawn_pos.y()),
        f64::from(spawn_pos.z()),
    );
    let box_ = WorldAabb::new(min_x, min_y, min_z, min_x + 2.0, min_y + 3.0, min_z + 2.0);
    !has_block_collision(&WorldCollisionProvider::new(world), box_)
}

/// Vanilla `BiomeTags.WITHOUT_WANDERING_TRADER_SPAWNS` gate.
fn biome_without_wandering_trader_spawns(world: &Arc<World>, pos: BlockPos) -> bool {
    world
        .biome_at(pos)
        .is_some_and(|biome| Biome::has_tag(biome, &BiomeTag::WITHOUT_WANDERING_TRADER_SPAWNS))
}

/// Vanilla `EntityType.spawn(level, pos, EntitySpawnReason.EVENT)`.
fn spawn_entity_at(
    world: &Arc<World>,
    entity_type: EntityTypeRef,
    pos: BlockPos,
) -> Option<SharedEntity> {
    spawn_entity(
        world,
        EntitySpawnRequest {
            entity_type,
            placement: EntitySpawnPlacement::Block {
                pos,
                try_move_down: false,
                moved_up: false,
            },
            reason: EntitySpawnReason::Event,
            finalize_spawn: true,
            play_ambient_sound: true,
            item_stack: None,
            user_is_operator: false,
        },
    )
    .ok()
}

/// Vanilla `WanderingTraderSpawner.tryToSpawnLlamaFor`.
fn try_to_spawn_llama_for(world: &Arc<World>, trader: &SharedEntity, radius: i32) {
    let Some(spawn_position) = find_spawn_position_near(world, trader.block_position(), radius)
    else {
        return;
    };
    let Some(llama) = spawn_entity_at(world, &vanilla_entities::TRADER_LLAMA, spawn_position)
    else {
        return;
    };

    if llama.downcast_ref::<TraderLlamaEntity>().is_none() {
        return;
    }
    let Some(leashable) = llama.as_leashable() else {
        return;
    };
    leashable.set_leashed_to(trader);
}

impl crate::world::custom_spawner::CustomSpawner for WanderingTraderSpawner {
    fn tick(&mut self, world: &Arc<World>) {
        WanderingTraderSpawner::tick(self, world);
    }

    fn saved_data_name(&self) -> Option<steel_utils::saved_data::SavedDataName> {
        Some(saved_data_names::WANDERING_TRADER)
    }

    fn save<'a>(
        &'a self,
        world: &'a World,
    ) -> crate::world::custom_spawner::CustomSpawnerSaveFuture<'a> {
        Box::pin(async move { WanderingTraderSpawner::save(self, world).await })
    }
}
