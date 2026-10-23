use rustc_hash::FxHashMap;
use steel_utils::Identifier;

use crate::flat_level_generator_preset::FlatLevelGeneratorSettings;

/// Chunk generator used by one dimension of a world preset, mirroring the vanilla
/// `ChunkGenerator` codec variants.
#[derive(Debug)]
pub enum WorldPresetGenerator {
    /// Vanilla `NoiseBasedChunkGenerator`: noise settings plus a biome source.
    Noise {
        biome_source: PresetBiomeSource,
        settings: Identifier,
    },
    /// Vanilla `FlatLevelSource`: inline superflat settings.
    Flat {
        settings: FlatLevelGeneratorSettings,
    },
    /// Vanilla `DebugLevelSource`: the all-block-states debug generator.
    Debug,
}

/// Biome source referenced by a noise chunk generator.
///
/// Vanilla also serializes inline multi-noise parameter lists; every built-in world preset
/// uses one of the three forms modeled here.
#[derive(Debug)]
pub enum PresetBiomeSource {
    /// Vanilla `MultiNoiseBiomeSource` referencing a named `multi_noise` preset.
    MultiNoise { preset: Identifier },
    /// Vanilla `FixedBiomeSource` locked to a single biome.
    Fixed { biome: Identifier },
    /// Vanilla `TheEndBiomeSource`.
    TheEnd,
}

/// One dimension of a world preset, mirroring a vanilla `LevelStem`.
#[derive(Debug)]
pub struct WorldPresetDimension {
    /// Key of the dimension stem (`overworld`, `the_nether`, `the_end`, ...).
    pub dimension: Identifier,
    /// Dimension type the stem generates its level with.
    pub dimension_type: Identifier,
    pub generator: WorldPresetGenerator,
}

/// Represents a `world_preset` definition from a data pack JSON file.
#[derive(Debug)]
pub struct WorldPreset {
    pub key: Identifier,
    /// Dimensions ordered like vanilla `WorldDimensions.keysInOrder`
    /// (overworld, then nether, then end, then any others).
    pub dimensions: Vec<WorldPresetDimension>,
}

pub type WorldPresetRef = &'static WorldPreset;

pub struct WorldPresetRegistry {
    tags: crate::RegistryTags,
    world_presets_by_id: Vec<WorldPresetRef>,
    world_presets_by_key: FxHashMap<Identifier, usize>,
    allows_registering: bool,
}

impl WorldPresetRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            world_presets_by_id: Vec::new(),
            world_presets_by_key: FxHashMap::default(),
            allows_registering: true,
            tags: crate::RegistryTags::default(),
        }
    }
}

crate::impl_standard_methods!(
    WorldPresetRegistry,
    WorldPresetRef,
    world_presets_by_id,
    world_presets_by_key,
    allows_registering
);

crate::impl_registry!(
    WorldPresetRegistry,
    WorldPreset,
    world_presets_by_id,
    world_presets_by_key,
    world_presets
);
crate::impl_tagged_registry!(WorldPresetRegistry, world_presets_by_key, "preset");
