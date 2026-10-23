use glam::IVec3;
use steel_registry::biome::BiomeRef;
use steel_registry::{REGISTRY, RegistryEntry, RegistryExt, vanilla_biomes, vanilla_blocks};
use steel_utils::random::RandomSource;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, ChunkPos};

use crate::chunk::Chunk;
use crate::worldgen::generator::{
    ChunkGenerator, GenerationChunk, TerrainPhase, xoroshiro_worldgen_region_random,
};
use crate::worldgen::region::WorldGenRegion;
use steel_worldgen::noise::Beardifier;

/// Y level of the barrier sheet, matching vanilla `DebugLevelSource.BARRIER_HEIGHT`.
const BARRIER_HEIGHT: i32 = 60;
/// Y level of the block-state grid, matching vanilla `DebugLevelSource.HEIGHT`.
const BLOCK_GRID_HEIGHT: i32 = 70;
/// Vanilla `DebugLevelSource.BLOCK_MARGIN`: each grid cell spans two blocks per axis.
const BLOCK_MARGIN: i32 = 2;

/// A chunk generator that generates vanilla's debug world.
///
/// Mirrors `DebugLevelSource`: the terrain is empty, every chunk carries the
/// `minecraft:plains` biome, and biome decoration writes a barrier sheet at y=60 plus
/// one block state per grid cell at y=70. The grid walks every registered block state
/// in registry order (`ALL_BLOCKS`), so it doubles as a block-state visual reference.
pub struct DebugChunkGenerator {
    /// Every block state, in vanilla `ALL_BLOCKS` order.
    ///
    /// Steel assigns global state ids in registry iteration order, so
    /// `ALL_BLOCKS[i] == BlockStateId(i)` for `i < state_count`; only the length is
    /// needed here.
    all_blocks_len: usize,
    /// Grid width, vanilla `GRID_WIDTH = ceil(sqrt(ALL_BLOCKS.size()))`.
    grid_width: i32,
    /// Grid height, vanilla `GRID_HEIGHT = ceil(ALL_BLOCKS.size() / GRID_WIDTH)`.
    grid_height: i32,
}

impl DebugChunkGenerator {
    pub(crate) const MIN_Y: i32 = 0;
    pub(crate) const HEIGHT: i32 = 384;
    pub(crate) const SEA_LEVEL: i32 = 63;

    /// Creates the debug generator from the current block registry contents.
    #[must_use]
    pub fn new() -> Self {
        let all_blocks_len = usize::from(REGISTRY.blocks.next_state_id);
        let grid_width = (all_blocks_len as f64).sqrt().ceil() as i32;
        let grid_height = (all_blocks_len as f64 / f64::from(grid_width)).ceil() as i32;
        Self {
            all_blocks_len,
            grid_width,
            grid_height,
        }
    }

    /// Vanilla `DebugLevelSource.getBlockStateFor`: maps world coordinates to the block
    /// state shown at that grid cell, or air outside the grid.
    #[must_use]
    pub fn block_state_for(&self, world_x: i32, world_z: i32) -> BlockStateId {
        if world_x > 0 && world_z > 0 && world_x % 2 != 0 && world_z % 2 != 0 {
            let grid_x = world_x / BLOCK_MARGIN;
            let grid_z = world_z / BLOCK_MARGIN;
            if grid_x <= self.grid_width && grid_z <= self.grid_height {
                let index = (i64::from(grid_x) * i64::from(self.grid_width) + i64::from(grid_z))
                    .abs() as usize;
                if index < self.all_blocks_len {
                    return BlockStateId(index as u16);
                }
            }
        }
        REGISTRY.blocks.get_default_state_id(&vanilla_blocks::AIR)
    }
}

impl Default for DebugChunkGenerator {
    fn default() -> Self {
        Self::new()
    }
}

impl ChunkGenerator for DebugChunkGenerator {
    fn min_y(&self) -> i32 {
        Self::MIN_Y
    }

    fn gen_depth(&self) -> i32 {
        Self::HEIGHT
    }

    fn noise_biome(&self, _quart_x: i32, _quart_y: i32, _quart_z: i32) -> BiomeRef {
        &vanilla_biomes::PLAINS
    }

    fn create_structures(&self, _chunk: &Chunk) {}

    fn create_biomes(&self, chunk: &Chunk) {
        let biome_id = vanilla_biomes::PLAINS.try_id().map_or(0, |id| id as u16);
        let section_count = chunk.sections().sections.len();

        for section_index in 0..section_count {
            let section = &chunk.sections().sections[section_index];
            let mut section_guard = section.write();

            for local_quart_x in 0..4usize {
                for local_quart_y in 0..4usize {
                    for local_quart_z in 0..4usize {
                        section_guard.biomes.set(
                            local_quart_x,
                            local_quart_y,
                            local_quart_z,
                            biome_id,
                        );
                    }
                }
            }
            drop(section_guard);
        }

        chunk.mark_dirty();
    }

    fn fill_from_noise(
        &self,
        _chunk: GenerationChunk<'_, TerrainPhase>,
        _beardifier: Option<&Beardifier>,
    ) {
    }

    fn build_surface(
        &self,
        _chunk: GenerationChunk<'_, TerrainPhase>,
        _neighbor_biomes: &dyn Fn(IVec3) -> u16,
    ) {
    }

    fn apply_carvers(&self, _chunk: GenerationChunk<'_, TerrainPhase>) {}

    fn create_worldgen_region_random(&self, world_seed: i64, center: ChunkPos) -> RandomSource {
        xoroshiro_worldgen_region_random(world_seed, center)
    }

    /// Vanilla `DebugLevelSource.applyBiomeDecoration`: writes the barrier sheet and the
    /// block-state grid and runs no feature decoration.
    fn apply_biome_decorations(&self, region: &mut WorldGenRegion<'_>) {
        let center = region.center();
        let barrier = REGISTRY.blocks.get_default_state_id(&vanilla_blocks::BARRIER);

        for local_x in 0..16 {
            for local_z in 0..16 {
                let world_x = center.0.x * 16 + local_x;
                let world_z = center.0.y * 16 + local_z;
                let _ = region.set_block_state(
                    BlockPos::new(world_x, BARRIER_HEIGHT, world_z),
                    barrier,
                    UpdateFlags::UPDATE_CLIENTS,
                );
                let _ = region.set_block_state(
                    BlockPos::new(world_x, BLOCK_GRID_HEIGHT, world_z),
                    self.block_state_for(world_x, world_z),
                    UpdateFlags::UPDATE_CLIENTS,
                );
            }
        }
    }
}
