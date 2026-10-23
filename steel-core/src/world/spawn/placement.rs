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
//! implementation. Rows exist only for entity types Steel implements a rule for:
//! a type with no row behaves like a vanilla type with no `SpawnPlacements` entry
//! (`NO_RESTRICTIONS`, motion-blocking-no-leaves heightmap, no predicate).

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::shapes::is_offset_shape_full_block;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::fluid::is_water_fluid;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_blocks;
use steel_registry::vanilla_entities;
use steel_registry::vanilla_fluids;
use steel_utils::random::RandomSource;
use steel_utils::{BlockPos, BlockStateId};

use crate::behavior::BLOCK_BEHAVIORS;
use crate::chunk::heightmap::HeightmapType;
use crate::entity::Animal;
use crate::entity::EntitySpawnReason;
use crate::entity::Mob;
use crate::entity::Monster;
use crate::entity::ai::path::PathComputationType;
use crate::entity::ai::walk::WalkPathEvaluator;
use crate::entity::entities::{
    ChickenEntity, CowEntity, LlamaEntity, MushroomCowEntity, PigEntity, SheepEntity,
    TraderLlamaEntity, ZombieEntity,
};
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
    crate::world::is_redstone_conductor(level, state, pos)
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

/// Vanilla `Mob.checkMobSpawnRules`.
fn check_mob_spawn_rules(
    entity_type: EntityTypeRef,
    level: &World,
    spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    _random: &mut RandomSource,
) -> bool {
    <PigEntity as Mob>::check_mob_spawn_rules(entity_type, level, spawn_reason, pos)
}

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

/// Vanilla `SpawnPlacements`: the per-type placement table and queries.
pub struct SpawnPlacements;

impl SpawnPlacements {
    /// Vanilla `SpawnPlacements.DATA_BY_TYPE`, restricted to types Steel has a rule for.
    pub const DATA_BY_TYPE: &'static [SpawnPlacementsData] = &[
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
            entity_type: &vanilla_entities::ENDERMITE,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_monster_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::IRON_GOLEM,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_mob_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::LLAMA,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_animal_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::MOOSHROOM,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_mushroom_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::PHANTOM,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::NoRestrictions,
            predicate: Some(check_mob_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::PIG,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_animal_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::PILLAGER,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_monster_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::SHEEP,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_animal_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::SHULKER,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::NoRestrictions,
            predicate: Some(check_mob_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::TRADER_LLAMA,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::NoRestrictions,
            predicate: Some(check_animal_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::VILLAGER,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_mob_spawn_rules),
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
            predicate: Some(check_mob_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::ZOMBIE,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_monster_spawn_rules),
        },
        SpawnPlacementsData {
            entity_type: &vanilla_entities::ZOMBIE_VILLAGER,
            heightmap: DEFAULT_HEIGHTMAP,
            placement: SpawnPlacementType::OnGround,
            predicate: Some(check_monster_spawn_rules),
        },
    ];

    /// Vanilla `SpawnPlacements.DATA_BY_TYPE` lookup.
    #[must_use]
    pub fn data_for(entity_type: EntityTypeRef) -> Option<&'static SpawnPlacementsData> {
        Self::DATA_BY_TYPE
            .iter()
            .find(|data| std::ptr::eq(data.entity_type, entity_type))
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
        if !entity_type.allowed_in_peaceful
            && level.difficulty() == steel_utils::types::Difficulty::Peaceful
        {
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
