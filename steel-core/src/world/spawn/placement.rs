//! Vanilla `SpawnPlacements`: the per-entity-type spawn placement table.
//!
//! Vanilla keeps a `Map<EntityType, Data(heightmap, placementType, predicate)>`
//! filled by a static registration block. Steel's spawn predicates are trait
//! helper methods (`Monster::check_monster_spawn_rules`,
//! `Animal::check_animal_spawn_rules`, `Mob::check_mob_spawn_rules`), so this
//! module is the type → `(heightmap, placement, predicate)` binding that gives
//! the spawners the same lookup vanilla has.
//!
//! Those helpers are static functions whose bodies never read `Self`, so a row
//! evaluates them through one representative implementation of the trait; a type
//! that overrides the default (vanilla `MushroomCow`) gets a row bound to its own
//! implementation. The table mirrors vanilla's registration block: every entity
//! type Steel implements *and* vanilla registers has a row, and a type with no row
//! behaves like a vanilla type with no `SpawnPlacements` entry
//! (`NO_RESTRICTIONS`, motion-blocking-no-leaves heightmap, no predicate).

use std::ptr;

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::shapes::is_offset_shape_full_block;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::fluid::is_water_fluid;
use steel_registry::vanilla_biome_tags::BiomeTag;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_blocks;
use steel_registry::vanilla_entities;
use steel_registry::vanilla_fluids;
use steel_utils::random::Random as _;
use steel_utils::random::RandomSource;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::types::Difficulty;
use steel_utils::{BlockPos, BlockStateId};

use crate::behavior::BLOCK_BEHAVIORS;
use crate::behavior::blocks::vegetation::float_attribute;
use crate::chunk::heightmap::HeightmapType;
use crate::chunk::light::LightLayer;
use crate::entity::Animal;
use crate::entity::EntitySpawnReason;
use crate::entity::Mob;
use crate::entity::Monster;
use crate::entity::ai::path::PathComputationType;
use crate::entity::ai::walk::WalkPathEvaluator;
use crate::entity::entities::mobs::hostile::piglin::check_piglin_spawn_rules;
use crate::entity::entities::mobs::neutral::nautilus;
use crate::entity::entities::mobs::water::abstract_fish;
use crate::entity::entities::{
    ArmadilloEntity, AxolotlEntity, BatEntity, CamelEntity, DolphinEntity, DrownedEntity,
    EndermiteEntity, FoxEntity, FrogEntity, GhastEntity, GlowSquidEntity, GoatEntity,
    GuardianEntity, HoglinEntity, MagmaCubeEntity, MushroomCowEntity, OcelotEntity, ParrotEntity,
    PigEntity, PolarBearEntity, RabbitEntity, SilverfishEntity, SkeletonHorseEntity, SlimeEntity,
    SquidEntity, StrayEntity, StriderEntity, SulfurCubeEntity, TropicalFishEntity, TurtleEntity,
    WolfEntity, ZombieEntity, ZombifiedPiglinEntity,
};
use crate::world::is_redstone_conductor as world_is_redstone_conductor;
use crate::world::{LevelReader, World};

/// Vanilla `SpawnPlacementType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnPlacementType {
    /// Vanilla `SpawnPlacementTypes.NO_RESTRICTIONS`.
    NoRestrictions,
    /// Vanilla `SpawnPlacementTypes.IN_WATER`.
    InWater,
    /// Vanilla `SpawnPlacementTypes.IN_LAVA`.
    InLava,
    /// Vanilla `SpawnPlacementTypes.ON_GROUND`.
    OnGround,
}

impl SpawnPlacementType {
    /// Vanilla `SpawnPlacementType.isSpawnPositionOk`.
    ///
    /// The vanilla world-border check that wraps every placement is applied by
    /// the caller through [`World::is_within_spawn_border`]; `LevelReader` alone
    /// cannot answer it.
    #[must_use]
    pub fn is_spawn_position_ok(
        self,
        level: &dyn LevelReader,
        pos: BlockPos,
        entity_type: EntityTypeRef,
    ) -> bool {
        match self {
            Self::NoRestrictions => true,
            Self::InWater => {
                let above = pos.above();
                let above_state = level.get_block_state(above);
                is_water_fluid(level.get_block_state(pos).get_fluid_state().fluid_id)
                    && !is_redstone_conductor(level, above_state, above)
            }
            Self::InLava => {
                level.get_block_state(pos).get_fluid_state().fluid_id == &vanilla_fluids::LAVA
            }
            Self::OnGround => {
                let above = pos.above();
                let below = pos.below();
                let below_state = level.get_block_state(below);
                is_valid_spawn_on_block(level, below, below_state, entity_type)
                    && is_valid_empty_spawn_block(
                        level,
                        pos,
                        level.get_block_state(pos),
                        entity_type,
                    )
                    && is_valid_empty_spawn_block(
                        level,
                        above,
                        level.get_block_state(above),
                        entity_type,
                    )
            }
        }
    }

    /// Vanilla `SpawnPlacementTypes.ON_GROUND.adjustSpawnPosition`.
    #[must_use]
    pub fn adjust_spawn_position(self, level: &dyn LevelReader, candidate: BlockPos) -> BlockPos {
        if self != Self::OnGround {
            return candidate;
        }

        let below = candidate.below();
        let state = level.get_block_state(below);
        if BLOCK_BEHAVIORS
            .get_behavior(state.get_block())
            .is_pathfindable(state, PathComputationType::Land)
        {
            below
        } else {
            candidate
        }
    }
}

/// Vanilla `BlockState.isValidSpawn` — the block's own spawn acceptance.
pub fn is_valid_spawn_on_block(
    level: &dyn LevelReader,
    pos: BlockPos,
    state: BlockStateId,
    entity_type: EntityTypeRef,
) -> bool {
    BLOCK_BEHAVIORS
        .get_behavior(state.get_block())
        .is_valid_spawn(state, level, pos, entity_type)
}

/// Vanilla `BlockState.isRedstoneConductor`.
#[must_use]
pub fn is_redstone_conductor(level: &dyn LevelReader, state: BlockStateId, pos: BlockPos) -> bool {
    world_is_redstone_conductor(level, state, pos)
}

/// Vanilla `EntityType.isBlockDangerous`.
///
/// Vanilla first consults the type's `immuneTo` block tag; Steel's entity types
/// do not carry that tag yet, so an entity is never immune by tag.
#[must_use]
pub fn is_block_dangerous(entity_type: EntityTypeRef, state: BlockStateId) -> bool {
    if !entity_type.fire_immune && WalkPathEvaluator::is_burning_block(state) {
        return true;
    }

    let block = state.get_block();
    block == &vanilla_blocks::WITHER_ROSE
        || block == &vanilla_blocks::SWEET_BERRY_BUSH
        || block == &vanilla_blocks::CACTUS
        || block == &vanilla_blocks::POWDER_SNOW
}

/// Vanilla `SpawnPlacements.SpawnPredicate`, narrowed to the read view and the
/// seeded RNG Steel's rule helpers accept.
pub type SpawnPredicate =
    fn(EntityTypeRef, &World, EntitySpawnReason, BlockPos, &mut RandomSource) -> bool;

/// Vanilla `Monster.checkMonsterSpawnRules`.
fn check_monster_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    random: &mut RandomSource,
) -> bool {
    <ZombieEntity as Monster>::check_monster_spawn_rules(
        entity_type,
        level,
        spawn_reason,
        pos,
        random,
    )
}

/// Vanilla `Monster.checkAnyLightMonsterSpawnRules`.
fn check_any_light_monster_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    _random: &mut RandomSource,
) -> bool {
    <ZombieEntity as Monster>::check_any_light_monster_spawn_rules(
        entity_type,
        level,
        spawn_reason,
        pos,
    )
}

/// Vanilla `Monster.checkSurfaceMonstersSpawnRules`.
fn check_surface_monsters_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    random: &mut RandomSource,
) -> bool {
    <ZombieEntity as Monster>::check_surface_monsters_spawn_rules(
        entity_type,
        level,
        spawn_reason,
        pos,
        random,
    )
}

/// Vanilla `Animal.checkAnimalSpawnRules`.
fn check_animal_spawn_rules(
    _entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    _random: &mut RandomSource,
) -> bool {
    <PigEntity as Animal>::check_animal_spawn_rules(level, spawn_reason, pos)
}

/// Vanilla `MushroomCow.checkMushroomSpawnRules`.
fn check_mushroom_spawn_rules(
    _entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    _random: &mut RandomSource,
) -> bool {
    <MushroomCowEntity as Animal>::check_animal_spawn_rules(level, spawn_reason, pos)
}

/// Vanilla `Armadillo.checkArmadilloSpawnRules`.
fn check_armadillo_spawn_rules(
    _entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    _random: &mut RandomSource,
) -> bool {
    <ArmadilloEntity as Animal>::check_animal_spawn_rules(level, spawn_reason, pos)
}

/// Vanilla `Fox.checkFoxSpawnRules`.
fn check_fox_spawn_rules(
    _entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    _random: &mut RandomSource,
) -> bool {
    <FoxEntity as Animal>::check_animal_spawn_rules(level, spawn_reason, pos)
}

/// Vanilla `Wolf.checkWolfSpawnRules`.
fn check_wolf_spawn_rules(
    _entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    _random: &mut RandomSource,
) -> bool {
    <WolfEntity as Animal>::check_animal_spawn_rules(level, spawn_reason, pos)
}

/// Vanilla `Goat.checkGoatSpawnRules`.
fn check_goat_spawn_rules(
    _entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    _random: &mut RandomSource,
) -> bool {
    <GoatEntity as Animal>::check_animal_spawn_rules(level, spawn_reason, pos)
}

/// Vanilla `Frog.checkFrogSpawnRules`.
fn check_frog_spawn_rules(
    _entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    _random: &mut RandomSource,
) -> bool {
    <FrogEntity as Animal>::check_animal_spawn_rules(level, spawn_reason, pos)
}

/// Vanilla `Parrot.checkParrotSpawnRules`.
fn check_parrot_spawn_rules(
    _entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    _random: &mut RandomSource,
) -> bool {
    <ParrotEntity as Animal>::check_animal_spawn_rules(level, spawn_reason, pos)
}

/// Vanilla `Camel.checkCamelSpawnRules`.
fn check_camel_spawn_rules(
    _entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    _random: &mut RandomSource,
) -> bool {
    <CamelEntity as Animal>::check_animal_spawn_rules(level, spawn_reason, pos)
}

/// Vanilla `Turtle.checkTurtleSpawnRules`.
fn check_turtle_spawn_rules(
    _entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    _random: &mut RandomSource,
) -> bool {
    <TurtleEntity as Animal>::check_animal_spawn_rules(level, spawn_reason, pos)
}

/// Vanilla `WaterAnimal.checkSurfaceWaterAnimalSpawnRules`, the shared predicate
/// of cod, salmon and pufferfish.
fn check_surface_water_animal_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    random: &mut RandomSource,
) -> bool {
    abstract_fish::check_surface_water_animal_spawn_rules(
        entity_type,
        level,
        spawn_reason,
        pos,
        random,
    )
}

/// Vanilla `TropicalFish.checkTropicalFishSpawnRules`.
fn check_tropical_fish_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    random: &mut RandomSource,
) -> bool {
    TropicalFishEntity::check_tropical_fish_spawn_rules(
        entity_type,
        level,
        spawn_reason,
        pos,
        random,
    )
}

/// Vanilla `AbstractNautilus.checkNautilusSpawnRules`.
fn check_nautilus_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    random: &mut RandomSource,
) -> bool {
    nautilus::check_nautilus_spawn_rules(entity_type, level, spawn_reason, pos, random)
}

/// Vanilla `Rabbit.checkRabbitSpawnRules`.
fn check_rabbit_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    random: &mut RandomSource,
) -> bool {
    RabbitEntity::check_rabbit_spawn_rules(entity_type, level, spawn_reason, pos, random)
}

/// Vanilla `PolarBear.checkPolarBearSpawnRules`.
fn check_polar_bear_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    random: &mut RandomSource,
) -> bool {
    PolarBearEntity::check_polar_bear_spawn_rules(entity_type, level, spawn_reason, pos, random)
}

/// Vanilla `SulfurCube.checkSulfurCubeSpawnRules`.
fn check_sulfur_cube_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    _random: &mut RandomSource,
) -> bool {
    <SulfurCubeEntity as Mob>::check_mob_spawn_rules(entity_type, level, spawn_reason, pos)
}

/// Vanilla `Axolotl.checkAxolotlSpawnRules`.
fn check_axolotl_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    random: &mut RandomSource,
) -> bool {
    AxolotlEntity::check_axolotl_spawn_rules(entity_type, level, spawn_reason, pos, random)
}

/// Vanilla `AgeableWaterCreature.checkSurfaceAgeableWaterCreatureSpawnRules`.
fn check_dolphin_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    random: &mut RandomSource,
) -> bool {
    DolphinEntity::check_surface_ageable_water_creature_spawn_rules(
        entity_type,
        level,
        spawn_reason,
        pos,
        random,
    )
}

/// Vanilla `AgeableWaterCreature.checkSurfaceAgeableWaterCreatureSpawnRules`.
fn check_squid_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    random: &mut RandomSource,
) -> bool {
    SquidEntity::check_surface_ageable_water_creature_spawn_rules(
        entity_type,
        level,
        spawn_reason,
        pos,
        random,
    )
}

/// Vanilla `GlowSquid.checkGlowSquidSpawnRules`.
fn check_glow_squid_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    random: &mut RandomSource,
) -> bool {
    GlowSquidEntity::check_glow_squid_spawn_rules(entity_type, level, spawn_reason, pos, random)
}

/// Vanilla `Drowned.checkDrownedSpawnRules`.
fn check_drowned_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    random: &mut RandomSource,
) -> bool {
    DrownedEntity::check_drowned_spawn_rules(entity_type, level, spawn_reason, pos, random)
}

/// Vanilla `Guardian.checkGuardianSpawnRules`.
fn check_guardian_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    random: &mut RandomSource,
) -> bool {
    GuardianEntity::check_guardian_spawn_rules(entity_type, level, spawn_reason, pos, random)
}

/// Vanilla `Endermite.checkEndermiteSpawnRules`.
fn check_endermite_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    random: &mut RandomSource,
) -> bool {
    EndermiteEntity::check_endermite_spawn_rules(entity_type, level, spawn_reason, pos, random)
}

/// Vanilla `Silverfish.checkSilverfishSpawnRules`.
fn check_silverfish_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    random: &mut RandomSource,
) -> bool {
    SilverfishEntity::check_silverfish_spawn_rules(entity_type, level, spawn_reason, pos, random)
}

/// Vanilla `Bat.checkBatSpawnRules`.
fn check_bat_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    random: &mut RandomSource,
) -> bool {
    BatEntity::check_bat_spawn_rules(entity_type, level, spawn_reason, pos, random)
}

/// Vanilla `Stray.checkStraySpawnRules`.
fn check_stray_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    random: &mut RandomSource,
) -> bool {
    StrayEntity::check_stray_spawn_rules(entity_type, level, spawn_reason, pos, random)
}

/// Vanilla `Ocelot.checkOcelotSpawnRules`.
fn check_ocelot_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    random: &mut RandomSource,
) -> bool {
    OcelotEntity::check_ocelot_spawn_rules(entity_type, level, spawn_reason, pos, random)
}

/// Vanilla `MagmaCube.checkMagmaCubeSpawnRules`.
fn check_magma_cube_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    random: &mut RandomSource,
) -> bool {
    MagmaCubeEntity::check_magma_cube_spawn_rules(entity_type, level, spawn_reason, pos, random)
}

/// Vanilla `Ghast.checkGhastSpawnRules`.
fn check_ghast_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    random: &mut RandomSource,
) -> bool {
    GhastEntity::check_ghast_spawn_rules(entity_type, level, spawn_reason, pos, random)
}

/// Vanilla `SkeletonHorse.checkSkeletonHorseSpawnRules`.
fn check_skeleton_horse_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    random: &mut RandomSource,
) -> bool {
    SkeletonHorseEntity::check_skeleton_horse_spawn_rules(
        entity_type,
        level,
        spawn_reason,
        pos,
        random,
    )
}

/// Vanilla `Slime.checkSlimeSpawnRules`.
///
/// Peaceful refuses outright, a spawner delegates straight to
/// `Mob.checkMobSpawnRules`, the swamp surface branch takes the biome tag, the
/// `(50, 70)` Y band, the timeline-driven `SURFACE_SLIME_SPAWN_CHANCE` attribute
/// and the `nextInt(8)` brightness roll, and the worldgen branch takes the
/// `WorldgenRandom.seedSlimeChunk(.., 987234911L)` `nextInt(10) == 0` chunk roll
/// below Y 40. Every accepting branch still requires `Mob.checkMobSpawnRules`.
fn check_slime_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    random: &mut RandomSource,
) -> bool {
    if level.difficulty() == Difficulty::Peaceful {
        return false;
    }
    if spawn_reason.is_spawner() {
        return <SlimeEntity as Mob>::check_mob_spawn_rules(entity_type, level, spawn_reason, pos);
    }

    let y = pos.y();
    let surface_slime = level
        .biome_at(pos)
        .is_some_and(|biome| biome.has_tag(&BiomeTag::ALLOWS_SURFACE_SLIME_SPAWNS))
        && y > SURFACE_SLIME_MIN_Y
        && y < SURFACE_SLIME_MAX_Y
        && float_attribute(level, SURFACE_SLIME_SPAWN_CHANCE, 0.0) > random.next_f32()
        && i32::from(level.max_local_raw_brightness(pos, level.sky_darkening()))
            <= random.next_i32_bounded(SURFACE_SLIME_BRIGHTNESS_BOUND);
    if surface_slime {
        return <SlimeEntity as Mob>::check_mob_spawn_rules(entity_type, level, spawn_reason, pos);
    }

    let chunk_x = pos.x() >> 4;
    let chunk_z = pos.z() >> 4;
    let slime_chunk =
        LegacyRandom::from_seed(slime_chunk_seed(chunk_x, chunk_z, level.seed()) as u64)
            .next_i32_bounded(10)
            == 0;
    random.next_i32_bounded(10) == 0
        && slime_chunk
        && y < SLIME_CHUNK_MAX_Y
        && <SlimeEntity as Mob>::check_mob_spawn_rules(entity_type, level, spawn_reason, pos)
}

/// Vanilla `WorldgenRandom.seedSlimeChunk(chunkX, chunkZ, seed, 987234911L)`.
///
/// The `x * x * 4987142` product wraps in `i32` before widening, exactly like the
/// vanilla bytecode's `imul`/`i2l` pair.
fn slime_chunk_seed(chunk_x: i32, chunk_z: i32, seed: i64) -> i64 {
    (seed
        + i64::from(
            chunk_x
                .wrapping_mul(chunk_x)
                .wrapping_mul(SLIME_CHUNK_SEED_X_SQUARED),
        )
        + i64::from(chunk_x.wrapping_mul(SLIME_CHUNK_SEED_X))
        + i64::from(chunk_z.wrapping_mul(chunk_z)).wrapping_mul(SLIME_CHUNK_SEED_Z_SQUARED)
        + i64::from(chunk_z.wrapping_mul(SLIME_CHUNK_SEED_Z)))
        ^ SLIME_CHUNK_SEED_SALT
}

/// Vanilla `Hoglin.checkHoglinSpawnRules`.
fn check_hoglin_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    _random: &mut RandomSource,
) -> bool {
    <HoglinEntity as Mob>::check_mob_spawn_rules(entity_type, level, spawn_reason, pos)
}

/// Vanilla `Strider.checkStriderSpawnRules`.
pub(super) fn check_strider_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    random: &mut RandomSource,
) -> bool {
    StriderEntity::check_strider_spawn_rules(entity_type, level, spawn_reason, pos, random)
}

/// Vanilla `Piglin.checkPiglinSpawnRules`.
fn check_piglin_placement_spawn_rules(
    _entity_type: EntityTypeRef,
    level: &World,
    _spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    _random: &mut RandomSource,
) -> bool {
    check_piglin_spawn_rules(level, pos)
}

/// Vanilla `ZombifiedPiglin.checkZombifiedPiglinSpawnRules`.
fn check_zombified_piglin_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    _random: &mut RandomSource,
) -> bool {
    <ZombifiedPiglinEntity as Mob>::check_mob_spawn_rules(entity_type, level, spawn_reason, pos)
}

/// Vanilla `PatrollingMonster.checkPatrollingMonsterSpawnRules`.
fn check_patrolling_monster_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    _random: &mut RandomSource,
) -> bool {
    if level.light_value_at(LightLayer::Block, pos) > 8 {
        return false;
    }
    <ZombieEntity as Monster>::check_any_light_monster_spawn_rules(
        entity_type,
        level,
        spawn_reason,
        pos,
    )
}

/// Vanilla `Mob.checkMobSpawnRules`, the predicate of the types vanilla
/// registers with the base `Mob` rule.
fn check_default_mob_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    _random: &mut RandomSource,
) -> bool {
    <PigEntity as Mob>::check_mob_spawn_rules(entity_type, level, spawn_reason, pos)
}

/// One `SpawnPlacements` row.
pub struct SpawnPlacementsData {
    /// Entity type this row applies to.
    pub entity_type: EntityTypeRef,
    /// Vanilla `Data.heightMap`.
    pub heightmap: HeightmapType,
    /// Vanilla `Data.placement`.
    pub placement: SpawnPlacementType,
    /// Vanilla `Data.predicate`; `None` means vanilla's missing-entry default of
    /// "predicate absent" (always `true`).
    pub predicate: Option<SpawnPredicate>,
}

/// Vanilla `SpawnPlacements.DEFAULT` heightmap.
pub const DEFAULT_HEIGHTMAP: HeightmapType = HeightmapType::MotionBlockingNoLeaves;

/// Vanilla `Slime.checkSlimeSpawnRules` surface branch: `getY() > 50`.
const SURFACE_SLIME_MIN_Y: i32 = 50;
/// Vanilla `Slime.checkSlimeSpawnRules` surface branch: `getY() < 70`.
const SURFACE_SLIME_MAX_Y: i32 = 70;
/// Vanilla `Slime.checkSlimeSpawnRules` surface branch: `getMaxLocalRawBrightness() <= random.nextInt(8)`.
const SURFACE_SLIME_BRIGHTNESS_BOUND: i32 = 8;
/// Vanilla `Slime.checkSlimeSpawnRules` worldgen branch: `getY() < 40`.
const SLIME_CHUNK_MAX_Y: i32 = 40;
/// Vanilla `EnvironmentAttributes.SURFACE_SLIME_SPAWN_CHANCE` key (default `0.0`).
const SURFACE_SLIME_SPAWN_CHANCE: &str = "minecraft:gameplay/surface_slime_spawn_chance";
/// Vanilla `WorldgenRandom.seedSlimeChunk` salt.
const SLIME_CHUNK_SEED_SALT: i64 = 987_234_911;
/// Vanilla `WorldgenRandom.seedSlimeChunk` `x * x` coefficient.
const SLIME_CHUNK_SEED_X_SQUARED: i32 = 4_987_142;
/// Vanilla `WorldgenRandom.seedSlimeChunk` `x` coefficient.
const SLIME_CHUNK_SEED_X: i32 = 5_947_611;
/// Vanilla `WorldgenRandom.seedSlimeChunk` `z * z` coefficient.
const SLIME_CHUNK_SEED_Z_SQUARED: i64 = 4_392_871;
/// Vanilla `WorldgenRandom.seedSlimeChunk` `z` coefficient.
const SLIME_CHUNK_SEED_Z: i32 = 389_711;

/// Vanilla `SpawnPlacements`: the per-type placement table and queries.
pub struct SpawnPlacements;

impl SpawnPlacements {
    /// Vanilla `SpawnPlacements.DATA_BY_TYPE`: every entity type Steel has a
    /// row for, in vanilla registration order.
    pub const DATA_BY_TYPE: &'static [SpawnPlacementsData] = &[
        SpawnPlacementsData {
            entity_type: &vanilla_entities::AXOLOTL,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::InWater,
            predicate: Some(check_axolotl_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::COD,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::InWater,
            predicate: Some(check_surface_water_animal_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::DOLPHIN,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::InWater,
            predicate: Some(check_dolphin_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::DROWNED,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::InWater,
            predicate: Some(check_drowned_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::GUARDIAN,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::InWater,
            predicate: Some(check_guardian_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::PUFFERFISH,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::InWater,
            predicate: Some(check_surface_water_animal_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::SALMON,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::InWater,
            predicate: Some(check_surface_water_animal_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::SQUID,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::InWater,
            predicate: Some(check_squid_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::TROPICAL_FISH,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::InWater,
            predicate: Some(check_tropical_fish_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::ARMADILLO,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_armadillo_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::BAT,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_bat_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::BLAZE,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_any_light_monster_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::BOGGED,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_monster_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::BREEZE,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_any_light_monster_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::CAMEL,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_camel_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::CAMEL_HUSK,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_surface_monsters_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::CAVE_SPIDER,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_monster_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::CHICKEN,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_animal_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::COW,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_animal_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::CREEPER,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_monster_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::DONKEY,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_animal_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::ENDERMAN,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_monster_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::ENDERMITE,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_endermite_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::ENDER_DRAGON,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_default_mob_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::FROG,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_frog_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::GHAST,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_ghast_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::HAPPY_GHAST,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_animal_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::GIANT,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_monster_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::GLOW_SQUID,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::InWater,
            predicate: Some(check_glow_squid_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::GOAT,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_goat_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::HORSE,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_animal_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::HUSK,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_surface_monsters_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::IRON_GOLEM,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_default_mob_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::LLAMA,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_animal_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::MAGMA_CUBE,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_magma_cube_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::SULFUR_CUBE,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_sulfur_cube_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::MOOSHROOM,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_mushroom_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::MULE,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_animal_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::NAUTILUS,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::InWater,
            predicate: Some(check_nautilus_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::OCELOT,
            heightmap: HeightmapType::MotionBlocking,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_ocelot_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::PARROT,
            heightmap: HeightmapType::MotionBlocking,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_parrot_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::PIG,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_animal_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::HOGLIN,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_hoglin_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::PIGLIN,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_piglin_placement_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::PILLAGER,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_patrolling_monster_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::POLAR_BEAR,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_polar_bear_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::RABBIT,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_rabbit_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::SHEEP,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_animal_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::SILVERFISH,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_silverfish_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::SKELETON,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_monster_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::SKELETON_HORSE,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_skeleton_horse_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::SLIME,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_slime_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::SNOW_GOLEM,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_default_mob_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::SPIDER,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_monster_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::STRAY,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_stray_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::PARCHED,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_surface_monsters_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::STRIDER,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::InLava,
            predicate: Some(check_strider_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::TURTLE,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_turtle_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::VILLAGER,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_default_mob_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::WITCH,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_monster_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::WITHER,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_monster_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::WITHER_SKELETON,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_monster_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::WOLF,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_wolf_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::ZOGLIN,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_any_light_monster_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::CREAKING,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_monster_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::ZOMBIE,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_monster_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::ZOMBIE_HORSE,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_monster_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::ZOMBIFIED_PIGLIN,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_zombified_piglin_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::ZOMBIE_VILLAGER,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_monster_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::CAT,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_animal_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::ELDER_GUARDIAN,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::InWater,
            predicate: Some(check_guardian_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::EVOKER,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::NoRestrictions,
            predicate: Some(check_monster_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::FOX,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::NoRestrictions,
            predicate: Some(check_fox_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::ILLUSIONER,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::NoRestrictions,
            predicate: Some(check_monster_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::PANDA,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::NoRestrictions,
            predicate: Some(check_animal_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::PHANTOM,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::NoRestrictions,
            predicate: Some(check_default_mob_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::RAVAGER,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_monster_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::SHULKER,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::NoRestrictions,
            predicate: Some(check_default_mob_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::TRADER_LLAMA,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::NoRestrictions,
            predicate: Some(check_animal_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::VEX,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::NoRestrictions,
            predicate: Some(check_monster_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::VINDICATOR,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::NoRestrictions,
            predicate: Some(check_monster_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::WANDERING_TRADER,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_default_mob_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::WARDEN,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::NoRestrictions,
            predicate: Some(check_monster_spawn_rules),
        },
    ];

    /// Vanilla `SpawnPlacements.DATA_BY_TYPE` lookup.
    #[must_use]
    pub fn data_for(entity_type: EntityTypeRef) -> Option<&'static SpawnPlacementsData> {
        Self::DATA_BY_TYPE
            .iter()
            .find(|data| ptr::eq(data.entity_type, entity_type))
    }

    /// Vanilla `SpawnPlacements.getPlacementType`.
    #[must_use]
    pub fn placement_type(entity_type: EntityTypeRef) -> SpawnPlacementType {
        Self::data_for(entity_type)
            .map_or(SpawnPlacementType::NoRestrictions, |data| data.placement)
    }

    /// Vanilla `SpawnPlacements.getHeightmapType`.
    #[must_use]
    pub fn heightmap_type(entity_type: EntityTypeRef) -> HeightmapType {
        Self::data_for(entity_type).map_or(DEFAULT_HEIGHTMAP, |data| data.heightmap)
    }

    /// Vanilla `SpawnPlacements.isSpawnPositionOk`.
    #[must_use]
    pub fn is_spawn_position_ok(
        entity_type: EntityTypeRef,
        level: &dyn LevelReader,
        pos: BlockPos,
    ) -> bool {
        Self::placement_type(entity_type).is_spawn_position_ok(level, pos, entity_type)
    }

    /// Vanilla `SpawnPlacements.checkSpawnRules`.
    ///
    /// Vanilla passes a `ServerLevelAccessor`; Steel's rule helpers need the live
    /// `World` because they read light layers and dimension spawn-light data.
    pub fn check_spawn_rules(
        entity_type: EntityTypeRef,
        level: &World,
        spawn_reason: EntitySpawnReason,
        pos: BlockPos,
        random: &mut RandomSource,
    ) -> bool {
        if !entity_type.allowed_in_peaceful && level.difficulty() == Difficulty::Peaceful {
            return false;
        }

        match Self::data_for(entity_type).and_then(|data| data.predicate) {
            Some(predicate) => predicate(entity_type, level, spawn_reason, pos, random),
            None => true,
        }
    }
}

/// Vanilla `BlockState.isCollisionShapeFullBlock`.
#[must_use]
pub fn is_collision_shape_full_block(state: BlockStateId, pos: BlockPos) -> bool {
    is_offset_shape_full_block(state.get_collision_shape_at(pos))
}

/// Vanilla `NaturalSpawner.isValidEmptySpawnBlock`.
#[must_use]
pub fn is_valid_empty_spawn_block(
    level: &dyn LevelReader,
    pos: BlockPos,
    state: BlockStateId,
    entity_type: EntityTypeRef,
) -> bool {
    let _ = level;
    if is_collision_shape_full_block(state, pos) {
        return false;
    }

    if state
        .get_block()
        .has_tag(&BlockTag::PREVENT_MOB_SPAWNING_INSIDE)
    {
        return false;
    }

    let fluid = state.get_fluid_state();
    if !fluid.is_empty() {
        return false;
    }

    !is_block_dangerous(entity_type, state)
}

#[cfg(test)]
mod tests {
    use std::ptr;
    use std::sync::Arc;

    use steel_registry::RegistryEntry as _;
    use steel_registry::vanilla_biomes;
    use steel_registry::vanilla_dimension_types;
    use steel_registry::vanilla_world_clocks;
    use steel_registry::{vanilla_blocks, vanilla_entities};
    use steel_utils::random::RandomSource;
    use steel_utils::random::legacy_random::LegacyRandom;
    use steel_utils::types::UpdateFlags;
    use steel_utils::{BlockPos, ChunkPos};

    use super::*;
    use crate::bootstrap::init_globals;
    use crate::chunk::light::{LightSection, LightSectionData};
    use crate::test_support::{
        TestLevel, fresh_test_world_with_dimension_type, insert_ready_full_chunk,
    };

    /// A fresh world with `biome` written into every biome quart cell of `chunk`.
    ///
    /// The biome is written into the whole chunk (all sections, all quads) so the
    /// fuzzed `biome_at` lookup — which samples the quart corners around `pos - 2` —
    /// stays inside the prepared chunk for positions at local 8.
    fn prepare_chunk(world: &Arc<World>, chunk: ChunkPos, biome_id: u16) {
        insert_ready_full_chunk(world, chunk);
        world
            .chunk_map
            .with_full_chunk(chunk, |chunk| {
                for holder in &chunk.sections().sections {
                    let mut section = holder.write();
                    for quart_x in 0..4 {
                        for quart_y in 0..4 {
                            for quart_z in 0..4 {
                                section.set_biome(quart_x, quart_y, quart_z, biome_id);
                            }
                        }
                    }
                }
            })
            .expect("the prepared chunk should be loaded");
    }

    fn legacy_random(seed: u64) -> RandomSource {
        RandomSource::Legacy(LegacyRandom::from_seed(seed))
    }

    /// Publishes a zero sky-light section so the vanilla brightness checks see darkness.
    ///
    /// Fresh test chunks keep every `LightSection::missing`, and `get_sky_light_value`
    /// falls back to a fully-lit `MAX_LIGHT_LEVEL` for those — a brightness the
    /// surface-slime roll can never accept.
    fn darken_sky(world: &Arc<World>, chunk: ChunkPos, section_y: i32) {
        world
            .chunk_map
            .with_full_chunk(chunk, |chunk| {
                let mut light = chunk.common().light_mut();
                if let Some(section) = light.sky.section_mut(section_y) {
                    *section = LightSection::visible(LightSectionData::homogeneous(0));
                }
            })
            .expect("the prepared chunk should be loaded");
    }

    /// The 10 types the table used to leave out now carry vanilla's placement type and
    /// reject a bare-air candidate. Before the rows existed `placement_type` fell back to
    /// `NO_RESTRICTIONS` and every position passed.
    ///
    /// Vanilla `SpawnPlacements` static block: `COD`/`PUFFERFISH`/`SALMON`/`TROPICAL_FISH`
    /// and `NAUTILUS` are `IN_WATER`, `RABBIT`/`CAMEL_HUSK`/`HAPPY_GHAST`/`GIANT`/
    /// `SULFUR_CUBE` are `ON_GROUND`.
    #[test]
    fn previously_missing_rows_now_reject_empty_air_positions() {
        init_globals();
        let land = TestLevel::default();
        let pos = BlockPos::new(4, 64, 4);

        for entity_type in [
            &vanilla_entities::COD,
            &vanilla_entities::SALMON,
            &vanilla_entities::TROPICAL_FISH,
            &vanilla_entities::PUFFERFISH,
            &vanilla_entities::NAUTILUS,
        ] {
            assert_eq!(
                SpawnPlacements::placement_type(entity_type),
                SpawnPlacementType::InWater,
                "{} must keep vanilla's IN_WATER placement",
                entity_type.key
            );
            assert!(
                !SpawnPlacements::is_spawn_position_ok(entity_type, &land, pos),
                "{} must not be placeable in air",
                entity_type.key
            );
        }

        for entity_type in [
            &vanilla_entities::RABBIT,
            &vanilla_entities::CAMEL_HUSK,
            &vanilla_entities::HAPPY_GHAST,
            &vanilla_entities::GIANT,
            &vanilla_entities::SULFUR_CUBE,
        ] {
            assert_eq!(
                SpawnPlacements::placement_type(entity_type),
                SpawnPlacementType::OnGround,
                "{} must keep vanilla's ON_GROUND placement",
                entity_type.key
            );
            assert!(
                !SpawnPlacements::is_spawn_position_ok(entity_type, &land, pos),
                "{} must not be placeable in air",
                entity_type.key
            );
        }

        // Water at the candidate and plain air above satisfies `IN_WATER`.
        let water = TestLevel::default()
            .with_block(pos, vanilla_blocks::WATER.default_state())
            .with_block(pos.above(), vanilla_blocks::AIR.default_state());
        assert!(
            SpawnPlacements::is_spawn_position_ok(&vanilla_entities::COD, &water, pos),
            "a water cell with air above satisfies IN_WATER"
        );
    }

    /// The water anchors are registered, so the missing-row default of "no predicate ⇒
    /// always true" no longer lets them spawn anywhere.
    #[test]
    fn water_rows_exist_and_bind_their_vanilla_predicates() {
        init_globals();
        assert!(SpawnPlacements::data_for(&vanilla_entities::COD).is_some());
        assert!(SpawnPlacements::data_for(&vanilla_entities::NAUTILUS).is_some());
        assert!(SpawnPlacements::data_for(&vanilla_entities::RABBIT).is_some());
        assert!(SpawnPlacements::data_for(&vanilla_entities::CAMEL_HUSK).is_some());
        assert!(SpawnPlacements::data_for(&vanilla_entities::HAPPY_GHAST).is_some());
        assert!(SpawnPlacements::data_for(&vanilla_entities::GIANT).is_some());
        assert!(SpawnPlacements::data_for(&vanilla_entities::SULFUR_CUBE).is_some());

        assert!(
            SpawnPlacements::data_for(&vanilla_entities::POLAR_BEAR).is_some_and(|row| row
                .predicate
                .is_some_and(|predicate| ptr::eq(
                    predicate as *const (),
                    check_polar_bear_spawn_rules as *const ()
                )))
        );
        assert!(
            SpawnPlacements::data_for(&vanilla_entities::SLIME).is_some_and(|row| row
                .predicate
                .is_some_and(|predicate| ptr::eq(
                    predicate as *const (),
                    check_slime_spawn_rules as *const ()
                )))
        );
    }

    /// A dry candidate is refused by both halves of the vanilla check, and a surface-water
    /// column below sea level is accepted — through the real `SpawnPlacements` lookup.
    #[test]
    fn water_animal_rules_require_surface_water() {
        init_globals();
        let fixture = fresh_test_world_with_dimension_type(
            "water_animal_rules",
            "water_animal_rules",
            &vanilla_dimension_types::OVERWORLD,
        );
        let world = &fixture.world;
        prepare_chunk(
            world,
            ChunkPos::new(0, 0),
            vanilla_biomes::PLAINS.id() as u16,
        );

        // Sea level is 63: cod/salmon/pufferfish accept 50..=63, nautilus 38..=58.
        let dry = BlockPos::new(8, 58, 8);
        for entity_type in [
            &vanilla_entities::COD,
            &vanilla_entities::SALMON,
            &vanilla_entities::TROPICAL_FISH,
            &vanilla_entities::PUFFERFISH,
            &vanilla_entities::NAUTILUS,
        ] {
            assert!(
                !SpawnPlacements::is_spawn_position_ok(entity_type, world.as_ref(), dry),
                "{} must refuse a dry cell",
                entity_type.key
            );
            assert!(
                !SpawnPlacements::check_spawn_rules(
                    entity_type,
                    world,
                    EntitySpawnReason::Natural,
                    dry,
                    &mut legacy_random(1),
                ),
                "{} must refuse a dry cell",
                entity_type.key
            );
        }

        // A full water column at the candidate: fluid water at `pos`, water below and a
        // water block above.
        let wet = BlockPos::new(8, 58, 8);
        let water = vanilla_blocks::WATER.default_state();
        for offset in [-1, 0, 1] {
            assert!(
                world.set_block(wet.offset(0, offset, 0), water, UpdateFlags::UPDATE_NONE),
                "water should be placeable at offset {offset}"
            );
        }

        for entity_type in [
            &vanilla_entities::COD,
            &vanilla_entities::SALMON,
            &vanilla_entities::TROPICAL_FISH,
            &vanilla_entities::PUFFERFISH,
            &vanilla_entities::NAUTILUS,
        ] {
            assert!(
                SpawnPlacements::is_spawn_position_ok(entity_type, world.as_ref(), wet),
                "{} must accept a water cell",
                entity_type.key
            );
            assert!(
                SpawnPlacements::check_spawn_rules(
                    entity_type,
                    world,
                    EntitySpawnReason::Natural,
                    wet,
                    &mut legacy_random(1),
                ),
                "{} must accept a surface-water column",
                entity_type.key
            );
        }

        // Above the cod band the surface rule refuses even with water present.
        let high = BlockPos::new(8, 64, 8);
        for offset in [-1, 0, 1] {
            world.set_block(high.offset(0, offset, 0), water, UpdateFlags::UPDATE_NONE);
        }
        assert!(
            !SpawnPlacements::check_spawn_rules(
                &vanilla_entities::COD,
                world,
                EntitySpawnReason::Natural,
                high,
                &mut legacy_random(1),
            ),
            "a water cell above the cod band is refused"
        );
    }

    /// `RABBIT` is `ON_GROUND`, and its rule needs a rabbit-spawnable block below and
    /// enough light — a mid-air candidate is refused by both halves of the check.
    #[test]
    fn rabbit_rule_requires_spawnable_ground() {
        init_globals();
        let fixture = fresh_test_world_with_dimension_type(
            "rabbit_rule",
            "rabbit_rule",
            &vanilla_dimension_types::OVERWORLD,
        );
        let world = &fixture.world;
        prepare_chunk(
            world,
            ChunkPos::new(0, 0),
            vanilla_biomes::PLAINS.id() as u16,
        );

        let air = BlockPos::new(8, 64, 8);
        assert!(
            !SpawnPlacements::is_spawn_position_ok(&vanilla_entities::RABBIT, world.as_ref(), air),
            "a mid-air rabbit candidate is not ON_GROUND"
        );
        assert!(
            !SpawnPlacements::check_spawn_rules(
                &vanilla_entities::RABBIT,
                world,
                EntitySpawnReason::Natural,
                air,
                &mut legacy_random(1),
            ),
            "the rabbit rule needs a rabbits-spawnable block below"
        );

        // Grass below in an open-air column satisfies both halves.
        assert!(world.set_block(
            air.below(),
            vanilla_blocks::GRASS_BLOCK.default_state(),
            UpdateFlags::UPDATE_NONE
        ));
        assert!(
            SpawnPlacements::is_spawn_position_ok(&vanilla_entities::RABBIT, world.as_ref(), air),
            "grass below and air at/above is a valid ON_GROUND rabbit cell"
        );
        assert!(
            SpawnPlacements::check_spawn_rules(
                &vanilla_entities::RABBIT,
                world,
                EntitySpawnReason::Natural,
                air,
                &mut legacy_random(1),
            ),
            "grass below and a bright open-air column satisfies the rabbit rule"
        );

        // Stone is not rabbits-spawnable.
        assert!(world.set_block(
            air.below(),
            vanilla_blocks::STONE.default_state(),
            UpdateFlags::UPDATE_NONE
        ));
        assert!(
            !SpawnPlacements::check_spawn_rules(
                &vanilla_entities::RABBIT,
                world,
                EntitySpawnReason::Natural,
                air,
                &mut legacy_random(1),
            ),
            "stone below is not in the rabbits-spawnable tag"
        );
    }

    /// The polar bear rule switches on `POLAR_BEARS_SPAWN_ON_ALTERNATE_BLOCKS`: tagged
    /// biomes require a bright `ice` floor, everything else uses the animal rule.
    #[test]
    fn polar_bear_rule_switches_on_the_biome_tag() {
        init_globals();
        let fixture = fresh_test_world_with_dimension_type(
            "polar_bear_rule",
            "polar_bear_rule",
            &vanilla_dimension_types::OVERWORLD,
        );
        let world = &fixture.world;
        let chunk = ChunkPos::new(0, 0);
        prepare_chunk(world, chunk, vanilla_biomes::FROZEN_OCEAN.id() as u16);

        let pos = BlockPos::new(8, 64, 8);
        // Tagged biome + ice below is the alternate-blocks branch.
        assert!(world.set_block(
            pos.below(),
            vanilla_blocks::ICE.default_state(),
            UpdateFlags::UPDATE_NONE
        ));
        assert!(
            SpawnPlacements::check_spawn_rules(
                &vanilla_entities::POLAR_BEAR,
                world,
                EntitySpawnReason::Natural,
                pos,
                &mut legacy_random(1),
            ),
            "frozen ocean with ice below takes the alternate-blocks branch"
        );
        assert!(
            SpawnPlacements::is_spawn_position_ok(
                &vanilla_entities::POLAR_BEAR,
                world.as_ref(),
                pos
            ),
            "the polar bear row keeps vanilla's ON_GROUND placement"
        );

        // A non-alternate floor fails the tagged branch.
        assert!(world.set_block(
            pos.below(),
            vanilla_blocks::GRASS_BLOCK.default_state(),
            UpdateFlags::UPDATE_NONE
        ));
        assert!(
            !SpawnPlacements::check_spawn_rules(
                &vanilla_entities::POLAR_BEAR,
                world,
                EntitySpawnReason::Natural,
                pos,
                &mut legacy_random(1),
            ),
            "the tagged branch needs an alternate-block (ice) floor"
        );

        // An untagged biome ignores ice entirely and falls back to the animal rule.
        let plains_chunk = ChunkPos::new(4, 4);
        prepare_chunk(world, plains_chunk, vanilla_biomes::PLAINS.id() as u16);
        let plains_pos = BlockPos::new((4 << 4) + 8, 64, (4 << 4) + 8);
        assert!(world.set_block(
            plains_pos.below(),
            vanilla_blocks::ICE.default_state(),
            UpdateFlags::UPDATE_NONE
        ));
        assert!(
            !SpawnPlacements::check_spawn_rules(
                &vanilla_entities::POLAR_BEAR,
                world,
                EntitySpawnReason::Natural,
                plains_pos,
                &mut legacy_random(1),
            ),
            "plains takes the animal rule, where ice is not an animal-spawnable block"
        );
        assert!(world.set_block(
            plains_pos.below(),
            vanilla_blocks::GRASS_BLOCK.default_state(),
            UpdateFlags::UPDATE_NONE
        ));
        assert!(
            SpawnPlacements::check_spawn_rules(
                &vanilla_entities::POLAR_BEAR,
                world,
                EntitySpawnReason::Natural,
                plains_pos,
                &mut legacy_random(1),
            ),
            "plains with grass below satisfies Animal.checkAnimalSpawnRules"
        );
    }

    /// The slime rule takes the swamp surface branch only inside the `(50, 70)` band, the
    /// surface-slime chance roll and the brightness roll, and the slime-chunk branch only
    /// below Y 40.
    #[test]
    fn slime_rule_accepts_swamp_surface_and_rejects_elsewhere() {
        init_globals();
        let fixture = fresh_test_world_with_dimension_type(
            "slime_rule",
            "slime_rule",
            &vanilla_dimension_types::OVERWORLD,
        );
        let world = &fixture.world;
        let chunk = ChunkPos::new(0, 0);
        prepare_chunk(world, chunk, vanilla_biomes::SWAMP.id() as u16);
        // Full moon (`moon.json` keyframe 0) lifts the surface-slime chance to 0.5.
        world.set_clock_total_ticks(&vanilla_world_clocks::OVERWORLD, 0);

        let pos = BlockPos::new(8, 60, 8);
        assert!(world.set_block(
            pos.below(),
            vanilla_blocks::STONE.default_state(),
            UpdateFlags::UPDATE_NONE
        ));
        darken_sky(world, chunk, pos.y() >> 4);

        let mut random = first_random_with_float_below(0.5);
        assert!(
            SpawnPlacements::check_spawn_rules(
                &vanilla_entities::SLIME,
                world,
                EntitySpawnReason::Natural,
                pos,
                &mut random.clone(),
            ),
            "a dark swamp cell in the (50, 70) band accepts the surface-slime branch"
        );

        // Outside the Y band the surface branch is refused and Y 60 is not a slime-chunk Y.
        let high = BlockPos::new(9, 80, 8);
        assert!(world.set_block(
            high.below(),
            vanilla_blocks::STONE.default_state(),
            UpdateFlags::UPDATE_NONE
        ));
        assert!(
            !SpawnPlacements::check_spawn_rules(
                &vanilla_entities::SLIME,
                world,
                EntitySpawnReason::Natural,
                high,
                &mut random.clone(),
            ),
            "above Y 70 the swamp branch is refused"
        );

        // An untagged biome never takes the surface branch.
        let plains_chunk = ChunkPos::new(8, 8);
        prepare_chunk(world, plains_chunk, vanilla_biomes::PLAINS.id() as u16);
        let plains_pos = BlockPos::new((8 << 4) + 8, 60, (8 << 4) + 8);
        assert!(world.set_block(
            plains_pos.below(),
            vanilla_blocks::STONE.default_state(),
            UpdateFlags::UPDATE_NONE
        ));
        assert!(
            !SpawnPlacements::check_spawn_rules(
                &vanilla_entities::SLIME,
                world,
                EntitySpawnReason::Natural,
                plains_pos,
                &mut random.clone(),
            ),
            "plains is not in the surface-slime biome tag"
        );

        // With the moon timeline back at its `0.0` default the chance roll fails.
        world.set_clock_total_ticks(&vanilla_world_clocks::OVERWORLD, 96_000);
        assert!(
            !SpawnPlacements::check_spawn_rules(
                &vanilla_entities::SLIME,
                world,
                EntitySpawnReason::Natural,
                pos,
                &mut random,
            ),
            "a new-moon surface-slime chance of 0.0 refuses the swamp branch"
        );
    }

    /// The worldgen slime-chunk branch: below Y 40, a slime chunk and a `nextInt(10) == 0`
    /// roll are all required, and a spawner short-circuits to the shared mob rule.
    #[test]
    fn slime_rule_requires_the_slime_chunk_roll_below_y_40() {
        init_globals();
        let fixture = fresh_test_world_with_dimension_type(
            "slime_chunk_rule",
            "slime_chunk_rule",
            &vanilla_dimension_types::OVERWORLD,
        );
        let world = &fixture.world;

        let slime_chunk = (0..64)
            .flat_map(|x| (0..64).map(move |z| ChunkPos::new(x, z)))
            .find(|chunk| {
                LegacyRandom::from_seed(slime_chunk_seed(chunk.0.x, chunk.0.y, world.seed()) as u64)
                    .next_i32_bounded(10)
                    == 0
            })
            .expect("seed 0 must have a slime chunk near the origin");

        // `PLAINS` keeps the surface branch out of the way.
        prepare_chunk(world, slime_chunk, vanilla_biomes::PLAINS.id() as u16);
        let pos = BlockPos::new((slime_chunk.0.x << 4) + 8, 20, (slime_chunk.0.y << 4) + 8);
        assert!(world.set_block(
            pos.below(),
            vanilla_blocks::STONE.default_state(),
            UpdateFlags::UPDATE_NONE
        ));

        let mut random = first_random_with_int_zero(10);
        assert!(
            SpawnPlacements::check_spawn_rules(
                &vanilla_entities::SLIME,
                world,
                EntitySpawnReason::Natural,
                pos,
                &mut random,
            ),
            "a slime chunk below Y 40 with a passing roll accepts the slime"
        );

        // A random that misses the `nextInt(10) == 0` roll refuses the same cell.
        let mut missing = first_random_with_int_not_zero(10);
        assert!(
            !SpawnPlacements::check_spawn_rules(
                &vanilla_entities::SLIME,
                world,
                EntitySpawnReason::Natural,
                pos,
                &mut missing.clone(),
            ),
            "the slime-chunk branch requires nextInt(10) == 0"
        );

        // A spawner bypasses every branch and delegates to the shared mob rule.
        assert!(
            SpawnPlacements::check_spawn_rules(
                &vanilla_entities::SLIME,
                world,
                EntitySpawnReason::Spawner,
                pos,
                &mut missing,
            ),
            "spawners short-circuit to Mob.checkMobSpawnRules"
        );
    }

    /// Finds a seeded legacy random whose first float is below `threshold`.
    fn first_random_with_float_below(threshold: f32) -> RandomSource {
        (0..10_000u64)
            .map(legacy_random)
            .find(|random| random.clone().next_f32() < threshold)
            .expect("some seed must draw a float below the threshold")
    }

    /// Finds a seeded legacy random whose first bounded int draw is zero.
    fn first_random_with_int_zero(bound: i32) -> RandomSource {
        (0..10_000u64)
            .map(legacy_random)
            .find(|random| random.clone().next_i32_bounded(bound) == 0)
            .expect("some seed must draw a zero first")
    }

    /// Finds a seeded legacy random whose first bounded int draw is non-zero.
    fn first_random_with_int_not_zero(bound: i32) -> RandomSource {
        (0..10_000u64)
            .map(legacy_random)
            .find(|random| random.clone().next_i32_bounded(bound) != 0)
            .expect("some seed must draw a non-zero first")
    }
}
