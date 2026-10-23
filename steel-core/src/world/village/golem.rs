//! Iron golem summoning (vanilla `Villager.spawnGolemIfNeeded` and
//! `SpawnUtil.trySpawnMob` with `Strategy.LEGACY_IRON_GOLEM`).
//!
//! Villagers agree to summon a golem by consensus: a villager that has slept recently
//! and has not seen a golem lately counts itself plus nearby villagers in the same
//! state, and once enough of them agree one of them runs the placement search. The
//! search is `SpawnUtil`'s: random XZ offsets around the villager, then descend from
//! `spawnRangeY` above the candidate until a block that can support a golem is found.
//!
//! Deliberate difference: vanilla samples `level.getRandom()`; Steel's `World` exposes no
//! level RNG, so the placement search and spawn yaw use the thread RNG like the other
//! world-spawn ports (see `entity::raid::patrol`).

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::ai::brain::memory::module_type as mem;
    use crate::entity::entities::mobs::npc::villager::VillagerEntity;
    use crate::test_support::{
        advance_test_game_time_to, fresh_test_world, insert_ready_full_chunk,
    };
    use steel_registry::blocks::block_state_ext::BlockStateExt as _;
    use steel_registry::{init_vanilla_registry, vanilla_blocks};
    use steel_utils::ChunkPos;
    use steel_utils::types::UpdateFlags;

    /// Loads the 3x3 chunk area around `center` and builds a flat stone floor at
    /// `center.y - 1` so the spawn search always finds a `LEGACY_IRON_GOLEM` support
    /// block. The search reaches 8 blocks horizontally, so every candidate stays inside
    /// the loaded area.
    fn build_floor(world: &Arc<World>, center: BlockPos) {
        let chunk = ChunkPos::from_block_pos(center);
        for dx in -1..=1 {
            for dz in -1..=1 {
                insert_ready_full_chunk(world, ChunkPos::new(chunk.0.x + dx, chunk.0.y + dz));
            }
        }
        for dx in -8..=8 {
            for dz in -8..=8 {
                assert!(
                    world.set_block(
                        center.offset(dx, -1, dz),
                        vanilla_blocks::STONE.default_state(),
                        UpdateFlags::UPDATE_CLIENTS,
                    ),
                    "the test floor must be writable at {} {}",
                    center.offset(dx, -1, dz).x(),
                    center.offset(dx, -1, dz).z()
                );
            }
        }
    }

    fn spawn_villager(world: &Arc<World>, id: i32, pos: DVec3) -> SharedEntity {
        let villager: SharedEntity = Arc::new(VillagerEntity::new(
            &vanilla_entities::VILLAGER,
            id,
            pos,
            Arc::downgrade(world),
        ));
        world
            .try_add_entity(Arc::clone(&villager))
            .expect("the villager joins the loaded chunk");
        villager
    }

    fn set_last_slept(villager: &SharedEntity, game_time: i64) {
        villager
            .as_mob()
            .and_then(Mob::brain)
            .expect("the villager has a brain")
            .lock()
            .set_memory(&mem::LAST_SLEPT, Some(game_time));
    }

    /// Vanilla `Villager.spawnGolemIfNeeded`: five villagers that slept recently agree and
    /// one golem appears; the whole neighborhood is then marked as having detected it.
    #[test]
    fn gossip_consensus_spawns_one_golem_and_marks_the_neighborhood() {
        init_vanilla_registry();
        crate::entity::init_entities();

        let fixture = fresh_test_world("village_golem_consensus");
        let world = &fixture.world;
        build_floor(world, BlockPos::new(8, 60, 8));

        let mut villagers = Vec::new();
        for (index, (x, z)) in [(8, 8), (9, 8), (8, 9), (9, 9), (10, 10)]
            .into_iter()
            .enumerate()
        {
            let villager = spawn_villager(
                world,
                i32::try_from(index + 1).expect("small index"),
                DVec3::new(f64::from(x) + 0.5, 60.0, f64::from(z) + 0.5),
            );
            set_last_slept(&villager, 0);
            villagers.push(villager);
        }
        assert!(
            villagers
                .iter()
                .all(|villager| villager_wants_golem(villager.as_ref(), world)),
            "every villager that slept within the window wants a golem"
        );

        let golem = spawn_golem_if_needed(world, villagers[0].as_ref(), 5)
            .expect("five agreeing villagers summon a golem");
        assert_eq!(golem.entity_type(), &vanilla_entities::IRON_GOLEM);
        assert!(
            world.get_entity_by_id(golem.id()).is_some(),
            "the summoned golem joins the level"
        );

        for villager in &villagers {
            assert!(
                !villager_wants_golem(villager.as_ref(), world),
                "`GolemSensor.golemDetected` blocks further summons"
            );
        }

        // A second summon is refused while the detection memory lives.
        assert!(spawn_golem_if_needed(world, villagers[0].as_ref(), 5).is_none());
    }

    /// A summon needs `villagersNeededToAgree` agreeing villagers, and villagers that did
    /// not sleep inside `GOLEM_SLEEP_WINDOW` do not count.
    #[test]
    fn golem_consensus_requires_recent_sleepers() {
        init_vanilla_registry();
        crate::entity::init_entities();

        let fixture = fresh_test_world("village_golem_consensus_threshold");
        let world = &fixture.world;
        build_floor(world, BlockPos::new(8, 60, 8));

        // `gameTime` must exceed the sleep window for a stale `LAST_SLEPT` to disqualify.
        advance_test_game_time_to(world, GOLEM_SLEEP_WINDOW + 1);

        let fresh = spawn_villager(world, 1, DVec3::new(8.5, 60.0, 8.5));
        set_last_slept(&fresh, world.game_time());
        let stale = spawn_villager(world, 2, DVec3::new(9.5, 60.0, 8.5));
        set_last_slept(&stale, 0);
        assert!(
            !villager_wants_golem(stale.as_ref(), world),
            "a villager that slept a full day ago does not want a golem"
        );

        assert!(
            spawn_golem_if_needed(world, fresh.as_ref(), 2).is_none(),
            "one willing villager cannot meet a two-villager consensus"
        );
        assert!(
            spawn_golem_if_needed(world, fresh.as_ref(), 1).is_some(),
            "a single villager can meet a one-villager consensus"
        );
    }

    /// Vanilla `Villager.wantsToSpawnGolem` also requires `LAST_SLEPT` to be present.
    #[test]
    fn golem_summon_requires_the_last_slept_memory() {
        init_vanilla_registry();
        crate::entity::init_entities();

        let fixture = fresh_test_world("village_golem_no_sleep");
        let world = &fixture.world;
        build_floor(world, BlockPos::new(8, 60, 8));

        let villager = spawn_villager(world, 1, DVec3::new(8.5, 60.0, 8.5));
        assert!(!villager_wants_golem(villager.as_ref(), world));
        assert!(spawn_golem_if_needed(world, villager.as_ref(), 1).is_none());
    }
}
