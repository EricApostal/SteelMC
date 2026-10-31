//! Climate sampler for overworld world generation.
//!
//! Uses the compiled overworld density functions for fast evaluation, bypassing
//! the runtime tree interpreter entirely.
//!
//! Vanilla builds `MultiNoiseBiomeSource`'s `Climate.Sampler` from the chunk
//! generator's `NoiseGeneratorSettings`, so the same overworld parameter list is
//! sampled with a different climate router per settings entry: `minecraft:overworld`
//! for the normal world and `minecraft:overworld_large_biomes` for large biomes
//! (which widens biomes through `continentalness_large` / `erosion_large`). The
//! sampler is therefore generic over the settings' transpiled density functions.

use steel_utils::BlockPos;
use steel_utils::climate::{TargetPoint, quantize_coord};
use steel_utils::random::{Random, xoroshiro::Xoroshiro};
use steel_worldgen::density::{ColumnCache, DimensionNoises};
use steel_worldgen::density_functions::overworld::OverworldNoises;
use steel_worldgen::noise_parameters::get_noise_parameters;

use super::spawn_target::SpawnTargetEntry;

/// Climate sampler for the overworld-family noise settings using compiled density functions.
///
/// Evaluates the settings' noise router (temperature, vegetation, continentalness,
/// erosion, depth, ridges) to produce `TargetPoint` values for biome lookup.
pub struct OverworldClimateSampler<N: DimensionNoises = OverworldNoises> {
    /// All noise generators needed by this settings entry's density functions.
    /// Boxed because the noise structs are several kilobytes.
    noises: Box<N>,
}

impl<N: DimensionNoises> OverworldClimateSampler<N> {
    /// Create a new climate sampler with the given seed.
    ///
    /// Only valid for settings that use Xoroshiro (`legacy_random_source: false`),
    /// which is every overworld-family entry: `overworld`, `large_biomes`, `amplified`.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        let mut rng = Xoroshiro::from_seed(seed);
        let splitter = rng.next_positional();
        let noise_params = get_noise_parameters();
        let noises = N::create(seed, &splitter, &noise_params);

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
        cache: &mut N::ColumnCache,
    ) -> TargetPoint {
        let block_x = quart_x << 2;
        let block_y = quart_y << 2;
        let block_z = quart_z << 2;

        // Ensure column cache is populated for this (x, z)
        cache.ensure(block_x, block_z, &self.noises);

        let temp = self
            .noises
            .router_temperature(cache, block_x, block_y, block_z);
        let humidity = self
            .noises
            .router_vegetation(cache, block_x, block_y, block_z);
        let cont = self
            .noises
            .router_continentalness(cache, block_x, block_y, block_z);
        let erosion = self.noises.router_erosion(cache, block_x, block_y, block_z);
        let depth = self.noises.router_depth(cache, block_x, block_y, block_z);
        let weirdness = self.noises.router_ridges(cache, block_x, block_y, block_z);

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
        cache: &mut N::ColumnCache,
        chunk_block_x: i32,
        chunk_block_z: i32,
    ) {
        cache.init_grid(chunk_block_x, chunk_block_z, &self.noises);
    }

    /// Finds the climate-biased spawn origin.
    ///
    /// This mirrors vanilla's `NoiseSpawnFinder.findSpawnPosition` driven by the
    /// settings' datapack `spawn_target` list, e.g. `large_biomes::SPAWN_TARGETS`.
    #[must_use]
    pub fn find_spawn_position<T: SpawnTargetEntry>(&self, targets: &'static [T]) -> BlockPos {
        super::spawn_target::find_spawn_position(
            targets,
            SpawnTargetEntry::fitness,
            |block_x, block_z| {
                let mut cache = N::ColumnCache::default();
                self.sample(block_x >> 2, 0, block_z >> 2, &mut cache)
            },
        )
    }
}
