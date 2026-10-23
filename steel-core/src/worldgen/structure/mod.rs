//! Core chunk adapter for the worldgen structure engine.

pub(crate) mod piece_placer;

use steel_registry::{Registry, RegistryExt as _};
use steel_utils::random::worldgen_random::WorldgenRandom;
use steel_utils::{BlockPos, BoundingBox, ChunkPos, Identifier};
use steel_worldgen::structure::StructureGenerationContext;

use crate::chunk::Chunk;
use crate::world::LevelReader as _;
use crate::world::LevelReader as _;
use crate::worldgen::template::TemplateLevelAccess;

pub use steel_worldgen::structure::{
    FixedStructureBiomeProvider, StructureGenerator, StructureLocateCandidate, StructureLocatePlan,
    StructureStart, squared_distance,
};

use piece_placer::StructurePiecePlacer;

/// Generates structure starts and writes them into the chunk.
///
/// This is vanilla's `ChunkGenerator.createStructures`. Vanilla guards the call
/// with `StructureCheck`, a per-chunk cache of "does a start exist here" answers
/// used by `/locate` so it can avoid loading chunks. Steel has no storage-side
/// cache: the structure-starts stage is the single place a chunk's starts are
/// produced, and `/locate` requests those chunks at `StructureStarts` instead of
/// scanning region files. Chunk-local results are identical (placement chunk +
/// frequency + exclusion zone, then the structure's own biome-validated
/// generation point); only the `/locate` chunk-loading strategy differs.
pub fn create_structures(
    generator: &StructureGenerator,
    chunk: &Chunk,
    ctx: &mut dyn StructureGenerationContext,
) {
    let starts = generator.generate_starts_for_chunk(ctx, |structure: &Identifier| {
        chunk
            .structure_starts()
            .get(structure)
            .is_some_and(|start| !start.pieces.is_empty())
    });

    if starts.is_empty() {
        return;
    }

    {
        let mut chunk_starts = chunk.structure_starts_mut();
        for start in starts {
            chunk_starts.insert(start.structure.clone(), start);
        }
    }
    chunk.mark_dirty();
}

/// Places a generated structure start into a live level.
///
/// This is the `/place structure` half of vanilla `StructureStart.placeInChunk`:
/// every chunk in the start's (inflated) bounding box is clipped independently
/// and the pieces intersecting that chunk's writable box are placed. Biome
/// data is never written; only blocks, block entities, and entities are.
///
/// Returns whether any piece was placed.
pub(crate) fn place_structure_start<L: TemplateLevelAccess>(
    level: &mut L,
    registry: &Registry,
    start: &mut StructureStart,
    random: &mut WorldgenRandom,
    biome_zoom_seed: i64,
) -> bool {
    let Some(bounding_box) = start.bounding_box else {
        return false;
    };
    let Some(reference_pos) = start.placement_reference_pos() else {
        return false;
    };

    let chunk_min = ChunkPos::new(bounding_box.min_x() >> 4, bounding_box.min_z() >> 4);
    let chunk_max = ChunkPos::new(bounding_box.max_x() >> 4, bounding_box.max_z() >> 4);

    let mut placed_any = false;
    for chunk_x in chunk_min.0.x..=chunk_max.0.x {
        for chunk_z in chunk_min.0.y..=chunk_max.0.y {
            let clip = chunk_clip(level, chunk_x, chunk_z);
            for piece in &mut start.pieces {
                if !piece.bounding_box.intersects(clip) {
                    continue;
                }
                placed_any |= StructurePiecePlacer::place_piece(
                    level,
                    registry,
                    piece,
                    reference_pos,
                    clip,
                    random,
                    biome_zoom_seed,
                );
            }

            if let Some(structure) = registry.structures.by_key(&start.structure) {
                StructurePiecePlacer::after_place_structure(
                    level,
                    structure,
                    &mut start.pieces,
                    clip,
                );
            }
        }
    }

    start.bounding_box = StructureStart::compute_bounding_box(&start.pieces, start.bb_inflate);
    placed_any
}

/// Vanilla `ChunkGenerator.getWritableArea`: the chunk column clipped to the
/// level's build height.
///
/// `LevelHeightAccessor.getMaxY()` is inclusive (`min_y + height - 1`), so the
/// writable Y range is `min_y + 1 ..= max_y_exclusive - 1`. This matches
/// `FeatureDecorationRunner::chunk_writable_box`, which is the worldgen-side
/// copy of the same vanilla helper.
fn chunk_clip<L: TemplateLevelAccess>(level: &L, chunk_x: i32, chunk_z: i32) -> BoundingBox {
    let min_x = chunk_x * 16;
    let min_z = chunk_z * 16;
    BoundingBox::from_corners(
        BlockPos::new(min_x, level.min_y() + 1, min_z),
        BlockPos::new(min_x + 15, level.max_y_exclusive() - 1, min_z + 15),
    )
}
