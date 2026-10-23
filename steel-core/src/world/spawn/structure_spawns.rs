//! The structure-aware half of vanilla `ChunkGenerator.getMobsAt` and
//! `NaturalSpawner.isInNetherFortressBounds`: which mob list applies at a
//! position once structure spawn overrides are taken into account.
//!
//! Vanilla asks `StructureManager` for the structures covering a position and
//! walks their `spawnOverrides`; Steel's chunks already carry vanilla's two
//! structure maps (`structure_starts` for starts originating in the chunk and
//! `structure_references` pointing at origin chunks), so this module is the
//! read-only query layer over them.

use steel_registry::RegistryExt as _;
use steel_registry::biome::SpawnerData;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::entity_type::MobCategory;
use steel_registry::structure::{
    StructureRef, StructureSpawnBoundingBox, StructureSpawnOverrideData,
};
use steel_registry::{REGISTRY, vanilla_blocks};
use steel_utils::{BlockPos, BoundingBox, ChunkPos, Identifier};
use steel_worldgen::structure::StructurePiece;

use crate::chunk::status::ChunkStatus;
use crate::world::{LevelReader as _, World};

/// Vanilla `NetherFortressStructure.FORTRESS_ENEMIES`.
///
/// Vanilla keeps this list as a code constant rather than datapack data and
/// applies it to the whole fortress start, unlike the piece-bounded
/// `spawn_overrides` the datapack carries for `minecraft:fortress`.
pub const FORTRESS_ENEMIES: &[SpawnerData] = &[
    SpawnerData {
        entity_type: Identifier::vanilla_static("blaze"),
        weight: 10,
        min_count: 2,
        max_count: 3,
    },
    SpawnerData {
        entity_type: Identifier::vanilla_static("zombified_piglin"),
        weight: 5,
        min_count: 4,
        max_count: 4,
    },
    SpawnerData {
        entity_type: Identifier::vanilla_static("wither_skeleton"),
        weight: 8,
        min_count: 5,
        max_count: 5,
    },
    SpawnerData {
        entity_type: Identifier::vanilla_static("skeleton"),
        weight: 2,
        min_count: 5,
        max_count: 5,
    },
    SpawnerData {
        entity_type: Identifier::vanilla_static("magma_cube"),
        weight: 3,
        min_count: 4,
        max_count: 4,
    },
];

/// Vanilla `NaturalSpawner.isInNetherFortressBounds`.
#[must_use]
pub fn is_in_nether_fortress_bounds(world: &World, pos: BlockPos, category: MobCategory) -> bool {
    if category != MobCategory::Monster
        || world.get_block_state(pos.below()).get_block() != &vanilla_blocks::NETHER_BRICKS
    {
        return false;
    }

    let Some(fortress) = REGISTRY
        .structures
        .by_key(&Identifier::vanilla_static("fortress"))
    else {
        return false;
    };

    // Vanilla `StructureManager.getStructureAt(pos, fortress).isValid()`.
    let mut found = false;
    for_each_start_at(world, pos, |start| {
        found |= start.structure == &fortress.key;
    });
    found
}

/// Vanilla `ChunkGenerator.getMobsAt`: the structure spawn override covering
/// `pos` for `category`, or `None` when the biome list applies.
///
/// Structures are visited in `structure_references` order, matching vanilla's
/// `getAllStructuresAt` walk over the chunk's reference map.
#[must_use]
pub fn structure_spawns_at(
    world: &World,
    category: MobCategory,
    pos: BlockPos,
) -> Option<&'static [SpawnerData]> {
    let mut spawns = None;
    for_each_start_at(world, pos, |start| {
        if spawns.is_some() {
            return;
        }

        let Some(structure) = REGISTRY.structures.by_key(start.structure) else {
            return;
        };
        let Some(override_data) = override_for(structure, category) else {
            return;
        };
        if start_in_override_bounds(override_data, start, pos) {
            spawns = Some(override_data.spawns.as_slice());
        }
    });
    spawns
}

/// The read view of a [`steel_worldgen::structure::StructureStart`] the bounds
/// checks need, borrowed from the chunk that owns it.
#[derive(Clone, Copy)]
struct StructureStartView<'a> {
    structure: &'a Identifier,
    pieces: &'a [StructurePiece],
    bounding_box: Option<BoundingBox>,
}

/// Vanilla `StructureManager.getAllStructuresAt` + `fillStartsForStructure`:
/// visits every start whose bounding box covers `pos`, resolved through the
/// structure references of the chunk containing it.
fn for_each_start_at(world: &World, pos: BlockPos, mut visit: impl FnMut(StructureStartView<'_>)) {
    let references: Vec<(StructureRef, Vec<ChunkPos>)> = world
        .chunk_map
        .with_chunk_at_status(
            ChunkPos::from_block_pos(pos),
            // Vanilla `StructureManager.getAllStructuresAt` reads the chunk at
            // `ChunkStatus.STRUCTURE_REFERENCES`.
            ChunkStatus::StructureReferences,
            |chunk| {
                chunk
                    .structure_references()
                    .iter()
                    // Vanilla resolves the reference key through the structure
                    // registry; an unregistered key yields no structure.
                    .filter_map(|(key, sources)| {
                        let structure = REGISTRY.structures.by_key(key)?;
                        Some((structure, sources.iter().copied().collect()))
                    })
                    .collect()
            },
        )
        .unwrap_or_default();

    for (structure, sources) in references {
        for source in sources {
            world
                .chunk_map
                .with_chunk_at_status(source, ChunkStatus::StructureStarts, |chunk| {
                    let starts = chunk.structure_starts();
                    let Some(start) = starts.get(&structure.key) else {
                        return;
                    };
                    let Some(bounding_box) = start.bounding_box else {
                        return;
                    };
                    if !bounding_box.contains_blockpos(pos) {
                        return;
                    }
                    visit(StructureStartView {
                        structure: &structure.key,
                        pieces: &start.pieces,
                        bounding_box: Some(bounding_box),
                    });
                });
        }
    }
}

/// Vanilla `Structure.spawnOverrides().get(category)`.
fn override_for(
    structure: StructureRef,
    category: MobCategory,
) -> Option<&'static StructureSpawnOverrideData> {
    structure
        .spawn_overrides
        .iter()
        .find(|override_data| override_data.category == category.name())
}

/// Vanilla's per-override bounds predicate: either the whole start box or one
/// of the pieces must contain `pos`.
fn start_in_override_bounds(
    override_data: &StructureSpawnOverrideData,
    start: StructureStartView<'_>,
    pos: BlockPos,
) -> bool {
    match override_data.bounding_box {
        StructureSpawnBoundingBox::Full => start
            .bounding_box
            .is_some_and(|bounding_box| bounding_box.contains_blockpos(pos)),
        StructureSpawnBoundingBox::Piece => start
            .pieces
            .iter()
            .any(|piece| piece.bounding_box.contains_blockpos(pos)),
    }
}
