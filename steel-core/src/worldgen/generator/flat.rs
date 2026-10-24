use glam::IVec3;
use rustc_hash::FxHashMap;
use steel_registry::biome::BiomeRef;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::feature::data::{
    BlockStateData, ConfiguredFeatureKind, ConfiguredFeatureRef, FillLayerConfiguration,
    PlacedFeatureData,
};
use steel_registry::template_pool::{TemplateData, TemplatePoolData};
use steel_registry::{REGISTRY, RegistryEntry, vanilla_biomes, vanilla_blocks};
use steel_utils::random::RandomSource;
use steel_utils::{BlockStateId, ChunkPos, Identifier};

use crate::chunk::Chunk;
use crate::worldgen::feature::{FeatureDecorationRunner, StagedFeature};
use crate::worldgen::generator::{
    ChunkGenerator, GenerationChunk, TerrainPhase, xoroshiro_worldgen_region_random,
};
use crate::worldgen::region::WorldGenRegion;
use crate::worldgen::structure::{StructureGenerator, StructureStart, create_structures};
use steel_worldgen::biomes::obfuscate_biome_seed;
use steel_worldgen::noise::Beardifier;
use steel_worldgen::structure::{ColumnBlock, StructureGenerationContext};

/// Vanilla `GenerationStep.Decoration` ordinals used by flat settings.
const STEP_LAKES: usize = 1;
const STEP_UNDERGROUND_STRUCTURES: usize = 3;
const STEP_SURFACE_STRUCTURES: usize = 4;
const STEP_TOP_LAYER_MODIFICATION: usize = 10;

/// Runtime inputs for a flat generator, mirroring vanilla
/// `FlatLevelGeneratorSettings` after `updateLayers`.
pub struct FlatGeneratorSettings {
    /// Block layers from world bottom upwards; `None` for layers vanilla replaces with
    /// a `minecraft:fill_layer` top-layer-modification feature.
    pub layers: Vec<Option<BlockStateId>>,
    /// Biome generated across the flat layers (vanilla `settings.getBiome()`).
    pub biome: BiomeRef,
    /// World seed for structure placement and decoration seeding.
    pub seed: i64,
    /// Sea level for this flat generator's dimension type.
    pub sea_level: i32,
    /// Vanilla `features` flag (`FlatLevelGeneratorSettings.decoration`).
    pub features: bool,
    /// Vanilla `lakes` flag (`FlatLevelGeneratorSettings.addLakes`).
    pub lakes: bool,
    /// Optional structure engine from flat structure overrides.
    pub structure_generator: Option<StructureGenerator>,
}

/// A chunk generator that generates a flat world.
///
/// Mirrors vanilla's `FlatLevelSource`: a `FixedBiomeSource` for the configured biome
/// plus layer-by-layer terrain, with biome decoration driven by
/// `FlatLevelGeneratorSettings.adjustGenerationSettings`.
pub struct FlatChunkGenerator {
    /// Block layers from world bottom upwards.
    ///
    /// `None` marks a layer that is not `MOTION_BLOCKING`-opaque: vanilla removes it
    /// from the terrain layers and places it with a top-layer-modification
    /// `fill_layer` feature instead.
    pub layers: Vec<Option<BlockStateId>>,
    /// The single biome this generator produces.
    biome: BiomeRef,
    /// Cached registry id of [`Self::biome`].
    biome_id: u16,
    /// World seed for structure placement.
    seed: i64,
    /// Sea level for this flat generator's dimension type.
    sea_level: i32,
    /// Optional structure engine from flat structure overrides.
    structure_generator: Option<StructureGenerator>,
    /// Decoration runner built from the adjusted flat feature stages.
    feature_runner: FeatureDecorationRunner,
    /// Obfuscated seed for `BiomeManager` biome zoom fuzzing.
    biome_zoom_seed: i64,
}

impl FlatChunkGenerator {
    pub(crate) const MIN_Y: i32 = 0;
    pub(crate) const HEIGHT: i32 = 384;
    pub(crate) const SEA_LEVEL: i32 = -63;

    /// Creates a new `FlatChunkGenerator`.
    #[must_use]
    pub fn new(bedrock: BlockStateId, dirt: BlockStateId, grass: BlockStateId) -> Self {
        Self::new_layers(vec![bedrock, dirt, dirt, grass])
    }

    /// Creates a new flat generator with explicit block layers from bottom upwards.
    #[must_use]
    pub fn new_layers(layers: Vec<BlockStateId>) -> Self {
        Self::from_settings(FlatGeneratorSettings {
            layers: layers.into_iter().map(Some).collect(),
            biome: &vanilla_biomes::PLAINS,
            seed: 0,
            sea_level: Self::SEA_LEVEL,
            features: false,
            lakes: false,
            structure_generator: None,
        })
    }

    /// Creates a flat generator from vanilla flat settings.
    ///
    /// Applies `FlatLevelGeneratorSettings.adjustGenerationSettings` up front, exactly
    /// like vanilla's `ChunkGenerator.validate()` forces the memoized generation
    /// settings getter before any chunk is generated: non-opaque layers are removed
    /// from the terrain and re-added as `fill_layer` features.
    #[must_use]
    pub fn from_settings(settings: FlatGeneratorSettings) -> Self {
        let FlatGeneratorSettings {
            mut layers,
            biome,
            seed,
            sea_level,
            features,
            lakes,
            structure_generator,
        } = settings;

        let biome_id = biome.try_id().map_or(0, |id| id as u16);
        let void_gen = layers
            .iter()
            .all(|layer| layer.is_some_and(|state| state.get_block() == &vanilla_blocks::AIR));
        let stages = adjusted_feature_stages(biome, &mut layers, features, lakes, void_gen);
        let feature_runner =
            FeatureDecorationRunner::new_with_feature_stages(biome, stages, &REGISTRY);

        Self {
            layers,
            biome,
            biome_id,
            seed,
            sea_level,
            structure_generator,
            feature_runner,
            biome_zoom_seed: obfuscate_biome_seed(seed),
        }
    }

    /// The biome this flat generator produces at every position.
    #[must_use]
    pub const fn biome(&self) -> BiomeRef {
        self.biome
    }
}

/// Vanilla `Heightmap.Types.MOTION_BLOCKING.isOpaque()`: the block is in
/// `minecraft:blocks_motion_in_heightmap`, or the state carries a fluid.
fn motion_blocking_opaque(state: BlockStateId) -> bool {
    state
        .get_block()
        .has_tag(&Identifier::vanilla_static("blocks_motion_in_heightmap"))
        || state.has_fluid()
}

/// Vanilla `FlatLevelGeneratorSettings.adjustGenerationSettings`.
///
/// Builds the decorated biome's feature stages: lava lakes are added when `lakes` is
/// set, the biome's own stages are copied when `features` is set (minus
/// `UNDERGROUND_STRUCTURES`, `SURFACE_STRUCTURES`, and `LAKES` when lakes were added),
/// and every non-`MOTION_BLOCKING`-opaque layer becomes an inline `fill_layer` feature
/// in `TOP_LAYER_MODIFICATION`. Those layers are cleared to `None` so terrain skips
/// them, matching vanilla mutating `layers` inside the (memoized) getter.
fn adjusted_feature_stages(
    biome: BiomeRef,
    layers: &mut [Option<BlockStateId>],
    features: bool,
    lakes: bool,
    void_gen: bool,
) -> Vec<Vec<StagedFeature>> {
    let mut stages: Vec<Vec<StagedFeature>> = (0..=STEP_TOP_LAYER_MODIFICATION)
        .map(|_| Vec::new())
        .collect();

    if lakes {
        for key in ["lake_lava_underground", "lake_lava_surface"] {
            stages[STEP_LAKES].push(StagedFeature::registry(Identifier::vanilla_static(key)));
        }
    }

    let decoration = (!void_gen || biome.key == vanilla_biomes::THE_VOID.key) && features;
    if decoration {
        for (step, stage) in biome.features.iter().enumerate() {
            if step == STEP_UNDERGROUND_STRUCTURES || step == STEP_SURFACE_STRUCTURES {
                continue;
            }
            if lakes && step == STEP_LAKES {
                continue;
            }
            let Some(target) = stages.get_mut(step) else {
                continue;
            };
            target.extend(stage.iter().cloned().map(StagedFeature::registry));
        }
    }

    for (index, layer) in layers.iter_mut().enumerate() {
        let Some(state) = *layer else {
            continue;
        };
        if motion_blocking_opaque(state) {
            continue;
        }
        *layer = None;
        let block = state.get_block();
        stages[STEP_TOP_LAYER_MODIFICATION].push(StagedFeature::Inline(Box::new(
            PlacedFeatureData {
                feature: ConfiguredFeatureRef::Inline(Box::new(ConfiguredFeatureKind::FillLayer(
                    FillLayerConfiguration {
                        height: index as i32,
                        state: BlockStateData {
                            block,
                            properties: &[],
                        },
                    },
                ))),
                placement: Vec::new(),
            },
        )));
    }

    stages
}

struct FlatGenerationContext<'a> {
    seed: i64,
    chunk_x: i32,
    chunk_z: i32,
    chunk_min_x: i32,
    chunk_min_z: i32,
    center_block_x: i32,
    center_block_z: i32,
    sea_level: i32,
    min_y: i32,
    height: i32,
    layers: &'a [Option<BlockStateId>],
    biome: BiomeRef,
    template_pools: &'a FxHashMap<Identifier, TemplatePoolData>,
    templates: &'a FxHashMap<Identifier, TemplateData>,
    surface_y_cache: Option<i32>,
}

impl FlatGenerationContext<'_> {
    fn state_at_y(&self, y: i32) -> Option<BlockStateId> {
        let relative_y = y.checked_sub(self.min_y)? as usize;
        self.layers.get(relative_y).copied().flatten()
    }

    fn is_opaque_at_y(&self, y: i32, ocean_floor: bool) -> bool {
        let Some(state) = self.state_at_y(y) else {
            return false;
        };
        if ocean_floor {
            state.is_solid()
        } else {
            state.is_solid() || state.has_fluid()
        }
    }

    fn base_height_flat(&self, ocean_floor: bool) -> i32 {
        for y in (self.min_y..self.min_y + self.height).rev() {
            if self.is_opaque_at_y(y, ocean_floor) {
                return y + 1;
            }
        }
        self.min_y
    }
}

impl StructureGenerationContext for FlatGenerationContext<'_> {
    fn seed(&self) -> i64 {
        self.seed
    }

    fn chunk_x(&self) -> i32 {
        self.chunk_x
    }

    fn chunk_z(&self) -> i32 {
        self.chunk_z
    }

    fn chunk_min_x(&self) -> i32 {
        self.chunk_min_x
    }

    fn chunk_min_z(&self) -> i32 {
        self.chunk_min_z
    }

    fn center_block_x(&self) -> i32 {
        self.center_block_x
    }

    fn center_block_z(&self) -> i32 {
        self.center_block_z
    }

    fn sea_level(&self) -> i32 {
        self.sea_level
    }

    fn min_y(&self) -> i32 {
        self.min_y
    }

    fn height(&self) -> i32 {
        self.height
    }

    fn template_pools(&self) -> &FxHashMap<Identifier, TemplatePoolData> {
        self.template_pools
    }

    fn templates(&self) -> &FxHashMap<Identifier, TemplateData> {
        self.templates
    }

    fn base_height(&mut self, _x: i32, _z: i32, ocean_floor: bool) -> i32 {
        self.base_height_flat(ocean_floor)
    }

    fn base_height_full(&mut self, _x: i32, _z: i32, ocean_floor: bool) -> i32 {
        self.base_height_flat(ocean_floor)
    }

    fn biome_at(&mut self, _block_x: i32, _block_y: i32, _block_z: i32) -> BiomeRef {
        self.biome
    }

    fn column_state(&mut self, _x: i32, y: i32, _z: i32) -> ColumnBlock {
        let Some(state) = self.state_at_y(y) else {
            return ColumnBlock::Air;
        };
        if state.is_solid() {
            ColumnBlock::Solid
        } else if state.has_fluid() {
            ColumnBlock::Fluid
        } else {
            ColumnBlock::Air
        }
    }

    fn surface_y(&mut self) -> i32 {
        if let Some(y) = self.surface_y_cache {
            return y;
        }
        let y = self.base_height_flat(false) - 1;
        self.surface_y_cache = Some(y);
        y
    }

    fn terrain_surface_height(&self, _x: i32, _z: i32, ocean_floor: bool) -> i32 {
        self.base_height_flat(ocean_floor)
    }

    fn terrain_is_opaque(&self, _x: i32, y: i32, _z: i32, ocean_floor: bool) -> bool {
        self.is_opaque_at_y(y, ocean_floor)
    }
}

impl FlatChunkGenerator {
    /// Runs `f` with a flat structure generation context for one chunk.
    fn with_structure_context<R>(
        &self,
        chunk_x: i32,
        chunk_z: i32,
        min_y: i32,
        height: i32,
        f: impl FnOnce(&StructureGenerator, &mut FlatGenerationContext<'_>) -> R,
    ) -> Option<R> {
        let structure_generator = self.structure_generator.as_ref()?;
        let chunk_min_x = chunk_x * 16;
        let chunk_min_z = chunk_z * 16;
        let mut ctx = FlatGenerationContext {
            seed: self.seed,
            chunk_x,
            chunk_z,
            chunk_min_x,
            chunk_min_z,
            center_block_x: chunk_min_x + 8,
            center_block_z: chunk_min_z + 8,
            sea_level: self.sea_level,
            min_y,
            height,
            layers: &self.layers,
            biome: self.biome,
            template_pools: structure_generator.template_pools(),
            templates: structure_generator.templates(),
            surface_y_cache: None,
        };
        Some(f(structure_generator, &mut ctx))
    }
}

impl ChunkGenerator for FlatChunkGenerator {
    fn min_y(&self) -> i32 {
        Self::MIN_Y
    }

    fn gen_depth(&self) -> i32 {
        Self::HEIGHT
    }

    fn noise_biome(&self, _quart_x: i32, _quart_y: i32, _quart_z: i32) -> BiomeRef {
        self.biome
    }

    fn spawn_height(&self, min_y: i32, height: i32) -> i32 {
        min_y + height.min(self.layers.len() as i32)
    }

    fn structure_generator(&self) -> Option<&StructureGenerator> {
        self.structure_generator.as_ref()
    }

    fn create_structures(&self, chunk: &Chunk) {
        let pos = chunk.pos();
        self.with_structure_context(
            pos.0.x,
            pos.0.y,
            chunk.min_y(),
            (chunk.sections().sections.len() * 16) as i32,
            |structure_generator, ctx| {
                create_structures(structure_generator, chunk, ctx);
            },
        );
    }

    fn generate_structure_start(
        &self,
        chunk_pos: ChunkPos,
        structure: &Identifier,
    ) -> Option<StructureStart> {
        self.with_structure_context(
            chunk_pos.0.x,
            chunk_pos.0.y,
            Self::MIN_Y,
            Self::HEIGHT,
            |structure_generator, ctx| {
                structure_generator.generate_start_for_structure(structure, ctx)
            },
        )
        .flatten()
    }

    fn create_biomes(&self, chunk: &Chunk) {
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
                            self.biome_id,
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
        chunk: GenerationChunk<'_, TerrainPhase>,
        _beardifier: Option<&Beardifier>,
    ) {
        let max_relative_y = chunk.section_count() * 16;

        for x in 0..16 {
            for z in 0..16 {
                for (relative_y, block) in self.layers.iter().enumerate().take(max_relative_y) {
                    let Some(block) = block else {
                        continue;
                    };
                    chunk.set_relative_block(x, relative_y, z, *block);
                }
            }
        }
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

    fn apply_biome_decorations(&self, region: &mut WorldGenRegion<'_>) {
        self.feature_runner
            .decorate_flat(region, &REGISTRY, self.seed, self.biome_zoom_seed);
    }
}
