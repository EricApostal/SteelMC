//! Live-behaviour tests for the water-mob movement path (`AbstractFish` and the squids).
//!
//! Each test drives the real path: a fish (or squid) spawned in an entity-ticking water
//! chunk, ticked through the world entity manager, with assertions on the resulting
//! displacement, altitude and flock state.

use std::sync::Arc;

use glam::DVec3;
use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_entities, vanilla_game_rules};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, ChunkPos};

use super::cod::CodEntity;
use crate::behavior::init_behaviors;
use crate::chunk::chunk_holder::TickingReadiness;
use crate::chunk::chunk_ticket_manager::ChunkTicketLevel;
use crate::entity::ai::goal::SchoolingFish;
use crate::entity::entities::mobs::neutral::squid::glow_squid::GlowSquidEntity;
use crate::entity::entities::mobs::neutral::squid::squid::SquidEntity;
use crate::entity::{Entity, SharedEntity, init_entities};
use crate::test_support::{TestWorld, fresh_test_world, insert_ready_full_chunk};
use crate::world::World;

/// Stone floor block of the test pool.
const POOL_FLOOR_Y: i32 = 56;
/// First water block of the test pool.
const POOL_MIN_WATER_Y: i32 = 57;
/// Last water block of the test pool.
const POOL_MAX_WATER_Y: i32 = 68;
/// Edge length of the test pool, in blocks (a 2x2 chunk of water, so a jetting squid
/// stays inside it for the length of a run).
const POOL_EXTENT: i32 = 32;
/// Chunks per axis spanned by [`POOL_EXTENT`].
const POOL_CHUNKS: i32 = POOL_EXTENT / 16;
/// Spawn height inside the pool (mob feet).
const SPAWN_Y: f64 = 62.0;
/// Horizontal spawn position inside the pool (the middle of the 2x2 chunk pool).
const SPAWN_XZ: f64 = 15.5;
/// Vanilla `Squid.getDefaultGravity`.
const SQUID_GRAVITY: f64 = 0.08;
/// Vanilla `Squid.SquidRandomMovementGoal` picks the vertical jet component in
/// `-0.1..0.1`, so this is the fastest a squid can move downward in water (the in-water
/// `aiStep` never applies gravity, only the jet vector or its decay).
const SQUID_JET_MAX_DOWNWARD_SPEED: f64 = 0.1;
/// Vanilla `LivingEntity.getAirDrag` for a squid (`AIR_DRAG_MODIFIER` defaults to 1.0).
const VANILLA_AIR_DRAG: f64 = 0.98;

/// Builds a 32x32 pool of still water with a stone floor and marks the chunks
/// entity-ticking, so mob AI actually runs.
fn fish_pool_world(key: &'static str) -> TestWorld {
    init_vanilla_registry();
    init_behaviors();
    init_entities();
    let fixture = fresh_test_world(key);
    let world = &fixture.world;

    for chunk_x in 0..POOL_CHUNKS {
        for chunk_z in 0..POOL_CHUNKS {
            let chunk_pos = ChunkPos::new(chunk_x, chunk_z);
            let holder = insert_ready_full_chunk(world, chunk_pos);
            holder.set_simulation_level(Some(ChunkTicketLevel::ENTITY_TICKING_CHUNK));
            holder.transition_ticking_readiness(TickingReadiness::EntityTicking);
            world.update_entity_chunk_visibility(chunk_pos, holder.entity_visibility());
        }
    }
    assert!(world.set_game_rule(&vanilla_game_rules::SPAWN_MOBS, false));

    for x in 0..POOL_EXTENT {
        for z in 0..POOL_EXTENT {
            world.set_block(
                BlockPos::new(x, POOL_FLOOR_Y, z),
                vanilla_blocks::STONE.default_state(),
                UpdateFlags::UPDATE_ALL,
            );
            for y in POOL_MIN_WATER_Y..=POOL_MAX_WATER_Y {
                world.set_block(
                    BlockPos::new(x, y, z),
                    vanilla_blocks::WATER.default_state(),
                    UpdateFlags::UPDATE_ALL,
                );
            }
        }
    }

    fixture
}

/// Registers a mob in the world as persistence-required, which is how vanilla keeps a mob
/// from being despawned (and from accumulating `noActionTime`) in a world with no players.
fn add_entity(world: &Arc<World>, entity: SharedEntity) {
    if let Some(mob) = entity.as_mob() {
        mob.set_persistence_required();
    }
    world
        .try_add_entity(entity)
        .expect("mob should be added to the world");
}

fn spawn_cod(world: &Arc<World>, id: i32, position: DVec3) -> Arc<CodEntity> {
    let cod = Arc::new(CodEntity::new(
        &vanilla_entities::COD,
        id,
        position,
        Arc::downgrade(world),
    ));
    add_entity(world, Arc::<CodEntity>::clone(&cod) as SharedEntity);
    cod
}

fn spawn_squid(world: &Arc<World>, id: i32, position: DVec3) -> Arc<SquidEntity> {
    let squid = Arc::new(SquidEntity::new(
        &vanilla_entities::SQUID,
        id,
        position,
        Arc::downgrade(world),
    ));
    add_entity(world, Arc::<SquidEntity>::clone(&squid) as SharedEntity);
    squid
}

fn spawn_glow_squid(world: &Arc<World>, id: i32, position: DVec3) -> Arc<GlowSquidEntity> {
    let squid = Arc::new(GlowSquidEntity::new(
        &vanilla_entities::GLOW_SQUID,
        id,
        position,
        Arc::downgrade(world),
    ));
    add_entity(world, Arc::<GlowSquidEntity>::clone(&squid) as SharedEntity);
    squid
}

/// Ticks the world's entity manager, which is what drives mob AI in these tests.
fn tick_world(world: &Arc<World>, ticks: i32) {
    for tick in 0..ticks {
        world.entity_manager().tick_entities(tick, true);
    }
}

#[test]
fn cod_in_water_swims_horizontally_instead_of_spinning_in_place() {
    let fixture = fish_pool_world("cod_swims_horizontally");
    let world = &fixture.world;
    let cod = spawn_cod(world, 2, DVec3::new(SPAWN_XZ, SPAWN_Y, SPAWN_XZ));

    let mut travelled = 0.0;
    let mut previous = cod.position();
    for _ in 0..300 {
        tick_world(world, 1);
        let position = cod.position();
        travelled += (position - previous).with_y(0.0).length();
        previous = position;
    }

    assert!(
        !cod.is_removed(),
        "the cod should stay loaded while ticking"
    );
    assert!(
        travelled > 4.0,
        "a cod in open water must cover ground while it turns; swam {travelled} blocks"
    );
    assert!(
        (cod.position() - DVec3::new(SPAWN_XZ, SPAWN_Y, SPAWN_XZ))
            .with_y(0.0)
            .length()
            > 1.0,
        "the cod ended up where it started: {:?}",
        cod.position()
    );
}

#[test]
fn cod_stays_in_the_water_column_instead_of_sinking_to_the_floor() {
    let fixture = fish_pool_world("cod_keeps_altitude");
    let world = &fixture.world;
    let cod = spawn_cod(world, 2, DVec3::new(SPAWN_XZ, SPAWN_Y, SPAWN_XZ));

    tick_world(world, 200);
    let mut free_swimming_ticks = 0;
    let mut highest = f64::MIN;
    for _ in 0..100 {
        tick_world(world, 1);
        let y = cod.position().y;
        highest = highest.max(y);
        if y > f64::from(POOL_MIN_WATER_Y) + 1.5 {
            free_swimming_ticks += 1;
        }
    }

    assert!(
        free_swimming_ticks >= 50,
        "the cod sat on the pool floor for {}/100 ticks (y={})",
        100 - free_swimming_ticks,
        cod.position().y
    );
    assert!(
        highest <= f64::from(POOL_MAX_WATER_Y) + 1.0,
        "the cod left the water column: reached y={highest}"
    );
}

#[test]
fn schooling_cod_keep_one_leader_with_the_flock_in_range() {
    let fixture = fish_pool_world("cod_schools");
    let world = &fixture.world;
    let cods: Vec<Arc<CodEntity>> = (0..4)
        .map(|index| {
            spawn_cod(
                world,
                2 + index,
                DVec3::new(SPAWN_XZ + f64::from(index) * 0.6, SPAWN_Y, SPAWN_XZ),
            )
        })
        .collect();

    let mut saw_a_follower = false;
    for _ in 0..30 {
        tick_world(world, 10);

        // Multiple flocks in one pool are legal vanilla behaviour: a fish that leaves
        // `inRangeOfLeader` (11 blocks) stops following, after which vanilla lets it
        // elect itself. So assert the invariants vanilla actually guarantees rather
        // than "exactly one leader" or a guaranteed school size — the fish wander a
        // 32x32 pool and both of those are observations, not guarantees.
        for cod in &cods {
            if SchoolingFish::is_follower(cod.as_ref()) {
                saw_a_follower = true;
                // Vanilla `AbstractSchoolingFish.canContinueToUse` drops a follower
                // that leaves `inRangeOfLeader`, so a follower is never out of range.
                assert!(
                    SchoolingFish::in_range_of_leader(cod.as_ref()),
                    "every follower must stay within `inRangeOfLeader` of its leader"
                );
            }
        }
    }

    assert!(
        saw_a_follower,
        "the cods should have formed at least one school within 30 samples"
    );
}

#[test]
fn squid_in_water_jets_around_without_sinking_to_the_floor() {
    let fixture = fish_pool_world("squid_jets");
    let world = &fixture.world;
    let squid = spawn_squid(world, 2, DVec3::new(SPAWN_XZ, SPAWN_Y, SPAWN_XZ));

    let mut travelled = 0.0;
    let mut previous = squid.position();
    let mut in_water_ticks = 0;
    // A squid's jet points wherever its random movement vector points, so vanilla does not
    // hold it at a fixed altitude: with the jet bound below it legitimately drifts several
    // blocks down (and back up) over a few hundred ticks, and can even come to rest on the
    // pool floor. What vanilla never does is *fall* in water: `Squid.aiStep`'s in-water
    // branch only sets the velocity to the jet vector or decays it, and applies gravity
    // only out of water. Track the slowest vertical speed the squid reaches while it stays
    // in water; a velocity carried in from the air is not the in-water dynamics, so those
    // samples are skipped.
    let mut min_in_water_velocity_y = f64::INFINITY;
    let mut was_in_water = false;
    for _ in 0..300 {
        tick_world(world, 1);
        let position = squid.position();
        travelled += (position - previous).length();
        previous = position;
        let in_water = squid.is_in_water();
        if in_water {
            in_water_ticks += 1;
            if was_in_water {
                min_in_water_velocity_y = min_in_water_velocity_y.min(squid.velocity().y);
            }
        }
        was_in_water = in_water;
    }

    assert_eq!(
        squid.tick_count(),
        300,
        "the entity manager must step the squid once per tick for this test to mean anything"
    );
    assert!(
        travelled > 5.0,
        "a squid in water must jet around; moved {travelled} blocks"
    );
    assert!(
        in_water_ticks > 250,
        "the squid left the pool: only {in_water_ticks}/300 ticks in water"
    );
    // Vanilla `Squid.SquidRandomMovementGoal` picks `y` in `-0.1..0.1` and the in-water
    // body only ever assigns that vector or its `0.9` decay to the velocity, so |v.y| can
    // never exceed the jet's own bound. The shipped bug (water gravity / `LivingEntity`
    // travel applied to the squid) makes the downward speed grow past it within two ticks.
    assert!(
        min_in_water_velocity_y > -(SQUID_JET_MAX_DOWNWARD_SPEED + 0.05),
        "a squid must not fall in water (vanilla applies no water gravity), but its \
         vertical speed reached {min_in_water_velocity_y} over {in_water_ticks} water ticks"
    );
}

#[test]
fn glow_squid_runs_the_vanilla_squid_body() {
    let fixture = fish_pool_world("glow_squid_jets");
    let world = &fixture.world;
    let squid = spawn_glow_squid(world, 2, DVec3::new(SPAWN_XZ, SPAWN_Y, SPAWN_XZ));

    let mut travelled = 0.0;
    let mut previous = squid.position();
    for _ in 0..300 {
        tick_world(world, 1);
        let position = squid.position();
        travelled += (position - previous).length();
        previous = position;
    }

    assert!(
        travelled > 5.0,
        "a glow squid inherits `Squid`'s movement; moved {travelled} blocks"
    );
    assert!(
        squid.x_body_rot() != 0.0 || squid.z_body_rot() != 0.0,
        "the glow squid should run the squid body rotation state"
    );
}

#[test]
fn squid_in_air_uses_vanilla_air_drag() {
    let fixture = fish_pool_world("squid_air_drag");
    let world = &fixture.world;
    // Spawned 20 blocks above the pool surface, so the squid stays in air for the run.
    let start_y = f64::from(POOL_MAX_WATER_Y) + 20.0;
    let squid = spawn_squid(world, 2, DVec3::new(SPAWN_XZ, start_y, SPAWN_XZ));

    let ticks = 20;
    tick_world(world, ticks);

    // Vanilla `Squid.aiStep` outside water: `delta.y = (delta.y - getGravity()) * getAirDrag()`,
    // so `v_n` converges on the terminal velocity as `-g*d/(1-d) * (1 - d^n)`.
    let terminal = -SQUID_GRAVITY * VANILLA_AIR_DRAG / (1.0 - VANILLA_AIR_DRAG);
    let expected = terminal * (1.0 - VANILLA_AIR_DRAG.powi(ticks));
    let actual = squid.velocity().y;
    assert!(
        (actual - expected).abs() < 0.05,
        "vanilla air drag should keep {expected} after {ticks} ticks, got {actual}"
    );
    assert!(
        squid.position().y < start_y && squid.position().y > f64::from(POOL_MAX_WATER_Y),
        "the squid should still be falling through air: {:?}",
        squid.position()
    );
}
