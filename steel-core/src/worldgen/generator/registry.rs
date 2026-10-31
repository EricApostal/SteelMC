//! Runtime registry for world generator factories.

use rustc_hash::{FxHashMap, FxHashSet};
use serde::Deserialize;
use std::iter::repeat_n;
use std::path::Path;
use std::sync::Arc;
use steel_registry::dimension_type::DimensionTypeRef;
use steel_registry::vanilla_dimension_types::{OVERWORLD, THE_END, THE_NETHER};
use steel_registry::world_preset::{PresetBiomeSource, WorldPresetGenerator};
use steel_registry::{REGISTRY, RegistryExt};
use steel_utils::Identifier;
use toml::map::Map;

use crate::worldgen::structure::{FixedStructureBiomeProvider, StructureGenerator};
use crate::worldgen::{
    ChunkGeneratorType, DebugChunkGenerator, EmptyChunkGenerator, FlatChunkGenerator,
    FlatGeneratorSettings, VanillaGenerator,
};
use steel_worldgen::biomes::BiomeSourceKind;
use steel_worldgen::density::NoiseSettingsKey;
use steel_worldgen::density_functions::{
    amplified::AmplifiedNoiseSettings, caves::CavesNoiseSettings, end::EndNoiseSettings,
    floating_islands::FloatingIslandsNoiseSettings, large_biomes::LargeBiomesNoiseSettings,
    nether::NetherNoiseSettings, overworld::OverworldNoiseSettings,
};
use steel_worldgen::structure::placement::load_vanilla_structure_sets;

/// Fully constructed generator metadata for a world.
pub struct GeneratorOutput {
    /// Vanilla dimension type rules used by this loaded world.
    pub dimension_type: DimensionTypeRef,
    /// Generator config after applying generator defaults.
    pub config: toml::Value,
    /// Chunk generator instance.
    pub generator: ChunkGeneratorType,
    /// Whether the client should treat this as a flat world.
    pub is_flat: bool,
    /// Sea level sent in login/respawn packets.
    pub sea_level: i32,
}

struct WorldGeneratorFactory {
    validate: fn(&toml::Value) -> Result<WorldGeneratorConfigData, String>,
    create: fn(
        Option<&Path>,
        &WorldGeneratorConfigData,
        i64,
        &rayon::ThreadPool,
    ) -> Result<GeneratorOutput, String>,
}

/// Generator config after parsing, validation, and default application.
#[derive(Debug, Clone)]
pub struct ValidatedWorldGeneratorConfig {
    generator: Identifier,
    data: WorldGeneratorConfigData,
}

impl ValidatedWorldGeneratorConfig {
    /// Generator factory identifier this config was validated for.
    #[must_use]
    pub const fn generator(&self) -> &Identifier {
        &self.generator
    }

    /// Dimension type selected by this validated generator config.
    #[must_use]
    pub fn dimension_type(&self) -> DimensionTypeRef {
        match &self.data {
            WorldGeneratorConfigData::Empty => fixed_generator_dimension_type(&self.generator),
            WorldGeneratorConfigData::EmptyWorld(config) => {
                validated_dimension_type_by_key(&config.dimension_type)
            }
            WorldGeneratorConfigData::Flat(config) => {
                validated_dimension_type_by_key(&config.dimension_type)
            }
            WorldGeneratorConfigData::Preset(config) => {
                validated_dimension_type_by_key(&config.dimension_type)
            }
        }
    }
}

#[derive(Debug, Clone)]
enum WorldGeneratorConfigData {
    Empty,
    EmptyWorld(DimensionTypeOnlyConfig),
    Flat(ResolvedFlatConfig),
    Preset(ResolvedPresetConfig),
}

/// Registry of server-side world generator factories.
pub struct WorldGeneratorRegistry {
    factories: FxHashMap<Identifier, WorldGeneratorFactory>,
}

impl WorldGeneratorRegistry {
    /// Creates a registry containing Steel's built-in generator factories.
    pub fn new_with_builtins() -> Result<Self, String> {
        let mut registry = Self {
            factories: FxHashMap::default(),
        };

        registry.register(
            Identifier::vanilla_static("overworld"),
            WorldGeneratorFactory {
                validate: validate_empty_config,
                create: create_overworld,
            },
        )?;
        registry.register(
            Identifier::vanilla_static("the_nether"),
            WorldGeneratorFactory {
                validate: validate_empty_config,
                create: create_nether,
            },
        )?;
        registry.register(
            Identifier::vanilla_static("the_end"),
            WorldGeneratorFactory {
                validate: validate_empty_config,
                create: create_end,
            },
        )?;
        registry.register(
            Identifier::vanilla_static("flat"),
            WorldGeneratorFactory {
                validate: validate_flat_config,
                create: create_flat,
            },
        )?;
        registry.register(
            Identifier::from_steel("empty"),
            WorldGeneratorFactory {
                validate: validate_empty_world_config,
                create: create_empty,
            },
        )?;
        registry.register(
            Identifier::vanilla_static("debug"),
            WorldGeneratorFactory {
                validate: validate_empty_config,
                create: create_debug,
            },
        )?;
        registry.register(
            Identifier::vanilla_static("preset"),
            WorldGeneratorFactory {
                validate: validate_preset_config,
                create: create_preset,
            },
        )?;

        Ok(registry)
    }

    fn register(&mut self, key: Identifier, factory: WorldGeneratorFactory) -> Result<(), String> {
        if self.factories.insert(key.clone(), factory).is_some() {
            return Err(format!("duplicate world generator registration {key}"));
        }
        Ok(())
    }

    /// Parses and validates config for a generator ID.
    pub fn validate_config(
        &self,
        key: &Identifier,
        config: &toml::Value,
    ) -> Result<ValidatedWorldGeneratorConfig, String> {
        let factory = self
            .factories
            .get(key)
            .ok_or_else(|| format!("unknown world generator {key}"))?;
        let data = (factory.validate)(config)?;
        Ok(ValidatedWorldGeneratorConfig {
            generator: key.clone(),
            data,
        })
    }

    /// Creates a generator from a validated generator ID and config.
    pub fn create(
        &self,
        world_path: Option<&Path>,
        config: &ValidatedWorldGeneratorConfig,
        seed: i64,
        thread_pool: Arc<rayon::ThreadPool>,
    ) -> Result<GeneratorOutput, String> {
        let factory = self
            .factories
            .get(&config.generator)
            .ok_or_else(|| format!("unknown world generator {}", config.generator))?;
        (factory.create)(world_path, &config.data, seed, &thread_pool)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct DimensionTypeOnlyConfig {
    dimension_type: Identifier,
}

/// Raw `minecraft:flat` config.
///
/// Every field is optional so a `preset` can supply it; explicit fields override the
/// preset, mirroring vanilla's superflat preset selection screen.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FlatGeneratorConfig {
    #[serde(default = "default_flat_dimension_type")]
    dimension_type: Identifier,
    #[serde(default)]
    preset: Option<Identifier>,
    #[serde(default)]
    biome: Option<Identifier>,
    #[serde(default)]
    layers: Option<Vec<FlatLayerConfig>>,
    #[serde(default)]
    features: Option<bool>,
    #[serde(default)]
    lakes: Option<bool>,
    #[serde(default)]
    structure_overrides: Option<Vec<Identifier>>,
}

/// `minecraft:flat` config after applying preset and vanilla defaults.
#[derive(Debug, Clone)]
struct ResolvedFlatConfig {
    dimension_type: Identifier,
    biome: Identifier,
    layers: Vec<FlatLayerConfig>,
    features: bool,
    lakes: bool,
    structure_overrides: Vec<Identifier>,
}

/// Raw `minecraft:preset` config: one dimension of a vanilla world preset.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PresetGeneratorConfig {
    #[serde(default = "default_world_preset")]
    preset: Identifier,
    #[serde(default = "default_preset_dimension")]
    dimension: Identifier,
}

/// `minecraft:preset` config resolved to one dimension's generator.
#[derive(Debug, Clone)]
struct ResolvedPresetConfig {
    preset: Identifier,
    dimension: Identifier,
    dimension_type: Identifier,
    generator: PresetDimensionGenerator,
}

/// Chunk generator selected by a world preset dimension.
#[derive(Debug, Clone)]
enum PresetDimensionGenerator {
    Noise {
        biome_source: PresetNoiseBiomeSource,
        settings: NoiseSettingsKey,
    },
    Flat(ResolvedFlatConfig),
    Debug,
}

/// Biome source of a preset's noise generator, resolved to a Steel variant.
#[derive(Debug, Clone)]
enum PresetNoiseBiomeSource {
    Overworld,
    Nether,
    End,
    Fixed(Identifier),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct FlatLayerConfig {
    block: Identifier,
    height: usize,
}

const fn default_flat_dimension_type() -> Identifier {
    Identifier::vanilla_static("overworld")
}

const fn default_world_preset() -> Identifier {
    Identifier::vanilla_static("normal")
}

const fn default_preset_dimension() -> Identifier {
    Identifier::vanilla_static("overworld")
}

fn default_flat_layers() -> Vec<FlatLayerConfig> {
    vec![
        FlatLayerConfig {
            block: Identifier::vanilla_static("bedrock"),
            height: 1,
        },
        FlatLayerConfig {
            block: Identifier::vanilla_static("dirt"),
            height: 2,
        },
        FlatLayerConfig {
            block: Identifier::vanilla_static("grass_block"),
            height: 1,
        },
    ]
}

fn default_flat_structure_overrides() -> Vec<Identifier> {
    vec![
        Identifier::vanilla_static("strongholds"),
        Identifier::vanilla_static("villages"),
    ]
}

fn validate_empty_config(config: &toml::Value) -> Result<WorldGeneratorConfigData, String> {
    let Some(table) = config.as_table() else {
        return Err("generator config must be a table".to_owned());
    };
    if !table.is_empty() {
        return Err("this generator does not accept config".to_owned());
    }
    Ok(WorldGeneratorConfigData::Empty)
}

fn validate_empty_world_config(config: &toml::Value) -> Result<WorldGeneratorConfigData, String> {
    let parsed: DimensionTypeOnlyConfig = config
        .clone()
        .try_into()
        .map_err(|e| format!("invalid steel:empty config: {e}"))?;
    dimension_type_by_key(&parsed.dimension_type)?;
    Ok(WorldGeneratorConfigData::EmptyWorld(parsed))
}

fn validate_flat_config(config: &toml::Value) -> Result<WorldGeneratorConfigData, String> {
    let parsed = parse_flat_config(config)?;
    let resolved = resolve_flat_config(&parsed)?;
    validate_flat_settings(&resolved)?;
    Ok(WorldGeneratorConfigData::Flat(resolved))
}

/// Resolves vanilla's flat preset/default cascade: explicit config fields win, then the
/// selected `flat_level_generator_preset`'s settings, then vanilla's classic defaults.
fn resolve_flat_config(parsed: &FlatGeneratorConfig) -> Result<ResolvedFlatConfig, String> {
    let preset = match &parsed.preset {
        Some(key) => Some(
            REGISTRY
                .flat_level_generator_presets
                .by_key(key)
                .ok_or_else(|| format!("unknown flat level generator preset {key}"))?,
        ),
        None => None,
    };

    let biome = parsed
        .biome
        .clone()
        .or_else(|| preset.map(|preset| preset.settings.biome.clone()))
        .unwrap_or_else(|| Identifier::vanilla_static("plains"));

    let layers = match &parsed.layers {
        Some(layers) => layers.clone(),
        None => preset.map_or_else(default_flat_layers, |preset| {
            preset
                .settings
                .layers
                .iter()
                .map(|layer| FlatLayerConfig {
                    block: layer.block.clone(),
                    height: layer.height as usize,
                })
                .collect()
        }),
    };

    let features = parsed
        .features
        .or_else(|| preset.map(|preset| preset.settings.decoration))
        .unwrap_or(false);
    let lakes = parsed
        .lakes
        .or_else(|| preset.map(|preset| preset.settings.add_lakes))
        .unwrap_or(false);
    let structure_overrides = parsed
        .structure_overrides
        .clone()
        .or_else(|| preset.map(|preset| preset.settings.structure_overrides.clone()))
        .unwrap_or_else(default_flat_structure_overrides);

    Ok(ResolvedFlatConfig {
        dimension_type: parsed.dimension_type.clone(),
        biome,
        layers,
        features,
        lakes,
        structure_overrides,
    })
}

fn validate_flat_settings(config: &ResolvedFlatConfig) -> Result<(), String> {
    if config.layers.is_empty() {
        return Err("minecraft:flat requires at least one layer".to_owned());
    }
    dimension_type_by_key(&config.dimension_type)?;
    if REGISTRY.biomes.by_key(&config.biome).is_none() {
        return Err(format!(
            "unknown biome {} in minecraft:flat settings",
            config.biome
        ));
    }
    let mut total_height = 0usize;
    for layer in &config.layers {
        if layer.height == 0 {
            return Err("minecraft:flat layer height must be greater than zero".to_owned());
        }
        total_height = total_height.saturating_add(layer.height);
        if REGISTRY.blocks.by_key(&layer.block).is_none() {
            return Err(format!(
                "unknown block {} in minecraft:flat layer",
                layer.block
            ));
        }
    }
    // Vanilla `FlatLevelGeneratorSettings.validateHeight`: the summed layer heights must
    // fit the dimension's build height.
    if total_height > 384 {
        return Err(format!(
            "sum of minecraft:flat layer heights is {total_height}, above the 384-block limit"
        ));
    }
    let available_structure_sets: FxHashSet<_> = load_vanilla_structure_sets()
        .into_iter()
        .map(|(key, _)| key)
        .collect();
    for structure_set in &config.structure_overrides {
        if !available_structure_sets.contains(structure_set) {
            return Err(format!(
                "unknown structure set {structure_set} in minecraft:flat structure_overrides"
            ));
        }
    }
    Ok(())
}

fn validate_preset_config(config: &toml::Value) -> Result<WorldGeneratorConfigData, String> {
    let parsed: PresetGeneratorConfig = config
        .clone()
        .try_into()
        .map_err(|e| format!("invalid minecraft:preset config: {e}"))?;
    let preset = REGISTRY
        .world_presets
        .by_key(&parsed.preset)
        .ok_or_else(|| format!("unknown world preset {}", parsed.preset))?;
    let dimension = preset
        .dimensions
        .iter()
        .find(|dimension| dimension.dimension == parsed.dimension)
        .ok_or_else(|| {
            format!(
                "world preset {} has no dimension {}",
                parsed.preset, parsed.dimension
            )
        })?;
    dimension_type_by_key(&dimension.dimension_type)?;

    let generator = match &dimension.generator {
        WorldPresetGenerator::Noise {
            biome_source,
            settings,
        } => {
            let biome_source = match biome_source {
                PresetBiomeSource::MultiNoise { preset } => {
                    if preset == &Identifier::vanilla_static("overworld") {
                        PresetNoiseBiomeSource::Overworld
                    } else if preset == &Identifier::vanilla_static("nether") {
                        PresetNoiseBiomeSource::Nether
                    } else {
                        return Err(format!(
                            "multi-noise preset {preset} in world preset {} is not supported",
                            parsed.preset
                        ));
                    }
                }
                PresetBiomeSource::Fixed { biome } => {
                    if REGISTRY.biomes.by_key(biome).is_none() {
                        return Err(format!(
                            "unknown biome {biome} in world preset {}",
                            parsed.preset
                        ));
                    }
                    PresetNoiseBiomeSource::Fixed(biome.clone())
                }
                PresetBiomeSource::TheEnd => PresetNoiseBiomeSource::End,
            };
            let settings = resolve_preset_noise_settings(settings, &parsed.preset)?;
            PresetDimensionGenerator::Noise {
                biome_source,
                settings,
            }
        }
        WorldPresetGenerator::Flat { settings } => {
            let flat = ResolvedFlatConfig {
                dimension_type: dimension.dimension_type.clone(),
                biome: settings.biome.clone(),
                layers: settings
                    .layers
                    .iter()
                    .map(|layer| FlatLayerConfig {
                        block: layer.block.clone(),
                        height: layer.height as usize,
                    })
                    .collect(),
                features: settings.decoration,
                lakes: settings.add_lakes,
                structure_overrides: settings.structure_overrides.clone(),
            };
            validate_flat_settings(&flat)?;
            PresetDimensionGenerator::Flat(flat)
        }
        WorldPresetGenerator::Debug => PresetDimensionGenerator::Debug,
    };

    Ok(WorldGeneratorConfigData::Preset(ResolvedPresetConfig {
        preset: parsed.preset,
        dimension: parsed.dimension,
        dimension_type: dimension.dimension_type.clone(),
        generator,
    }))
}

/// Resolves a preset dimension's `noise_settings` reference to a transpiled settings key.
///
/// Settings without transpiled density functions are rejected rather than silently
/// falling back to the overworld's.
fn resolve_preset_noise_settings(
    settings: &Identifier,
    preset: &Identifier,
) -> Result<NoiseSettingsKey, String> {
    NoiseSettingsKey::from_identifier(settings).ok_or_else(|| {
        format!("noise settings {settings} in world preset {preset} are not supported")
    })
}

fn parse_flat_config(config: &toml::Value) -> Result<FlatGeneratorConfig, String> {
    config
        .clone()
        .try_into()
        .map_err(|e| format!("invalid minecraft:flat config: {e}"))
}

fn create_overworld(
    world_path: Option<&Path>,
    config: &WorldGeneratorConfigData,
    seed: i64,
    thread_pool: &rayon::ThreadPool,
) -> Result<GeneratorOutput, String> {
    let WorldGeneratorConfigData::Empty = config else {
        return Err("validated config does not match minecraft:overworld".to_owned());
    };
    let seed = seed as u64;
    Ok(GeneratorOutput {
        dimension_type: &OVERWORLD,
        config: empty_config(),
        generator: ChunkGeneratorType::Overworld(VanillaGenerator::new(
            world_path,
            BiomeSourceKind::overworld(seed),
            seed,
            thread_pool,
        )),
        is_flat: false,
        sea_level: sea_level_for_dimension_type(&OVERWORLD),
    })
}

fn create_nether(
    world_path: Option<&Path>,
    config: &WorldGeneratorConfigData,
    seed: i64,
    thread_pool: &rayon::ThreadPool,
) -> Result<GeneratorOutput, String> {
    let WorldGeneratorConfigData::Empty = config else {
        return Err("validated config does not match minecraft:the_nether".to_owned());
    };
    let seed = seed as u64;
    Ok(GeneratorOutput {
        dimension_type: &THE_NETHER,
        config: empty_config(),
        generator: ChunkGeneratorType::Nether(VanillaGenerator::new(
            world_path,
            BiomeSourceKind::nether(seed),
            seed,
            thread_pool,
        )),
        is_flat: false,
        sea_level: sea_level_for_dimension_type(&THE_NETHER),
    })
}

fn create_end(
    world_path: Option<&Path>,
    config: &WorldGeneratorConfigData,
    seed: i64,
    thread_pool: &rayon::ThreadPool,
) -> Result<GeneratorOutput, String> {
    let WorldGeneratorConfigData::Empty = config else {
        return Err("validated config does not match minecraft:the_end".to_owned());
    };
    let seed = seed as u64;
    Ok(GeneratorOutput {
        dimension_type: &THE_END,
        config: empty_config(),
        generator: ChunkGeneratorType::End(VanillaGenerator::new(
            world_path,
            BiomeSourceKind::end(seed),
            seed,
            thread_pool,
        )),
        is_flat: false,
        sea_level: sea_level_for_dimension_type(&THE_END),
    })
}

fn create_flat(
    world_path: Option<&Path>,
    config: &WorldGeneratorConfigData,
    seed: i64,
    thread_pool: &rayon::ThreadPool,
) -> Result<GeneratorOutput, String> {
    let WorldGeneratorConfigData::Flat(parsed) = config else {
        return Err("validated config does not match minecraft:flat".to_owned());
    };
    create_flat_generator(world_path, parsed, seed, thread_pool)
}

/// Builds a flat generator from resolved flat settings.
fn create_flat_generator(
    world_path: Option<&Path>,
    parsed: &ResolvedFlatConfig,
    seed: i64,
    thread_pool: &rayon::ThreadPool,
) -> Result<GeneratorOutput, String> {
    let dimension_type = dimension_type_by_key(&parsed.dimension_type)?;
    let normalized_config = normalized_flat_config(parsed);
    let biome = REGISTRY
        .biomes
        .by_key(&parsed.biome)
        .ok_or_else(|| format!("unknown biome {} in minecraft:flat settings", parsed.biome))?;
    let mut layers = Vec::new();
    for layer in &parsed.layers {
        let block = REGISTRY
            .blocks
            .by_key(&layer.block)
            .ok_or_else(|| format!("unknown block {} in minecraft:flat layer", layer.block))?;
        let state = REGISTRY.blocks.get_default_state_id(block);
        layers.extend(repeat_n(Some(state), layer.height));
    }

    let structure_generator = if parsed.structure_overrides.is_empty() {
        None
    } else {
        let structure_sets = load_vanilla_structure_sets()
            .into_iter()
            .filter(|(key, _)| parsed.structure_overrides.contains(key))
            .collect();
        let biome_provider = FixedStructureBiomeProvider::new(biome);
        Some(StructureGenerator::vanilla_flat_with_structure_sets(
            seed,
            world_path,
            &biome_provider,
            structure_sets,
            thread_pool,
        ))
    };

    Ok(GeneratorOutput {
        dimension_type,
        config: normalized_config,
        generator: ChunkGeneratorType::Flat(FlatChunkGenerator::from_settings(
            FlatGeneratorSettings {
                layers,
                biome,
                seed,
                sea_level: FlatChunkGenerator::SEA_LEVEL,
                features: parsed.features,
                lakes: parsed.lakes,
                structure_generator,
            },
        )),
        is_flat: true,
        sea_level: FlatChunkGenerator::SEA_LEVEL,
    })
}

fn create_debug(
    _world_path: Option<&Path>,
    config: &WorldGeneratorConfigData,
    _seed: i64,
    _thread_pool: &rayon::ThreadPool,
) -> Result<GeneratorOutput, String> {
    let WorldGeneratorConfigData::Empty = config else {
        return Err("validated config does not match minecraft:debug".to_owned());
    };
    Ok(GeneratorOutput {
        dimension_type: &OVERWORLD,
        config: empty_config(),
        generator: ChunkGeneratorType::Debug(DebugChunkGenerator::new()),
        is_flat: false,
        sea_level: DebugChunkGenerator::SEA_LEVEL,
    })
}

fn create_preset(
    world_path: Option<&Path>,
    config: &WorldGeneratorConfigData,
    seed: i64,
    thread_pool: &rayon::ThreadPool,
) -> Result<GeneratorOutput, String> {
    let WorldGeneratorConfigData::Preset(parsed) = config else {
        return Err("validated config does not match minecraft:preset".to_owned());
    };
    let dimension_type = dimension_type_by_key(&parsed.dimension_type)?;
    let normalized_config = normalized_preset_config(parsed);

    match &parsed.generator {
        PresetDimensionGenerator::Noise {
            biome_source,
            settings,
        } => {
            let seed_u = seed as u64;
            let source = match biome_source {
                // Vanilla derives the multi-noise climate sampler from the generator's
                // noise settings: large biomes widens biomes through the
                // `overworld_large_biomes` climate router while keeping the overworld
                // parameter list. `amplified` reuses the overworld climate router.
                PresetNoiseBiomeSource::Overworld => {
                    if *settings == NoiseSettingsKey::LargeBiomes {
                        BiomeSourceKind::large_biomes(seed_u)
                    } else {
                        BiomeSourceKind::overworld(seed_u)
                    }
                }
                PresetNoiseBiomeSource::Nether => BiomeSourceKind::nether(seed_u),
                PresetNoiseBiomeSource::End => BiomeSourceKind::end(seed_u),
                PresetNoiseBiomeSource::Fixed(biome) => {
                    let biome = REGISTRY.biomes.by_key(biome).ok_or_else(|| {
                        format!("unknown biome {biome} in world preset {}", parsed.preset)
                    })?;
                    BiomeSourceKind::fixed(biome)
                }
            };
            let generator = match settings {
                NoiseSettingsKey::Overworld => ChunkGeneratorType::Overworld(
                    VanillaGenerator::new(world_path, source, seed_u, thread_pool),
                ),
                NoiseSettingsKey::Nether => ChunkGeneratorType::Nether(VanillaGenerator::new(
                    world_path,
                    source,
                    seed_u,
                    thread_pool,
                )),
                NoiseSettingsKey::End => ChunkGeneratorType::End(VanillaGenerator::new(
                    world_path,
                    source,
                    seed_u,
                    thread_pool,
                )),
                NoiseSettingsKey::LargeBiomes => ChunkGeneratorType::LargeBiomes(
                    VanillaGenerator::new(world_path, source, seed_u, thread_pool),
                ),
                NoiseSettingsKey::Amplified => ChunkGeneratorType::Amplified(
                    VanillaGenerator::new(world_path, source, seed_u, thread_pool),
                ),
                NoiseSettingsKey::Caves => ChunkGeneratorType::Caves(VanillaGenerator::new(
                    world_path,
                    source,
                    seed_u,
                    thread_pool,
                )),
                NoiseSettingsKey::FloatingIslands => ChunkGeneratorType::FloatingIslands(
                    VanillaGenerator::new(world_path, source, seed_u, thread_pool),
                ),
            };
            Ok(GeneratorOutput {
                dimension_type,
                config: normalized_config,
                generator,
                is_flat: false,
                sea_level: sea_level_for_noise_settings(*settings),
            })
        }
        PresetDimensionGenerator::Flat(flat) => {
            create_flat_generator(world_path, flat, seed, thread_pool)
        }
        PresetDimensionGenerator::Debug => Ok(GeneratorOutput {
            dimension_type,
            config: normalized_config,
            generator: ChunkGeneratorType::Debug(DebugChunkGenerator::new()),
            is_flat: false,
            sea_level: DebugChunkGenerator::SEA_LEVEL,
        }),
    }
}

fn create_empty(
    _world_path: Option<&Path>,
    config: &WorldGeneratorConfigData,
    _seed: i64,
    _thread_pool: &rayon::ThreadPool,
) -> Result<GeneratorOutput, String> {
    let WorldGeneratorConfigData::EmptyWorld(parsed) = config else {
        return Err("validated config does not match steel:empty".to_owned());
    };
    let dimension_type = dimension_type_by_key(&parsed.dimension_type)?;
    Ok(GeneratorOutput {
        dimension_type,
        config: normalized_dimension_type_config(&parsed.dimension_type),
        generator: ChunkGeneratorType::Empty(EmptyChunkGenerator::new()),
        is_flat: false,
        sea_level: sea_level_for_dimension_type(dimension_type),
    })
}

fn empty_config() -> toml::Value {
    toml::Value::Table(Map::new())
}

fn normalized_dimension_type_config(dimension_type: &Identifier) -> toml::Value {
    toml::Value::Table(normalized_dimension_type_table(dimension_type))
}

fn normalized_dimension_type_table(dimension_type: &Identifier) -> Map<String, toml::Value> {
    let mut table = Map::new();
    table.insert(
        "dimension_type".to_owned(),
        toml::Value::String(dimension_type.to_string()),
    );
    table
}

fn normalized_flat_config(config: &ResolvedFlatConfig) -> toml::Value {
    let mut table = normalized_dimension_type_table(&config.dimension_type);
    table.insert(
        "biome".to_owned(),
        toml::Value::String(config.biome.to_string()),
    );
    let layers = config
        .layers
        .iter()
        .map(|layer| {
            let mut layer_table = Map::new();
            layer_table.insert(
                "block".to_owned(),
                toml::Value::String(layer.block.to_string()),
            );
            layer_table.insert(
                "height".to_owned(),
                toml::Value::Integer(layer.height as i64),
            );
            toml::Value::Table(layer_table)
        })
        .collect();
    table.insert("layers".to_owned(), toml::Value::Array(layers));
    table.insert("features".to_owned(), toml::Value::Boolean(config.features));
    table.insert("lakes".to_owned(), toml::Value::Boolean(config.lakes));
    table.insert(
        "structure_overrides".to_owned(),
        toml::Value::Array(
            config
                .structure_overrides
                .iter()
                .map(|key| toml::Value::String(key.to_string()))
                .collect(),
        ),
    );
    toml::Value::Table(table)
}

fn normalized_preset_config(config: &ResolvedPresetConfig) -> toml::Value {
    let mut table = normalized_dimension_type_table(&config.dimension_type);
    table.insert(
        "preset".to_owned(),
        toml::Value::String(config.preset.to_string()),
    );
    table.insert(
        "dimension".to_owned(),
        toml::Value::String(config.dimension.to_string()),
    );
    toml::Value::Table(table)
}

fn dimension_type_by_key(key: &Identifier) -> Result<DimensionTypeRef, String> {
    REGISTRY
        .dimension_types
        .by_key(key)
        .ok_or_else(|| format!("unknown dimension type {key}"))
}

fn validated_dimension_type_by_key(key: &Identifier) -> DimensionTypeRef {
    match dimension_type_by_key(key) {
        Ok(dimension_type) => dimension_type,
        Err(error) => panic!("validated generator config should have a dimension type: {error}"),
    }
}

fn fixed_generator_dimension_type(generator: &Identifier) -> DimensionTypeRef {
    if generator == &Identifier::vanilla_static("overworld") {
        &OVERWORLD
    } else if generator == &Identifier::vanilla_static("the_nether") {
        &THE_NETHER
    } else if generator == &Identifier::vanilla_static("the_end") {
        &THE_END
    } else {
        panic!("validated empty config does not have a fixed dimension type for {generator}")
    }
}

fn sea_level_for_dimension_type(dimension_type: DimensionTypeRef) -> i32 {
    if dimension_type == &THE_NETHER {
        NetherNoiseSettings::SEA_LEVEL
    } else if dimension_type == &THE_END {
        EndNoiseSettings::SEA_LEVEL
    } else {
        OverworldNoiseSettings::SEA_LEVEL
    }
}

/// Sea level of a preset's noise settings. A preset may pair a dimension type with
/// non-default settings (`caves`, `floating_islands`), so the settings are authoritative.
const fn sea_level_for_noise_settings(settings: NoiseSettingsKey) -> i32 {
    match settings {
        NoiseSettingsKey::Overworld => OverworldNoiseSettings::SEA_LEVEL,
        NoiseSettingsKey::Nether => NetherNoiseSettings::SEA_LEVEL,
        NoiseSettingsKey::End => EndNoiseSettings::SEA_LEVEL,
        NoiseSettingsKey::LargeBiomes => LargeBiomesNoiseSettings::SEA_LEVEL,
        NoiseSettingsKey::Amplified => AmplifiedNoiseSettings::SEA_LEVEL,
        NoiseSettingsKey::Caves => CavesNoiseSettings::SEA_LEVEL,
        NoiseSettingsKey::FloatingIslands => FloatingIslandsNoiseSettings::SEA_LEVEL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::behavior::init_behaviors;
    use crate::chunk::Chunk;
    use crate::chunk::section::{ChunkSection, Sections};
    use crate::worldgen::generator::{ChunkGenerator as _, GenerationChunk, TerrainPhase};
    use glam::IVec3;
    use std::sync::Weak;
    use steel_registry::init_vanilla_registry;
    use steel_registry::{vanilla_biomes, vanilla_blocks, vanilla_dimension_types};
    use steel_worldgen::biomes::BiomeSourceKind;

    fn make_overworld_chunk() -> Chunk {
        let dimension = &vanilla_dimension_types::OVERWORLD;
        let sections = (0..dimension.height / 16)
            .map(|_| ChunkSection::new_empty())
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Chunk::new(
            Sections::from_owned(sections),
            steel_utils::ChunkPos::new(0, 0),
            dimension.min_y,
            dimension.height,
            Weak::new(),
        )
    }

    fn self_neighbor_biome(chunk: &Chunk, quart: IVec3) -> u16 {
        let sections = chunk.sections();
        let min_quart_y = chunk.min_y() >> 2;
        let quart_y =
            (quart.y - min_quart_y).clamp(0, (sections.sections.len() * 4) as i32 - 1) as usize;
        sections.sections[quart_y / 4].read().biomes.get(
            (quart.x & 3) as usize,
            quart_y % 4,
            (quart.z & 3) as usize,
        )
    }

    fn generated_blocks(output: &GeneratorOutput) -> Vec<steel_utils::BlockStateId> {
        let chunk = make_overworld_chunk();
        output.generator.create_biomes(&chunk);
        output.generator.build_terrain(
            GenerationChunk::<TerrainPhase>::for_test(&chunk),
            None,
            &|quart: IVec3| self_neighbor_biome(&chunk, quart),
        );
        let mut blocks = Vec::with_capacity((chunk.height() * 16 * 16) as usize);
        for relative_y in 0..chunk.height() as usize {
            for z in 0..16 {
                for x in 0..16 {
                    let Some(state) = chunk.get_relative_block(x, relative_y, z) else {
                        panic!("test coordinates must stay inside the chunk");
                    };
                    blocks.push(state);
                }
            }
        }
        blocks
    }

    fn preset_output<'a>(
        outputs: &'a [(&str, GeneratorOutput)],
        preset: &str,
    ) -> &'a GeneratorOutput {
        &outputs
            .iter()
            .find(|(name, _)| *name == preset)
            .unwrap_or_else(|| panic!("{preset} should have been created"))
            .1
    }

    #[test]
    fn default_flat_config_matches_vanilla_superflat() {
        use std::panic;
        use std::thread;

        // Larger stack to match chunk_stage_hashes.rs / structure_starts.rs —
        // loading the default structure overrides (strongholds, villages)
        // recurses deeply for jigsaw assembly.
        let result = thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(default_flat_config_matches_vanilla_superflat_inner)
            .expect("Failed to spawn test thread")
            .join();

        if let Err(payload) = result {
            panic::resume_unwind(payload);
        }
    }

    fn default_flat_config_matches_vanilla_superflat_inner() {
        init_vanilla_registry();

        let registry = WorldGeneratorRegistry::new_with_builtins()
            .expect("built-in generator registry should initialize");
        let config = registry
            .validate_config(
                &Identifier::vanilla_static("flat"),
                &toml::Value::Table(Map::new()),
            )
            .expect("default flat config should validate");
        let thread_pool = Arc::new(
            rayon::ThreadPoolBuilder::default()
                .build()
                .expect("Cannot create a thread pool."),
        );
        let output = registry
            .create(None, &config, 0, thread_pool)
            .expect("default flat config should create a generator");
        let config = output
            .config
            .as_table()
            .expect("normalized flat config should be a table");

        assert_eq!(
            config.get("dimension_type"),
            Some(&toml::Value::String("minecraft:overworld".to_owned()))
        );
        assert_eq!(config.get("features"), Some(&toml::Value::Boolean(false)));
        assert_eq!(config.get("lakes"), Some(&toml::Value::Boolean(false)));
        assert_eq!(
            config.get("structure_overrides"),
            Some(&toml::Value::Array(vec![
                toml::Value::String("minecraft:strongholds".to_owned()),
                toml::Value::String("minecraft:villages".to_owned()),
            ]))
        );
    }

    #[test]
    fn flat_decoration_options_resolve_and_unknown_structure_sets_are_rejected() {
        // `validate_flat_config` resolves structure sets through the global registry.
        init_vanilla_registry();

        for key in ["features", "lakes"] {
            let config = toml::Value::Table(Map::from_iter([(
                key.to_owned(),
                toml::Value::Boolean(true),
            )]));
            let WorldGeneratorConfigData::Flat(resolved) = validate_flat_config(&config)
                .unwrap_or_else(|error| panic!("{key}=true should validate: {error}"))
            else {
                panic!("{key}=true should resolve a flat generator");
            };
            match key {
                "features" => assert!(resolved.features),
                _ => assert!(resolved.lakes),
            }
        }

        // A structure set that vanilla does not define is still rejected.
        let unknown_config = toml::Value::Table(Map::from_iter([(
            "structure_overrides".to_owned(),
            toml::Value::Array(vec![toml::Value::String(
                "minecraft:not_a_structure_set".to_owned(),
            )]),
        )]));
        let error = validate_flat_config(&unknown_config)
            .expect_err("an unknown structure set should be rejected");
        assert!(error.contains("unknown structure set"), "{error}");
    }

    #[test]
    fn vanilla_presets_select_their_noise_settings() {
        use std::panic;
        use std::thread;

        // Larger stack to match the other generation tests — noise and carver
        // generation recurses deeply enough to overflow the default stack.
        let result = thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(vanilla_presets_select_their_noise_settings_inner)
            .expect("Failed to spawn test thread")
            .join();

        if let Err(payload) = result {
            panic::resume_unwind(payload);
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "resolves every vanilla preset and asserts each overworld noise settings entry end-to-end"
    )]
    fn vanilla_presets_select_their_noise_settings_inner() {
        init_vanilla_registry();
        init_behaviors();

        let registry = WorldGeneratorRegistry::new_with_builtins()
            .expect("built-in generator registry should initialize");
        let thread_pool = Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(1)
                .build()
                .expect("Cannot create a thread pool."),
        );

        // Every vanilla preset resolves through the registry to the overworld stem.
        let presets = [
            "normal",
            "large_biomes",
            "amplified",
            "single_biome_surface",
            "flat",
            "flat_all_dimensions",
            "debug_all_block_states",
        ];
        let outputs: Vec<(&str, GeneratorOutput)> = presets
            .iter()
            .map(|preset| {
                (
                    *preset,
                    create_preset_generator(&registry, preset, &thread_pool),
                )
            })
            .collect();

        for (preset, output) in &outputs {
            assert_eq!(
                output.dimension_type, &OVERWORLD,
                "{preset} should resolve the overworld dimension type"
            );
        }

        // Noise presets pick the transpiled settings entry from the datapack.
        for preset in ["normal", "single_biome_surface"] {
            assert!(
                matches!(
                    &preset_output(&outputs, preset).generator,
                    ChunkGeneratorType::Overworld(_)
                ),
                "{preset} should use the overworld noise settings"
            );
        }
        assert!(
            matches!(
                &preset_output(&outputs, "large_biomes").generator,
                ChunkGeneratorType::LargeBiomes(_)
            ),
            "large_biomes should use the large biomes noise settings"
        );
        assert!(
            matches!(
                &preset_output(&outputs, "amplified").generator,
                ChunkGeneratorType::Amplified(_)
            ),
            "amplified should use the amplified noise settings"
        );

        let ChunkGeneratorType::Overworld(normal_generator) =
            &preset_output(&outputs, "normal").generator
        else {
            unreachable!("normal preset checked above")
        };
        let ChunkGeneratorType::LargeBiomes(large_biomes_generator) =
            &preset_output(&outputs, "large_biomes").generator
        else {
            unreachable!("large_biomes preset checked above")
        };
        let ChunkGeneratorType::Amplified(amplified_generator) =
            &preset_output(&outputs, "amplified").generator
        else {
            panic!("amplified should use the amplified noise settings")
        };
        let ChunkGeneratorType::Overworld(single_biome_generator) =
            &preset_output(&outputs, "single_biome_surface").generator
        else {
            unreachable!("single_biome_surface checked above")
        };

        // Vanilla derives the climate sampler from the chunk generator's settings:
        // large biomes widens biomes through the large-biomes climate router, while
        // amplified reuses the overworld climate router and parameter list.
        assert!(matches!(
            normal_generator.biome_source(),
            BiomeSourceKind::Overworld(_)
        ));
        assert!(matches!(
            large_biomes_generator.biome_source(),
            BiomeSourceKind::LargeBiomes(_)
        ));
        assert!(matches!(
            amplified_generator.biome_source(),
            BiomeSourceKind::Overworld(_)
        ));
        assert!(matches!(
            single_biome_generator.biome_source(),
            BiomeSourceKind::Fixed(_)
        ));
        assert_eq!(
            single_biome_generator.noise_biome(0, 0, 0).key,
            vanilla_biomes::PLAINS.key
        );
        assert_eq!(
            preset_output(&outputs, "large_biomes").sea_level,
            LargeBiomesNoiseSettings::SEA_LEVEL
        );

        let air = REGISTRY.blocks.get_default_state_id(&vanilla_blocks::AIR);
        let normal_blocks = generated_blocks(preset_output(&outputs, "normal"));
        let large_biomes_blocks = generated_blocks(preset_output(&outputs, "large_biomes"));
        let amplified_blocks = generated_blocks(preset_output(&outputs, "amplified"));
        for blocks in [&normal_blocks, &large_biomes_blocks, &amplified_blocks] {
            assert!(
                blocks.iter().any(|state| *state != air),
                "a generated chunk should contain solid terrain"
            );
        }
        assert_ne!(
            normal_blocks, large_biomes_blocks,
            "large biomes should shape terrain with its own density functions"
        );
        assert_ne!(
            normal_blocks, amplified_blocks,
            "amplified should shape terrain with its own density functions"
        );
    }

    fn create_preset_generator(
        registry: &WorldGeneratorRegistry,
        preset: &str,
        thread_pool: &Arc<rayon::ThreadPool>,
    ) -> GeneratorOutput {
        let config = toml::Value::Table(Map::from_iter([
            (
                "preset".to_owned(),
                toml::Value::String(format!("minecraft:{preset}")),
            ),
            (
                "dimension".to_owned(),
                toml::Value::String("minecraft:overworld".to_owned()),
            ),
        ]));
        let validated = registry
            .validate_config(&Identifier::vanilla_static("preset"), &config)
            .unwrap_or_else(|error| panic!("{preset} should validate: {error}"));
        registry
            .create(None, &validated, 0, Arc::clone(thread_pool))
            .unwrap_or_else(|error| panic!("{preset} should create a generator: {error}"))
    }

    #[test]
    fn unknown_noise_settings_are_rejected() {
        let error = resolve_preset_noise_settings(
            &Identifier::vanilla_static("not_a_noise_settings"),
            &Identifier::vanilla_static("normal"),
        )
        .expect_err("unknown noise settings should be rejected");
        assert!(
            error.contains("noise settings minecraft:not_a_noise_settings"),
            "{error}"
        );

        // Every transpiled settings entry still resolves to its key, including the
        // ones only reachable through custom presets today.
        for key in NoiseSettingsKey::ALL {
            assert_eq!(
                resolve_preset_noise_settings(
                    &key.identifier(),
                    &Identifier::vanilla_static("normal")
                )
                .unwrap_or_else(|error| panic!("{key:?} should resolve: {error}")),
                key
            );
        }
    }

    #[test]
    fn custom_preset_settings_select_matching_generator() {
        use std::panic;
        use std::thread;

        // `caves` and `floating_islands` are not referenced by any vanilla world preset,
        // so this drives the preset constructor directly to cover their dispatch arms.
        let result = thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(custom_preset_settings_select_matching_generator_inner)
            .expect("Failed to spawn test thread")
            .join();

        if let Err(payload) = result {
            panic::resume_unwind(payload);
        }
    }

    fn custom_preset_settings_select_matching_generator_inner() {
        init_vanilla_registry();
        init_behaviors();
        let thread_pool = Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(1)
                .build()
                .expect("Cannot create a thread pool."),
        );

        let cases = [
            (
                NoiseSettingsKey::Caves,
                CavesNoiseSettings::MIN_Y,
                CavesNoiseSettings::HEIGHT,
                CavesNoiseSettings::SEA_LEVEL,
            ),
            (
                NoiseSettingsKey::FloatingIslands,
                FloatingIslandsNoiseSettings::MIN_Y,
                FloatingIslandsNoiseSettings::HEIGHT,
                FloatingIslandsNoiseSettings::SEA_LEVEL,
            ),
        ];

        let air = REGISTRY.blocks.get_default_state_id(&vanilla_blocks::AIR);
        for (settings, min_y, height, sea_level) in cases {
            let config = WorldGeneratorConfigData::Preset(ResolvedPresetConfig {
                preset: Identifier::vanilla_static("custom"),
                dimension: Identifier::vanilla_static("overworld"),
                dimension_type: Identifier::vanilla_static("overworld"),
                generator: PresetDimensionGenerator::Noise {
                    biome_source: PresetNoiseBiomeSource::Fixed(Identifier::vanilla_static(
                        "plains",
                    )),
                    settings,
                },
            });
            let output = create_preset(None, &config, 0, &thread_pool)
                .unwrap_or_else(|error| panic!("{settings:?} preset should create: {error}"));

            match (&output.generator, settings) {
                (ChunkGeneratorType::Caves(_), NoiseSettingsKey::Caves)
                | (ChunkGeneratorType::FloatingIslands(_), NoiseSettingsKey::FloatingIslands) => {}
                _ => panic!("{settings:?} selected the wrong chunk generator"),
            }
            assert_eq!(output.generator.min_y(), min_y, "{settings:?} min_y");
            assert_eq!(output.generator.gen_depth(), height, "{settings:?} height");
            assert_eq!(output.sea_level, sea_level, "{settings:?} sea level");

            let sections = (0..height / 16)
                .map(|_| ChunkSection::new_empty())
                .collect::<Vec<_>>()
                .into_boxed_slice();
            let chunk = Chunk::new(
                Sections::from_owned(sections),
                steel_utils::ChunkPos::new(0, 0),
                min_y,
                height,
                Weak::new(),
            );
            output.generator.create_biomes(&chunk);
            output
                .generator
                .fill_from_noise(GenerationChunk::<TerrainPhase>::for_test(&chunk), None);
            assert!(
                (0..chunk.height() as usize).any(|relative_y| {
                    (0..16).any(|z| {
                        (0..16).any(|x| {
                            chunk
                                .get_relative_block(x, relative_y, z)
                                .is_some_and(|state| state != air)
                        })
                    })
                }),
                "{settings:?} should generate solid terrain"
            );
        }
    }
}
