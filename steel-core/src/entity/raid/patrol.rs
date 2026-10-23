//! Vanilla `PatrolSpawner` (`net.minecraft.world.level.levelgen.PatrolSpawner`).
//!
//! Vanilla registers one of these per level as a `CustomSpawner`; Steel registers it through
//! [`World::register_custom_spawner`](crate::world::World::register_custom_spawner) and the
//! world tick drives [`PatrolSpawner::tick`].
//!
//! Vanilla keeps its single countdown in memory only (vanilla's `CustomSpawner` has no saved
//! data at all), so this spawner reports no saved-data name and is not persisted.
//!
//! Vanilla's `spawnEnemies` guard is honoured through
//! [`CustomSpawner::tick_with_settings`](crate::world::custom_spawner::CustomSpawner::tick_with_settings),
//! exactly like the other `CustomSpawner` ports.
//!
//! Deliberate differences:
//! - Vanilla samples `level.getRandom()`; Steel uses the thread RNG, like the other
//!   `CustomSpawner` ports (see `wandering_trader::spawner`).
//! - Vanilla gates on `EnvironmentAttributes.CAN_PILLAGER_PATROL_SPAWN`; Steel has no
//!   environment-attribute reader for that key, so the gate is treated as the attribute's
//!   default (`true`) — reported dependency, not silently dropped.
//! - Vanilla samples `getCurrentDifficultyAt(pos)`; Steel's
//!   [`World::current_difficulty_at`](crate::world::World::current_difficulty_at) provides the
//!   same instance, except that chunk inhabited time is not persisted across restarts.
//! - Vanilla checks `NaturalSpawner.isValidEmptySpawnBlock` with the spawn type's
//!   `isBlockDangerous`; Steel reuses [`is_valid_empty_spawn_block`], which does not model
//!   the per-type dangerous-block table.

use std::sync::Arc;

use steel_registry::equipment::EquipmentSlot;
use steel_registry::vanilla_entities;
use steel_registry::vanilla_game_rules::SPAWN_PATROLS;
use steel_utils::BlockPos;

use crate::chunk::heightmap::HeightmapType;
use crate::chunk::light::LightLayer;
use crate::entity::entities::mobs::illager::PillagerEntity;
use crate::entity::raid::raid::is_valid_empty_spawn_block;
use crate::entity::raid::{Raid, Raider};
use crate::entity::{
    Entity, EntitySpawnPlacement, EntitySpawnReason, EntitySpawnRequest, LivingEntity, Mob,
    Monster, SharedEntity, spawn_entity,
};
use crate::player::Player;
use crate::world::custom_spawner::CustomSpawner;
use crate::world::{LevelReader, World};

/// Vanilla `PatrolSpawner` reset delay before the next attempt.
const PATROL_SPAWN_DELAY: i32 = 12_000;
/// Vanilla `PatrolSpawner` random delay added to the reset delay.
const PATROL_SPAWN_DELAY_JITTER: i32 = 1_200;
/// Vanilla `PatrolSpawner` gate: one attempt in five.
const PATROL_ATTEMPT_CHANCE: i32 = 5;
/// Vanilla `PatrolSpawner` village distance check, in sections.
const PATROL_VILLAGE_SECTION_DISTANCE: i32 = 2;
/// Vanilla `PatrolSpawner` minimum distance of the patrol from the player.
const PATROL_MIN_DISTANCE: i32 = 24;
/// Vanilla `PatrolSpawner` width of the random distance band past the minimum.
const PATROL_DISTANCE_BAND: i32 = 24;
/// Vanilla `PatrolSpawner` loaded-chunk radius around the candidate position.
const PATROL_CHUNK_RADIUS: i32 = 10;
/// Vanilla `PatrolSpawner` block-light ceiling for a patrol spawn
/// (`PatrollingMonster.checkPatrollingMonsterSpawnRules`).
const PATROL_MAX_BLOCK_LIGHT: u8 = 8;
/// Vanilla `PatrolSpawner` per-member position jitter bound (`random.nextInt(5)`).
const PATROL_MEMBER_JITTER: i32 = 5;
/// Vanilla `PatrollingMonster.finalizeSpawn` leader banner drop chance.
const PATROL_LEADER_BANNER_DROP_CHANCE: f32 = 2.0;

/// Vanilla `PatrolSpawner`.
#[derive(Debug, Default)]
pub struct PatrolSpawner {
    /// Vanilla `PatrolSpawner.nextTick`.
    next_tick: i32,
}

impl PatrolSpawner {
    /// Creates the spawner with vanilla's initial countdown (`0`, so the first tick already
    /// counts down and may attempt a patrol).
    #[must_use]
    pub const fn new() -> Self {
        Self { next_tick: 0 }
    }

    /// Vanilla `ServerLevel.tickCustomSpawners` entry point: derives vanilla's
    /// `spawnEnemies` argument from `ServerLevel.isSpawningMonsters`.
    pub fn tick(&mut self, world: &Arc<World>) {
        self.tick_with_settings(world, world.is_spawning_monsters());
    }

    /// Vanilla `PatrolSpawner.tick(ServerLevel, boolean spawnEnemies)`.
    ///
    /// Vanilla wraps the whole body in `if (spawnEnemies)`, so a level that is not spawning
    /// monsters (peaceful, or monster spawning disabled) must not even advance the countdown.
    pub fn tick_with_settings(&mut self, world: &Arc<World>, spawn_enemies: bool) {
        if !spawn_enemies {
            return;
        }

        if !world.get_game_rule(&SPAWN_PATROLS) {
            return;
        }

        self.next_tick -= 1;
        if self.next_tick > 0 {
            return;
        }
        self.next_tick += PATROL_SPAWN_DELAY + rand::random_range(0..PATROL_SPAWN_DELAY_JITTER);

        if !world.is_bright_outside() {
            return;
        }

        if rand::random_range(0..PATROL_ATTEMPT_CHANCE) != 0 {
            return;
        }

        let Some(player) = random_player(world) else {
            return;
        };
        let player_pos = player.block_position();

        if player.is_spectator()
            || world.is_close_to_village(player_pos, PATROL_VILLAGE_SECTION_DISTANCE)
        {
            return;
        }

        let distance_x = PATROL_MIN_DISTANCE + rand::random_range(0..PATROL_DISTANCE_BAND);
        let sign_x = if rand::random::<bool>() { -1 } else { 1 };
        let distance_z = PATROL_MIN_DISTANCE + rand::random_range(0..PATROL_DISTANCE_BAND);
        let sign_z = if rand::random::<bool>() { -1 } else { 1 };
        let spawn_x = player_pos.x() + sign_x * distance_x;
        let spawn_z = player_pos.z() + sign_z * distance_z;

        if !world.has_chunks_at(
            spawn_x - PATROL_CHUNK_RADIUS,
            spawn_z - PATROL_CHUNK_RADIUS,
            spawn_x + PATROL_CHUNK_RADIUS,
            spawn_z + PATROL_CHUNK_RADIUS,
        ) {
            return;
        }

        // Vanilla checks `CAN_PILLAGER_PATROL_SPAWN` at this point.
        //
        // Vanilla samples `getCurrentDifficultyAt(spawnPos)` before the loop; the difficulty
        // instance only depends on the chunk, so the horizontal position is what matters.
        let group_size = world
            .current_difficulty_at(BlockPos::new(spawn_x, 0, spawn_z))
            .effective_difficulty()
            .ceil() as i32
            + 1;
        let mut spawn_pos = BlockPos::new(spawn_x, 0, spawn_z);

        for index in 0..group_size {
            let y = world.height_at(
                HeightmapType::MotionBlockingNoLeaves,
                spawn_pos.x(),
                spawn_pos.z(),
            );
            spawn_pos = BlockPos::new(spawn_pos.x(), y, spawn_pos.z());

            if index == 0 {
                if !spawn_patrol_member(world, spawn_pos, true) {
                    break;
                }
            } else {
                spawn_patrol_member(world, spawn_pos, false);
            }

            let offset_x = rand::random_range(0..PATROL_MEMBER_JITTER)
                - rand::random_range(0..PATROL_MEMBER_JITTER);
            let offset_z = rand::random_range(0..PATROL_MEMBER_JITTER)
                - rand::random_range(0..PATROL_MEMBER_JITTER);
            spawn_pos = spawn_pos.offset(offset_x, 0, offset_z);
        }
    }
}

impl CustomSpawner for PatrolSpawner {
    fn tick(&mut self, world: &Arc<World>) {
        PatrolSpawner::tick(self, world);
    }

    fn tick_with_settings(&mut self, world: &Arc<World>, spawn_enemies: bool) {
        PatrolSpawner::tick_with_settings(self, world, spawn_enemies);
    }
}

/// Vanilla `ServerLevel.getRandomPlayer`.
fn random_player(world: &Arc<World>) -> Option<Arc<Player>> {
    let count = world.players.len();
    if count == 0 {
        return None;
    }

    let index = rand::random_range(0..count);
    let mut selected = None;
    let mut current = 0;
    world.players.iter_players(|_, player| {
        if current == index {
            selected = Some(Arc::clone(player));
            return false;
        }
        current += 1;
        true
    });
    selected
}

/// Vanilla `PatrolSpawner.spawnPatrolMember`.
fn spawn_patrol_member(world: &Arc<World>, pos: BlockPos, is_leader: bool) -> bool {
    if !is_valid_empty_spawn_block(world, pos) {
        return false;
    }

    if !check_patrolling_monster_spawn_rules(world, pos) {
        return false;
    }

    let Ok(pillager) = spawn_entity(
        world,
        EntitySpawnRequest {
            entity_type: &vanilla_entities::PILLAGER,
            placement: EntitySpawnPlacement::Block {
                pos,
                try_move_down: false,
                moved_up: false,
            },
            reason: EntitySpawnReason::Patrol,
            finalize_spawn: true,
            play_ambient_sound: false,
            item_stack: None,
            user_is_operator: false,
        },
    ) else {
        return false;
    };

    if is_leader {
        // Vanilla sets the leader flag before `finalizeSpawn`, whose
        // `PatrollingMonster` branch then equips the ominous banner; Steel applies the same
        // state after the shared spawn coordinator ran.
        equip_patrol_leader(&pillager);
    } else if let Some(raider) = pillager.as_raider() {
        // Vanilla `PatrollingMonster.finalizeSpawn` marks every `PATROL` spawn as patrolling.
        raider.set_patrolling(true);
    }

    true
}

/// Vanilla `PatrollingMonster.finalizeSpawn` for a patrol leader: patrol leader flag, a
/// fresh patrol target, the ominous banner in the head slot and its drop chance.
fn equip_patrol_leader(pillager: &SharedEntity) {
    if let Some(raider) = pillager.as_raider() {
        raider.set_patrol_leader(true);
        raider.find_patrol_target();
    }

    if let Some(living) = pillager.as_living_entity() {
        living
            .living_base()
            .equipment()
            .lock()
            .set(EquipmentSlot::Head, Raid::ominous_banner_instance());
    }

    if let Some(mob) = pillager.as_mob() {
        mob.set_equipment_drop_chance(EquipmentSlot::Head, PATROL_LEADER_BANNER_DROP_CHANCE);
    }
}

/// Vanilla `PatrollingMonster.checkPatrollingMonsterSpawnRules`.
fn check_patrolling_monster_spawn_rules(world: &Arc<World>, pos: BlockPos) -> bool {
    if world.light_value_at(LightLayer::Block, pos) > PATROL_MAX_BLOCK_LIGHT {
        return false;
    }

    <PillagerEntity as Monster>::check_any_light_monster_spawn_rules(
        &vanilla_entities::PILLAGER,
        world,
        EntitySpawnReason::Patrol,
        pos,
    )
}
