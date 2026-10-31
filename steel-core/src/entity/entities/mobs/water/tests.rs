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
use crate::entity::entities::mobs::neutral::nautilus::NautilusEntity;
use crate::entity::entities::mobs::neutral::squid::glow_squid::GlowSquidEntity;
use crate::entity::entities::mobs::neutral::squid::squid::SquidEntity;
use crate::entity::entities::mobs::passive::tadpole::TadpoleEntity;
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
    // The pool fills the 2x2 chunks `0..POOL_CHUNKS`, so the enclosure's wall columns at
    // `-1` and `POOL_EXTENT` fall into the neighbouring chunk ring. `set_block` is a no-op
    // on an unloaded chunk, so the ring must be loaded or the pool stays open at the sides.
    for chunk_x in -1..=POOL_CHUNKS {
        for chunk_z in -1..=POOL_CHUNKS {
            let inside_pool =
                (0..POOL_CHUNKS).contains(&chunk_x) && (0..POOL_CHUNKS).contains(&chunk_z);
            if !inside_pool {
                insert_ready_full_chunk(world, ChunkPos::new(chunk_x, chunk_z));
            }
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

    // Close the pool with a stone lid and walls. A jetting squid (or a swimming cod) can
    // leave an open 12-deep pool through the surface or a side within a few hundred ticks,
    // after which it is no longer "a mob in water": the water-contact assertions would then
    // measure the fixture instead of the mob, and an escaped fish flopping on the floor
    // never resumes its swim. The lid sits one block above the last water block, so the
    // water column (and the mobs' water contact) is unchanged, but the mobs cannot leave it.
    let stone = vanilla_blocks::STONE.default_state();
    for x in -1..=POOL_EXTENT {
        for z in -1..=POOL_EXTENT {
            let on_wall = x == -1 || z == -1 || x == POOL_EXTENT || z == POOL_EXTENT;
            if on_wall {
                for y in POOL_MIN_WATER_Y..=POOL_MAX_WATER_Y + 1 {
                    assert!(
                        world.set_block(BlockPos::new(x, y, z), stone, UpdateFlags::UPDATE_ALL),
                        "the pool wall at ({x}, {y}, {z}) must be placed"
                    );
                }
            } else {
                assert!(
                    world.set_block(
                        BlockPos::new(x, POOL_MAX_WATER_Y + 1, z),
                        stone,
                        UpdateFlags::UPDATE_ALL,
                    ),
                    "the pool lid at ({x}, {}, {z}) must be placed",
                    POOL_MAX_WATER_Y + 1
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
    // A cod's swim target is a uniform random position, so *net* displacement over a few
    // hundred ticks is a sample of that random walk: returning near the spawn point is
    // legal. Cumulative horizontal travel is the invariant that actually distinguishes
    // swimming from spinning in place (a spinning fish turns without covering ground), and
    // it does not depend on which random targets the shared RNG produced.
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
}

#[test]
fn cod_stays_in_the_water_column_instead_of_sinking_to_the_floor() {
    let fixture = fish_pool_world("cod_keeps_altitude");
    let world = &fixture.world;
    let cod = spawn_cod(world, 2, DVec3::new(SPAWN_XZ, SPAWN_Y, SPAWN_XZ));

    tick_world(world, 200);
    let mut highest = f64::MIN;
    for _ in 0..600 {
        tick_world(world, 1);
        highest = highest.max(cod.position().y);
    }

    // `FishSwimGoal` picks a uniform random `RandomPos.getPos(mob, 10, 7)` point and a cod only
    // travels a few blocks per hundred ticks, so its altitude at any instant (and any share of a
    // short window) is one sample of that random walk: a low pick legitimately keeps the cod
    // patrolling or hovering on the pool floor. What vanilla does guarantee is that the swim goal
    // keeps re-targeting (`nextInt(reducedTickDelay(40))` between starts) until the fish is
    // anywhere in its water column, while a fish whose swim goal stopped running only sinks
    // (`AbstractFish.travel` sinks a target-less fish). Over 600 ticks that difference is
    // unambiguous: a stuck cod never rises above the two bottom layers.
    assert!(
        highest > f64::from(POOL_MIN_WATER_Y) + 2.0,
        "the cod never left the bottom of the pool: it stayed at y={highest} \
         (floor {POOL_FLOOR_Y}, water {POOL_MIN_WATER_Y}..={POOL_MAX_WATER_Y})"
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
                // Vanilla `AbstractSchoolingFish.canContinueToUse` only re-checks
                // `inRangeOfLeader` when the follower's goal ticks (every 10 ticks), so a
                // follower can legally be a few blocks out of range between goal ticks.
                // Sample the invariant after a full goal interval instead of every tick.
                let out_of_range = !SchoolingFish::in_range_of_leader(cod.as_ref());
                if out_of_range {
                    tick_world(world, 10);
                    assert!(
                        SchoolingFish::in_range_of_leader(cod.as_ref())
                            || !SchoolingFish::is_follower(cod.as_ref()),
                        "a follower that left `inRangeOfLeader` must be dropped on its next goal tick"
                    );
                }
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

fn spawn_tadpole(world: &Arc<World>, id: i32, position: DVec3) -> Arc<TadpoleEntity> {
    let tadpole = Arc::new(TadpoleEntity::new(
        &vanilla_entities::TADPOLE,
        id,
        position,
        Arc::downgrade(world),
    ));
    add_entity(world, Arc::<TadpoleEntity>::clone(&tadpole) as SharedEntity);
    tadpole
}

fn spawn_nautilus(world: &Arc<World>, id: i32, position: DVec3) -> Arc<NautilusEntity> {
    let nautilus = Arc::new(NautilusEntity::new(
        &vanilla_entities::NAUTILUS,
        id,
        position,
        Arc::downgrade(world),
    ));
    add_entity(
        world,
        Arc::<NautilusEntity>::clone(&nautilus) as SharedEntity,
    );
    nautilus
}

/// Ticks a water mob in the pool and returns the highest altitude it reached.
fn highest_altitude(world: &Arc<World>, position: &dyn Fn() -> DVec3, ticks: i32) -> f64 {
    let mut highest = f64::MIN;
    for _ in 0..ticks {
        tick_world(world, 1);
        highest = highest.max(position().y);
    }
    highest
}

#[test]
fn tadpole_swims_up_through_the_water_column() {
    let fixture = fish_pool_world("tadpole_swims");
    let world = &fixture.world;
    // Spawn in the lower half so a swimming tadpole has room to rise; a mob that only
    // moves on the ground plane sinks with `AbstractFish.travelInWater` and never gets
    // back above its spawn height.
    let spawn_y = f64::from(POOL_MIN_WATER_Y) + 2.0;
    let tadpole = spawn_tadpole(world, 2, DVec3::new(SPAWN_XZ, spawn_y, SPAWN_XZ));

    let highest = highest_altitude(world, &|| tadpole.position(), 600);

    assert!(
        highest > spawn_y + 1.5,
        "a tadpole with `SmoothSwimmingMoveControl` must be able to swim up; \
         spawned at y={spawn_y}, highest y={highest}"
    );
    assert!(
        highest <= f64::from(POOL_MAX_WATER_Y) + 1.0,
        "the tadpole left the water column: reached y={highest}"
    );
}

#[test]
fn nautilus_swims_up_through_the_water_column() {
    let fixture = fish_pool_world("nautilus_swims");
    let world = &fixture.world;
    let spawn_y = f64::from(POOL_MIN_WATER_Y) + 2.0;
    let nautilus = spawn_nautilus(world, 2, DVec3::new(SPAWN_XZ, spawn_y, SPAWN_XZ));

    let highest = highest_altitude(world, &|| nautilus.position(), 600);

    assert!(
        highest > spawn_y + 1.5,
        "a nautilus with `SmoothSwimmingMoveControl` must be able to swim up; \
         spawned at y={spawn_y}, highest y={highest}"
    );
    assert!(
        highest <= f64::from(POOL_MAX_WATER_Y) + 1.0,
        "the nautilus left the water column: reached y={highest}"
    );
}
