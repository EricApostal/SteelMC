//! Biome source abstraction for dimension-agnostic biome generation.
//!
//! Mirrors vanilla's `BiomeSource` hierarchy:
//! - `MultiNoiseBiomeSource` — Overworld and Nether (climate parameter matching via `RTree`)
//! - `TheEndBiomeSource` — The End (spatial + erosion threshold)
//! - `FixedBiomeSource` — a single biome everywhere (superflat, single-biome worlds)
//! - `CheckerboardColumnBiomeSource` — cycles a biome list over a coarse grid
//!
//! Each dimension creates a different `BiomeSourceKind` variant. The chunk generator
//! calls `chunk_sampler()` per chunk to get a `ChunkBiomeSampler` that holds per-chunk
//! caches (column cache, R-tree warm-start index).
//!
//! ## R-tree cache strategy
//!
//! Vanilla uses `ThreadLocal<Leaf>` which persists the warm-start index across chunks,
//! making tie-breaking at equidistant biome boundaries depend on chunk generation order.
//! We use a per-sampler cache instead: reset per chunk, deterministic regardless of
//! generation order, and better L1 locality since the cache lives on the sampler struct
//! alongside the column cache. The only cost is one cold start per chunk (1/1536 lookups).

use rustc_hash::FxHashSet;
use steel_registry::biome::BiomeRef;
use steel_registry::vanilla_biomes;
use steel_utils::random::Random as _;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::{BlockPos, Identifier};
use steel_worldgen::density_functions::nether::NetherColumnCache;
use steel_worldgen::density_functions::overworld::OverworldColumnCache;
use steel_worldgen::multi_noise::{
    NETHER_BIOME_PARAMETERS, OVERWORLD_BIOME_PARAMETERS, get_nether_biome_cached,
    get_overworld_biome_cached,
};

use super::{NetherClimateSampler, OverworldClimateSampler};
use steel_worldgen::noise::EndIslands;

/// Dimension-specific biome source.
///
/// Each variant holds shared state (noise generators, parameter lists) and
/// creates per-chunk samplers via [`chunk_sampler`](BiomeSourceKind::chunk_sampler).
#[non_exhaustive]
pub enum BiomeSourceKind {
    /// Overworld biome source (multi-noise climate matching).
    Overworld(OverworldBiomeSource),
    /// Nether biome source (multi-noise temperature/vegetation matching).
    Nether(NetherBiomeSource),
    /// End biome source (spatial distance + erosion threshold).
    ///
    /// Boxed because `EndIslands` is ~2KB (simplex noise permutation table),
    /// while the other variants are pointer-sized.
    End(Box<EndBiomeSource>),
    /// Single-biome source (`FixedBiomeSource`), used by superflat and single-biome worlds.
    Fixed(FixedBiomeSource),
    /// Coarse-grid biome cycling source (`CheckerboardColumnBiomeSource`).
    Checkerboard(CheckerboardColumnBiomeSource),
}

impl BiomeSourceKind {
    /// Create an overworld biome source with the given world seed.
    #[must_use]
    pub fn overworld(seed: u64) -> Self {
        Self::Overworld(OverworldBiomeSource::new(seed))
    }

    /// Create a nether biome source with the given world seed.
    #[must_use]
    pub fn nether(seed: u64) -> Self {
        Self::Nether(NetherBiomeSource::new(seed))
    }

    /// Create an end biome source with the given world seed.
    #[must_use]
    pub fn end(seed: u64) -> Self {
        Self::End(Box::new(EndBiomeSource::new(seed)))
    }

    /// Create a fixed biome source returning `biome` everywhere.
    ///
    /// Mirrors vanilla `FixedBiomeSource`.
    #[must_use]
    pub const fn fixed(biome: BiomeRef) -> Self {
        Self::Fixed(FixedBiomeSource::new(biome))
    }

    /// Create a checkerboard biome source cycling `biomes` with vanilla's default scale of 2.
    ///
    /// Mirrors vanilla `CheckerboardColumnBiomeSource` with the default `scale`.
    ///
    /// # Panics
    /// Panics if `biomes` is empty, which vanilla's codec also rejects (a holder set
    /// cannot be empty).
    #[must_use]
    pub fn checkerboard(biomes: Vec<BiomeRef>) -> Self {
        Self::Checkerboard(CheckerboardColumnBiomeSource::new(biomes, 2))
    }

    /// Every biome this source can produce, preserving the source's parameter-list order.
    ///
    /// Feature sorting depends on biome iteration order because vanilla assigns global
    /// feature order while walking all possible biomes. The set-returning
    /// [`possible_biomes`](Self::possible_biomes) is kept for callers that only need
    /// membership tests.
    #[must_use]
    pub fn possible_biome_refs(&self) -> Vec<BiomeRef> {
        let biomes: Vec<BiomeRef> = match self {
            Self::Overworld(_) => OVERWORLD_BIOME_PARAMETERS
                .values()
                .iter()
                .map(|(_, biome)| *biome)
                .collect(),
            Self::Nether(_) => NETHER_BIOME_PARAMETERS
                .values()
                .iter()
                .map(|(_, biome)| *biome)
                .collect(),
            Self::End(_) => vec![
                &vanilla_biomes::THE_END,
                &vanilla_biomes::END_HIGHLANDS,
                &vanilla_biomes::END_MIDLANDS,
                &vanilla_biomes::SMALL_END_ISLANDS,
                &vanilla_biomes::END_BARRENS,
            ],
            Self::Fixed(source) => vec![source.biome()],
            Self::Checkerboard(source) => source.biomes().to_vec(),
        };

        distinct_biome_refs(biomes)
    }

    /// Every biome this source can produce. Used to filter structure sets whose
    /// resolved `allowed_biomes` are from a different dimension.
    #[must_use]
    pub fn possible_biomes(&self) -> FxHashSet<Identifier> {
        self.possible_biome_refs()
            .into_iter()
            .map(|biome| biome.key.clone())
            .collect()
    }

    /// Create a per-chunk biome sampler.
    ///
    /// The returned sampler holds per-chunk caches and should be dropped after
    /// the chunk's biomes are fully populated.
    #[must_use]
    pub fn chunk_sampler(&self) -> ChunkBiomeSampler<'_> {
        match self {
            Self::Overworld(source) => source.chunk_sampler(),
            Self::Nether(source) => source.chunk_sampler(),
            Self::End(source) => source.chunk_sampler(),
            Self::Fixed(source) => source.chunk_sampler(),
            Self::Checkerboard(source) => source.chunk_sampler(),
        }
    }

    /// Vanilla's `BiomeSource.findBiomeHorizontal(findClosest=false, skipSteps=1)`.
    /// Reservoir-samples at y=0. Returns `Some((block_x, block_z))` if found.
    #[must_use]
    pub fn find_biome_horizontal(
        &self,
        origin_x: i32,
        origin_z: i32,
        search_radius: i32,
        allowed: &dyn Fn(BiomeRef) -> bool,
        rng: &mut LegacyRandom,
    ) -> Option<(i32, i32)> {
        // Vanilla `FixedBiomeSource.findBiomeHorizontal` overrides the generic search:
        // it returns a uniformly random block inside the search square when the biome
        // is allowed, without sampling the source at all.
        if let Self::Fixed(source) = self {
            if !allowed(source.biome()) {
                return None;
            }
            let span = search_radius.saturating_mul(2).saturating_add(1);
            return Some((
                origin_x - search_radius + rng.next_i32_bounded(span),
                origin_z - search_radius + rng.next_i32_bounded(span),
            ));
        }

        let mut sampler = self.chunk_sampler();
        // QuartPos.fromBlock; origin_y = 0 so noise_y = 0.
        let noise_center_x = origin_x >> 2;
        let noise_center_z = origin_z >> 2;
        let noise_radius = search_radius >> 2;

        let mut result: Option<(i32, i32)> = None;
        let mut found = 0;
        for z in -noise_radius..=noise_radius {
            for x in -noise_radius..=noise_radius {
                let nx = noise_center_x + x;
                let nz = noise_center_z + z;
                if allowed(sampler.sample(nx, 0, nz)) {
                    // Reservoir: replace with probability 1/(found+1).
                    if result.is_none() || rng.next_i32_bounded(found + 1) == 0 {
                        result = Some((nx << 2, nz << 2));
                    }
                    found += 1;
                }
            }
        }
        result
    }

    /// Returns vanilla's climate-based initial spawn search origin for dimensions that define one.
    #[must_use]
    pub fn initial_spawn_search_origin(&self) -> BlockPos {
        match self {
            Self::Overworld(source) => source.climate_sampler().find_spawn_position(),
            Self::Nether(_) | Self::End(_) | Self::Fixed(_) | Self::Checkerboard(_) => {
                BlockPos::new(0, 0, 0)
            }
        }
    }
}

fn distinct_biome_refs(biomes: Vec<BiomeRef>) -> Vec<BiomeRef> {
    let mut seen = FxHashSet::default();
    let mut distinct = Vec::with_capacity(biomes.len());

    for biome in biomes {
        if seen.insert(&biome.key) {
            distinct.push(biome);
        }
    }

    distinct
}

/// Per-chunk biome sampler with internal caches.
///
/// Created by [`BiomeSourceKind::chunk_sampler`] for each chunk. Holds per-chunk
/// caches: column-level density function values and an R-tree warm-start index
/// that resets each chunk for deterministic biome selection.
///
/// Uses enum dispatch instead of `dyn` to avoid vtable overhead on the hot
/// per-quart sampling path (1536 calls per overworld chunk).
pub enum ChunkBiomeSampler<'a> {
    /// Overworld sampler (climate → R-tree lookup).
    Overworld(Box<OverworldChunkBiomeSampler<'a>>),
    /// Nether sampler (climate → R-tree lookup).
    Nether(Box<NetherChunkBiomeSampler<'a>>),
    /// End sampler (spatial distance thresholds).
    End(Box<EndChunkBiomeSampler<'a>>),
    /// Fixed sampler (single biome).
    Fixed(FixedChunkBiomeSampler),
    /// Checkerboard sampler (coarse-grid biome cycling).
    Checkerboard(CheckerboardChunkBiomeSampler<'a>),
}

impl ChunkBiomeSampler<'_> {
    /// Get the biome at the given quart position.
    #[inline]
    pub fn sample(&mut self, quart_x: i32, quart_y: i32, quart_z: i32) -> BiomeRef {
        match self {
            Self::Overworld(s) => s.sample(quart_x, quart_y, quart_z),
            Self::Nether(s) => s.sample(quart_x, quart_y, quart_z),
            Self::End(s) => s.sample(quart_x, quart_y, quart_z),
            Self::Fixed(s) => s.sample(),
            Self::Checkerboard(s) => s.sample(quart_x, quart_z),
        }
    }

    /// Pre-populate the per-chunk flat-noise grid so a full-chunk biome fill
    /// reuses vanilla's `NoiseChunk.FlatCache` (O(1) column lookups) instead of
    /// recomputing flat (xz-only) climate noise for every quart cell.
    ///
    /// Call once per chunk, before sampling, passing the chunk's minimum block
    /// coordinates. No-op for the End, fixed, and checkerboard sources, which select
    /// biomes without column caches. Samplers used for sparse, scattered lookups (e.g.
    /// `find_biome_horizontal`) intentionally skip this and keep lazy caching.
    pub fn init_grid(&mut self, chunk_block_x: i32, chunk_block_z: i32) {
        match self {
            Self::Overworld(s) => s.init_grid(chunk_block_x, chunk_block_z),
            Self::Nether(s) => s.init_grid(chunk_block_x, chunk_block_z),
            Self::End(_) | Self::Fixed(_) | Self::Checkerboard(_) => {}
        }
    }
}

/// Multi-noise biome source for the overworld.
///
/// Uses compiled overworld density functions to sample climate parameters, then
/// looks up the biome in the overworld parameter list (`RTree`).
///
/// Equivalent to vanilla's `MultiNoiseBiomeSource` with the overworld preset.
pub struct OverworldBiomeSource {
    seed: u64,
    climate_sampler: OverworldClimateSampler,
}

impl OverworldBiomeSource {
    /// Create a new overworld biome source with the given world seed.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            climate_sampler: OverworldClimateSampler::new(seed),
        }
    }

    /// World seed used to initialize this biome source.
    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// Access the underlying climate sampler (for tests, spawn point search, etc.).
    #[must_use]
    pub const fn climate_sampler(&self) -> &OverworldClimateSampler {
        &self.climate_sampler
    }

    fn chunk_sampler(&self) -> ChunkBiomeSampler<'_> {
        ChunkBiomeSampler::Overworld(Box::new(OverworldChunkBiomeSampler {
            source: self,
            column_cache: OverworldColumnCache::new(),
            biome_cache: None,
        }))
    }
}

pub struct OverworldChunkBiomeSampler<'a> {
    source: &'a OverworldBiomeSource,
    column_cache: OverworldColumnCache,
    biome_cache: Option<usize>,
}

impl OverworldChunkBiomeSampler<'_> {
    fn sample(&mut self, quart_x: i32, quart_y: i32, quart_z: i32) -> BiomeRef {
        let target =
            self.source
                .climate_sampler
                .sample(quart_x, quart_y, quart_z, &mut self.column_cache);
        get_overworld_biome_cached(&target, &mut self.biome_cache)
    }

    fn init_grid(&mut self, chunk_block_x: i32, chunk_block_z: i32) {
        self.source.climate_sampler.init_column_grid(
            &mut self.column_cache,
            chunk_block_x,
            chunk_block_z,
        );
    }
}

// ── Nether ──────────────────────────────────────────────────────────────────

/// Multi-noise biome source for the nether.
///
/// Uses compiled nether density functions to sample temperature and vegetation,
/// then looks up the biome in the nether parameter list (`RTree`).
///
/// Equivalent to vanilla's `MultiNoiseBiomeSource` with the nether preset.
pub struct NetherBiomeSource {
    seed: u64,
    climate_sampler: NetherClimateSampler,
}

impl NetherBiomeSource {
    /// Create a new nether biome source with the given world seed.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            climate_sampler: NetherClimateSampler::new(seed),
        }
    }

    /// World seed used to initialize this biome source.
    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    fn chunk_sampler(&self) -> ChunkBiomeSampler<'_> {
        ChunkBiomeSampler::Nether(Box::new(NetherChunkBiomeSampler {
            source: self,
            column_cache: NetherColumnCache::new(),
            biome_cache: None,
        }))
    }
}

pub struct NetherChunkBiomeSampler<'a> {
    source: &'a NetherBiomeSource,
    column_cache: NetherColumnCache,
    biome_cache: Option<usize>,
}

impl NetherChunkBiomeSampler<'_> {
    fn sample(&mut self, quart_x: i32, quart_y: i32, quart_z: i32) -> BiomeRef {
        let target =
            self.source
                .climate_sampler
                .sample(quart_x, quart_y, quart_z, &mut self.column_cache);
        get_nether_biome_cached(&target, &mut self.biome_cache)
    }

    fn init_grid(&mut self, chunk_block_x: i32, chunk_block_z: i32) {
        self.source.climate_sampler.init_column_grid(
            &mut self.column_cache,
            chunk_block_x,
            chunk_block_z,
        );
    }
}

// ── The End ───────────────────────────────────────────────────────────────────

/// Biome source for The End dimension.
///
/// Uses spatial distance from origin and the `EndIslands` density function for
/// biome selection. Does NOT use climate parameters — biome choice is based on:
///
/// 1. **Central island** (`chunkX² + chunkZ² ≤ 4096`): always `the_end`
/// 2. **Outer islands** (erosion from `EndIslands` at transformed coordinates):
///    - `> 0.25` → `end_highlands`
///    - `≥ -0.0625` → `end_midlands`
///    - `< -0.21875` → `small_end_islands`
///    - otherwise → `end_barrens`
pub struct EndBiomeSource {
    seed: u64,
    end_islands: EndIslands,
}

impl EndBiomeSource {
    /// Create a new End biome source with the given world seed.
    ///
    /// The `EndIslands` density function is initialized with the world seed,
    /// matching vanilla's `RandomState.NoiseWiringHelper.wrapNew()` which replaces
    /// the default seed-0 instance with `EndIslandDensityFunction(worldSeed)`.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            end_islands: EndIslands::new(seed),
        }
    }

    /// World seed used to initialize this biome source.
    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    fn chunk_sampler(&self) -> ChunkBiomeSampler<'_> {
        ChunkBiomeSampler::End(Box::new(EndChunkBiomeSampler {
            source: self,
            cached_erosion: None,
        }))
    }
}

pub struct EndChunkBiomeSampler<'a> {
    source: &'a EndBiomeSource,
    /// Cached erosion value keyed by (`chunk_x`, `chunk_z`).
    ///
    /// All quart positions within a chunk produce the same chunk coordinates,
    /// and `EndIslands::sample` ignores `block_y`, so the erosion is constant
    /// per chunk. This avoids redundant 25×25 simplex neighborhood scans.
    cached_erosion: Option<(i32, i32, f64)>,
}

impl EndChunkBiomeSampler<'_> {
    fn get_erosion(&mut self, chunk_x: i32, chunk_z: i32) -> f64 {
        if let Some((cx, cz, erosion)) = self.cached_erosion
            && cx == chunk_x
            && cz == chunk_z
        {
            return erosion;
        }
        let weird_block_x = f64::from((chunk_x * 2 + 1) * 8);
        let weird_block_z = f64::from((chunk_z * 2 + 1) * 8);
        let erosion = self
            .source
            .end_islands
            .sample(weird_block_x, 0.0, weird_block_z);
        self.cached_erosion = Some((chunk_x, chunk_z, erosion));
        erosion
    }

    fn sample(&mut self, quart_x: i32, _quart_y: i32, quart_z: i32) -> BiomeRef {
        let block_x = quart_x << 2;
        let block_z = quart_z << 2;
        let chunk_x = block_x >> 4;
        let chunk_z = block_z >> 4;

        // Central island: if within 64 chunks of origin
        if i64::from(chunk_x) * i64::from(chunk_x) + i64::from(chunk_z) * i64::from(chunk_z) <= 4096
        {
            return &vanilla_biomes::THE_END;
        }

        let erosion = self.get_erosion(chunk_x, chunk_z);

        if erosion > 0.25 {
            &vanilla_biomes::END_HIGHLANDS
        } else if erosion >= -0.0625 {
            &vanilla_biomes::END_MIDLANDS
        } else if erosion < -0.21875 {
            &vanilla_biomes::SMALL_END_ISLANDS
        } else {
            &vanilla_biomes::END_BARRENS
        }
    }
}

// ── Fixed ────────────────────────────────────────────────────────────────────

/// Single-biome source mirroring vanilla's `FixedBiomeSource`.
///
/// Every quart position returns the same biome. Used by superflat generators
/// and single-biome world presets.
pub struct FixedBiomeSource {
    biome: BiomeRef,
}

impl FixedBiomeSource {
    /// Create a fixed biome source returning `biome` at every position.
    #[must_use]
    pub const fn new(biome: BiomeRef) -> Self {
        Self { biome }
    }

    /// The single biome this source returns.
    #[must_use]
    pub const fn biome(&self) -> BiomeRef {
        self.biome
    }

    const fn chunk_sampler(&self) -> ChunkBiomeSampler<'_> {
        ChunkBiomeSampler::Fixed(FixedChunkBiomeSampler { biome: self.biome })
    }
}

/// Per-chunk sampler for [`FixedBiomeSource`]. Stateless: no caches needed.
pub struct FixedChunkBiomeSampler {
    biome: BiomeRef,
}

impl FixedChunkBiomeSampler {
    const fn sample(&mut self) -> BiomeRef {
        self.biome
    }
}

// ── Checkerboard ─────────────────────────────────────────────────────────────

/// Coarse-grid biome cycling source mirroring vanilla's
/// `CheckerboardColumnBiomeSource`.
///
/// Selection is `biomes[floor_mod((quart_x >> bit_shift) + (quart_z >> bit_shift), len)]`
/// with `bit_shift = scale + 2`; vanilla's default `scale` of 2 gives 16-quart
/// (64-block) cells.
pub struct CheckerboardColumnBiomeSource {
    biomes: Vec<BiomeRef>,
    bit_shift: u32,
}

impl CheckerboardColumnBiomeSource {
    /// Create a checkerboard source over `biomes` with the given vanilla `scale`
    /// (cell size in quarts is `1 << (scale + 2)`).
    ///
    /// # Panics
    /// Panics if `biomes` is empty or `scale` exceeds 62, matching the range of
    /// vanilla's `scale` codec (`Codec.intRange(0, 62)`).
    #[must_use]
    pub fn new(biomes: Vec<BiomeRef>, scale: u32) -> Self {
        assert!(
            !biomes.is_empty(),
            "checkerboard biome source needs a biome"
        );
        assert!(
            scale <= 62,
            "checkerboard scale {scale} exceeds vanilla maximum 62"
        );
        Self {
            biomes,
            bit_shift: scale + 2,
        }
    }

    /// The biome list this source cycles, in order.
    #[must_use]
    pub fn biomes(&self) -> &[BiomeRef] {
        &self.biomes
    }

    /// Vanilla `scale` (`bit_shift - 2`).
    #[must_use]
    pub const fn scale(&self) -> u32 {
        self.bit_shift - 2
    }

    fn sample_biome(&self, quart_x: i32, quart_z: i32) -> BiomeRef {
        // Vanilla `>>` on ints masks the shift distance mod 32 (JLS 15.19);
        // `wrapping_shr` reproduces that for scales whose `bit_shift` exceeds 31.
        let shift = self.bit_shift & 31;
        let index = quart_x
            .wrapping_shr(shift)
            .wrapping_add(quart_z.wrapping_shr(shift))
            .rem_euclid(self.biomes.len() as i32) as usize;
        self.biomes[index]
    }

    const fn chunk_sampler(&self) -> ChunkBiomeSampler<'_> {
        ChunkBiomeSampler::Checkerboard(CheckerboardChunkBiomeSampler { source: self })
    }
}

/// Per-chunk sampler for [`CheckerboardColumnBiomeSource`]. Stateless: selection
/// is a pure function of quart coordinates.
pub struct CheckerboardChunkBiomeSampler<'a> {
    source: &'a CheckerboardColumnBiomeSource,
}

impl CheckerboardChunkBiomeSampler<'_> {
    fn sample(&mut self, quart_x: i32, quart_z: i32) -> BiomeRef {
        self.source.sample_biome(quart_x, quart_z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_possible_biomes_follow_vanilla_order() {
        let source = BiomeSourceKind::end(0);
        let keys = source
            .possible_biome_refs()
            .into_iter()
            .map(|biome| &biome.key)
            .collect::<Vec<_>>();

        assert_eq!(
            keys,
            vec![
                &vanilla_biomes::THE_END.key,
                &vanilla_biomes::END_HIGHLANDS.key,
                &vanilla_biomes::END_MIDLANDS.key,
                &vanilla_biomes::SMALL_END_ISLANDS.key,
                &vanilla_biomes::END_BARRENS.key,
            ]
        );
    }

    #[test]
    fn fixed_returns_single_biome_everywhere() {
        let source = BiomeSourceKind::fixed(&vanilla_biomes::DESERT);
        assert_eq!(source.possible_biome_refs(), vec![&*vanilla_biomes::DESERT]);
        let mut sampler = source.chunk_sampler();
        assert_eq!(sampler.sample(0, 0, 0), &*vanilla_biomes::DESERT);
        assert_eq!(sampler.sample(-1234, 64, 5678), &*vanilla_biomes::DESERT);
    }

    #[test]
    fn checkerboard_matches_java_indexing() {
        // Two biomes, default scale 2 → bit_shift 4. Cell (qx>>4)+(qz>>4), floormod 2.
        let source =
            BiomeSourceKind::checkerboard(vec![&vanilla_biomes::PLAINS, &vanilla_biomes::DESERT]);
        let mut sampler = source.chunk_sampler();
        // (0,0): 0+0=0 → index 0.
        assert_eq!(sampler.sample(0, 0, 0), &*vanilla_biomes::PLAINS);
        // (16,0): 1+0=1 → index 1.
        assert_eq!(sampler.sample(16, 0, 0), &*vanilla_biomes::DESERT);
        // (16,16): 1+1=2 → floormod 0.
        assert_eq!(sampler.sample(16, 0, 16), &*vanilla_biomes::PLAINS);
        // (-1,0): -1>>4 = -1 in Java (arithmetic shift) → -1 floormod 2 = 1.
        assert_eq!(sampler.sample(-1, 0, 0), &*vanilla_biomes::DESERT);
    }
}
