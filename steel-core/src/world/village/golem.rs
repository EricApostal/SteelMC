//! Iron golem summoning (vanilla `Villager.spawnGolemIfNeeded` and
//! `SpawnUtil.trySpawnMob` with `Strategy.LEGACY_IRON_GOLEM`).
//!
//! Villagers agree to summon a golem by consensus: a villager that has slept recently
//! and has not seen a golem lately counts itself plus nearby villagers in the same
//! state, and once enough of them agree one of them runs the placement search. The
//! search is `SpawnUtil`'s: random XZ offsets around the villager, then descend from
//! `spawnRangeY` above the candidate until a block that can support a golem is found.

use std::sync::Arc;

use glam::DVec3;
use steel_math::{DEGREE_360, wrap_degrees};
use steel_registry::REGISTRY;
use steel_registry::TaggedRegistryExt as _;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_blocks;
use steel_registry::vanilla_entities;
use steel_utils::{BlockPos, WorldAabb};

use crate::entity::ai::brain::memory::module_type::{GOLEM_DETECTED_RECENTLY, LAST_SLEPT};
use crate::entity::{
    Entity, EntitySpawnReason, Mob, SharedEntity, add_spawned_entity, create_entity_instance,
};
use crate::world::World;

/// Vanilla `SpawnUtil.trySpawnMob` attempt count used by `Villager.spawnGolemIfNeeded`.
pub const SPAWN_ATTEMPTS: i32 = 10;
/// Vanilla `spawnRangeXZ` used by `Villager.spawnGolemIfNeeded`.
pub const SPAWN_RANGE_XZ: i32 = 8;
/// Vanilla `spawnRangeY` used by `Villager.spawnGolemIfNeeded`.
pub const SPAWN_RANGE_Y: i32 = 6;
/// Vanilla `GolemSensor.MEMORY_TIME_TO_LIVE`.
pub const GOLEM_DETECTED_TTL: i64 = 599;
/// Vanilla `Villager.golemSpawnConditionsMet`: a villager counts as having slept
/// recently while `gameTime - LAST_SLEPT` is below this.
pub const GOLEM_SLEEP_WINDOW: i64 = 24000;
/// Vanilla `Villager.spawnGolemIfNeeded` villager search box inflation.
pub const VILLAGER_SEARCH_INFLATION: f64 = 10.0;
/// Vanilla `Villager.spawnGolemIfNeeded` `limit(5)` on agreeing villagers.
pub const MAX_AGREEING_VILLAGERS: usize = 5;

/// Marks nearby villagers as having just detected a golem
/// (vanilla `GolemSensor.golemDetected`), which blocks further summons for
/// [`GOLEM_DETECTED_TTL`] ticks.
pub fn golem_detected(world: &World, center: BlockPos, radius: f64) {
    let bounds = WorldAabb::new(
        f64::from(center.x()),
        f64::from(center.y()),
        f64::from(center.z()),
        f64::from(center.x() + 1),
        f64::from(center.y() + 1),
        f64::from(center.z() + 1),
    )
    .inflate(radius);

    for entity in world.get_entities_in_aabb(&bounds) {
        mark_golem_detected(&entity);
    }
}

fn mark_golem_detected(entity: &SharedEntity) {
    if entity.entity_type() != &vanilla_entities::VILLAGER {
        return;
    }
    let Some(brain) = entity.as_mob().and_then(Mob::brain) else {
        return;
    };
    brain
        .lock()
        .set_memory_with_expiry(&GOLEM_DETECTED_RECENTLY, true, GOLEM_DETECTED_TTL);
}

/// Returns whether `mob` may summon a golem right now
/// (vanilla `Villager.wantsToSpawnGolem`): it must have slept within
/// [`GOLEM_SLEEP_WINDOW`] ticks and must not have detected a golem recently.
///
/// Vanilla's `timestamp` argument is unused there (the condition reads
/// `level.getGameTime()`), so this takes the game time it actually compares against.
#[must_use]
pub fn wants_to_spawn_golem(mob: &dyn Mob, game_time: i64) -> bool {
    let Some(brain) = mob.brain() else {
        return false;
    };
    let brain = brain.lock();
    if brain.has_memory_value(&GOLEM_DETECTED_RECENTLY) {
        return false;
    }
    brain
        .get_memory(&LAST_SLEPT)
        .is_some_and(|last_slept| game_time - last_slept < GOLEM_SLEEP_WINDOW)
}

/// Returns whether `entity` may summon a golem right now
/// (vanilla `Villager.wantsToSpawnGolem`), reading `world`'s game time.
///
/// Non-villager entities never want a golem: the memory it reads only exists on mobs
/// with a brain.
#[must_use]
pub fn villager_wants_golem(entity: &dyn Entity, world: &World) -> bool {
    entity
        .as_mob()
        .is_some_and(|mob| wants_to_spawn_golem(mob, world.game_time()))
}

/// Runs the villager golem consensus for `summoner`
/// (vanilla `Villager.spawnGolemIfNeeded`).
///
/// Returns the spawned golem when the consensus passed and a spawn position was found.
/// On success every villager inside the search box — not only the ones that agreed — is
/// marked with `GOLEM_DETECTED_RECENTLY`, like vanilla's
/// `nearbyVillagers.forEach(GolemSensor::golemDetected)`.
pub fn spawn_golem_if_needed(
    world: &Arc<World>,
    summoner: &dyn Entity,
    villagers_needed_to_agree: usize,
) -> Option<SharedEntity> {
    let game_time = world.game_time();
    if !summoner
        .as_mob()
        .is_some_and(|mob| wants_to_spawn_golem(mob, game_time))
    {
        return None;
    }

    let search_box = summoner.bounding_box().inflate_xyz(
        VILLAGER_SEARCH_INFLATION,
        VILLAGER_SEARCH_INFLATION,
        VILLAGER_SEARCH_INFLATION,
    );
    let nearby_villagers = world
        .get_entities_in_aabb(&search_box)
        .into_iter()
        .filter(|entity| entity.entity_type() == &vanilla_entities::VILLAGER)
        .collect::<Vec<_>>();

    let agreeing = nearby_villagers
        .iter()
        .filter(|entity| {
            entity
                .as_mob()
                .is_some_and(|mob| wants_to_spawn_golem(mob, game_time))
        })
        .take(MAX_AGREEING_VILLAGERS)
        .count();

    if agreeing < villagers_needed_to_agree {
        return None;
    }

    let golem = try_spawn_legacy_iron_golem(world, summoner.block_position())?;
    for villager in &nearby_villagers {
        mark_golem_detected(villager);
    }
    Some(golem)
}

/// Vanilla `SpawnUtil.trySpawnMob(IRON_GOLEM, MOB_SUMMONED, level, start, 10, 8, 6,
/// Strategy.LEGACY_IRON_GOLEM, checkCollisions = false)`.
///
/// The summoned golem is not player-created; that flag belongs to the pumpkin build path.
pub fn try_spawn_legacy_iron_golem(world: &Arc<World>, start: BlockPos) -> Option<SharedEntity> {
    for _ in 0..SPAWN_ATTEMPTS {
        // `Mth.randomBetweenInclusive`.
        let dx = rand::random_range(-SPAWN_RANGE_XZ..=SPAWN_RANGE_XZ);
        let dz = rand::random_range(-SPAWN_RANGE_XZ..=SPAWN_RANGE_XZ);
        let candidate = start.offset(dx, SPAWN_RANGE_Y, dz);
        if !world.is_block_within_world_border(candidate) {
            continue;
        }
        let Some(spawn_pos) = move_to_possible_spawn_position(world, candidate) else {
            continue;
        };

        // Vanilla SpawnUtil order: create, then check the spawn obstruction, and only then
        // add the golem (`checkCollisions` is false for this strategy, so no extra
        // collision test is needed). `checkSpawnRules` is `true` for iron golems in
        // vanilla, so it has no Steel equivalent to run here.
        let golem = create_entity_instance(
            world,
            &vanilla_entities::IRON_GOLEM,
            DVec3::new(
                f64::from(spawn_pos.x()) + 0.5,
                f64::from(spawn_pos.y()),
                f64::from(spawn_pos.z()) + 0.5,
            ),
        )
        .ok()?;

        // `Mth.wrapDegrees(level.getRandom().nextFloat() * 360.0F)`.
        let rotation = (wrap_degrees(rand::random::<f32>() * DEGREE_360), 0.0);
        golem.set_rotation(rotation);
        golem.set_old_position_to_current();
        golem.base().set_old_rotation_to_current();

        let Some(mob) = golem.as_mob() else {
            continue;
        };
        mob.set_y_head_rot(rotation.0);
        mob.set_y_body_rot(rotation.0);
        let _ = mob.finalize_spawn(world, EntitySpawnReason::MobSummoned, None);

        if !mob.check_spawn_obstruction(world) {
            continue;
        }
        if add_spawned_entity(world, Arc::clone(&golem)).is_err() {
            continue;
        }

        mob.play_ambient_sound();
        return Some(golem);
    }

    None
}

/// Vanilla `SpawnUtil.moveToPossibleSpawnPosition`: descends from `start` through
/// `y` in `[SPAWN_RANGE_Y, -SPAWN_RANGE_Y]` (13 candidate columns), returning the
/// position above the first column whose block satisfies the `LEGACY_IRON_GOLEM`
/// strategy. `start` is `BlockPos + (dx, spawnRangeY, dz)`, so the first candidate is
/// `start - 1`.
fn move_to_possible_spawn_position(world: &Arc<World>, start: BlockPos) -> Option<BlockPos> {
    let mut above_state = world.get_block_state(start);

    for offset in 1..=(SPAWN_RANGE_Y * 2 + 1) {
        let candidate = start.offset(0, -offset, 0);
        let current_state = world.get_block_state(candidate);
        if legacy_iron_golem_can_spawn_on(current_state, above_state) {
            return Some(candidate.above());
        }
        above_state = current_state;
    }

    None
}

/// Vanilla `SpawnUtil.Strategy.LEGACY_IRON_GOLEM`: `block` must be a support block that
/// is not in the strategy's disallowed set, with air or a liquid directly above.
fn legacy_iron_golem_can_spawn_on(
    block: steel_utils::BlockStateId,
    above: steel_utils::BlockStateId,
) -> bool {
    if is_disallowed_iron_golem_ground(block) {
        return false;
    }

    let above_is_air_or_liquid = above.is_air() || !above.get_fluid_state().is_empty();
    above_is_air_or_liquid
        && (block.is_solid() || block.get_block() == &vanilla_blocks::POWDER_SNOW)
}

fn is_disallowed_iron_golem_ground(state: steel_utils::BlockStateId) -> bool {
    let block = state.get_block();
    if block == &vanilla_blocks::COBWEB
        || block == &vanilla_blocks::CACTUS
        || block == &vanilla_blocks::CONDUIT
        || block == &vanilla_blocks::ICE
        || block == &vanilla_blocks::TNT
        || block == &vanilla_blocks::GLOWSTONE
        || block == &vanilla_blocks::BEACON
        || block == &vanilla_blocks::SEA_LANTERN
        || block == &vanilla_blocks::FROSTED_ICE
    {
        return true;
    }

    // Vanilla also rejects the `LeavesBlock`, `StainedGlassBlock` and
    // `StainedGlassPaneBlock` classes; those are exactly these tags.
    REGISTRY.blocks.is_in_tag(block, &BlockTag::LEAVES)
        || REGISTRY.blocks.is_in_tag(block, &BlockTag::C_GLASS_BLOCKS)
        || REGISTRY.blocks.is_in_tag(block, &BlockTag::C_GLASS_PANES)
}
