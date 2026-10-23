//! Climate sampler for overworld world generation.
//!
//! Uses the compiled overworld density functions from steel-registry for fast
//! evaluation, bypassing the runtime tree interpreter entirely.
//!
//! This is overworld-specific because it uses `OverworldNoises` and the overworld
//! noise router (`router_temperature`, `router_vegetation`, etc.). Other dimensions
//! need their own climate samplers with their own transpiled density functions.

use steel_utils::BlockPos;
use steel_utils::climate::{TargetPoint, quantize_coord};
use steel_utils::random::{Random, xoroshiro::Xoroshiro};
use steel_worldgen::density_functions::overworld::{self, OverworldColumnCache, OverworldNoises};
use steel_worldgen::noise_parameters::get_noise_parameters;

/// Climate sampler for the overworld using compiled density functions.
///
/// Evaluates the overworld noise router (temperature, vegetation, continentalness,
/// erosion, depth, ridges) to produce `TargetPoint` values for biome lookup.
///
pub struct OverworldClimateSampler {
    /// All noise generators needed by the overworld density functions.
    /// Boxed because `OverworldNoises` is ~5600 bytes (35 `NormalNoise` fields).
    noises: Box<OverworldNoises>,
}

impl OverworldClimateSampler {
    /// Create a new overworld climate sampler with the given seed.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        let mut rng = Xoroshiro::from_seed(seed);
        let splitter = rng.next_positional();
        let noise_params = get_noise_parameters();
        let noises = OverworldNoises::create(seed, &splitter, &noise_params);

        Self {
            noises: Box::new(noises),
        }
    }

    /// Sample climate at a quart position.
    ///
    /// The `cache` holds column-level (xz-only) precomputed values.
    /// It should persist across calls for the same chunk to avoid redundant
    /// noise evaluations when only `y` changes.
    #[must_use]
    pub fn sample(
        &self,
        quart_x: i32,
        quart_y: i32,
        quart_z: i32,
        cache: &mut OverworldColumnCache,
    ) -> TargetPoint {
        let block_x = quart_x << 2;
        let block_y = quart_y << 2;
        let block_z = quart_z << 2;

        // Ensure column cache is populated for this (x, z)
        cache.ensure(block_x, block_z, &self.noises);

        let block_x = f64::from(block_x);
        let block_y = f64::from(block_y);
        let block_z = f64::from(block_z);

        let temp = overworld::router_temperature(&self.noises, cache, block_x, block_y, block_z);
        let humidity = overworld::router_vegetation(&self.noises, cache, block_x, block_y, block_z);
        let cont =
            overworld::router_continentalness(&self.noises, cache, block_x, block_y, block_z);
        let erosion = overworld::router_erosion(&self.noises, cache, block_x, block_y, block_z);
        let depth = overworld::router_depth(&self.noises, cache, block_x, block_y, block_z);
        let weirdness = overworld::router_ridges(&self.noises, cache, block_x, block_y, block_z);

        TargetPoint::new(
            quantize_coord(f64::from(temp)),
            quantize_coord(f64::from(humidity)),
            quantize_coord(f64::from(cont)),
            quantize_coord(f64::from(erosion)),
            quantize_coord(f64::from(depth)),
            quantize_coord(f64::from(weirdness)),
        )
    }

    /// Pre-populate a column cache's flat-noise grid for a chunk's quart columns.
    ///
    /// The biome stage samples every quart cell in a chunk (1536 for the
    /// overworld) in `section → x → y → z` order. Without a grid, the cache is a
    /// single-entry lazy cache that misses on every cell (the innermost `z` loop
    /// changes column each step), so the expensive flat (xz-only) climate noise
    /// is recomputed per cell. Pre-computing the grid once — exactly as the noise
    /// stage's `fill_from_noise` does — turns those into O(1) lookups. Bit-identical
    /// because the grid evaluates the same functions at the same quart coordinates.
    pub fn init_column_grid(
        &self,
        cache: &mut OverworldColumnCache,
        chunk_block_x: i32,
        chunk_block_z: i32,
    ) {
        cache.init_grid(chunk_block_x, chunk_block_z, &self.noises);
    }

    /// Finds the climate-biased overworld spawn origin.
    ///
    /// This mirrors vanilla's `NoiseSpawnFinder.findSpawnPosition` driven by this
    /// dimension's datapack `spawn_target` list (`overworld::SPAWN_TARGETS`).
    #[must_use]
    pub fn find_spawn_position(&self) -> BlockPos {
        super::spawn_target::find_spawn_position(
            overworld::SPAWN_TARGETS,
            |target, point| {
                super::spawn_target::spawn_target_fitness(
                    target.temperature,
                    target.vegetation,
                    target.continents,
                    target.erosion,
                    target.depth,
                    target.ridges,
                    point,
                )
            },
            |block_x, block_z| {
                let mut cache = OverworldColumnCache::new();
                self.sample(block_x >> 2, 0, block_z >> 2, &mut cache)
            },
        )
    }
}
