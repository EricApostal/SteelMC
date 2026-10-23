use std::sync::Arc;

use crate::chunk::{
    chunk_generation_task::StaticCache2D, chunk_holder::ChunkHolder, chunk_pyramid::ChunkStep,
};
use crate::world::spawn::NaturalSpawner;
use crate::worldgen::generator::ChunkGenerator as _;
use crate::worldgen::generator::context::WorldGenContext;
use crate::worldgen::region::WorldGenRegion;

/// Vanilla `ChunkStatusTasks.generateSpawn`.
///
/// Vanilla calls `context.generator().spawnOriginalMobs(new WorldGenRegion(...))`;
/// `NaturalSpawner::spawn_original_mobs` is that same generator-shaped entry
/// point (it no-ops for flat/empty generators like vanilla's overrides and
/// applies the `SPAWN_MOBS` gate).
pub(crate) fn generate(
    context: Arc<WorldGenContext>,
    step: &ChunkStep,
    cache: &Arc<StaticCache2D<Arc<ChunkHolder>>>,
    holder: Arc<ChunkHolder>,
) {
    let center = holder.get_pos();
    let world = context.world();
    let world_seed = world.seed();
    let region_random = context
        .generator
        .create_worldgen_region_random(world_seed, center);
    let region = WorldGenRegion::new(context.as_ref(), step, cache, center, region_random);

    // Vanilla builds a dedicated `WorldgenRandom` for this pass
    // (`NoiseBasedChunkGenerator.spawnOriginalMobs`), so the region's own random
    // stream is not consumed by mob selection.
    let mut random = context
        .generator
        .create_worldgen_region_random(world_seed, center);

    NaturalSpawner::spawn_original_mobs(&world, context.generator.as_ref(), &region, &mut random);
}
