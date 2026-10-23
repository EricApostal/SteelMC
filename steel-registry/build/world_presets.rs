#![expect(
    clippy::unwrap_used,
    reason = "build script must fail immediately on invalid extracted world preset data"
)]

use std::collections::BTreeMap;
use std::fs;

use heck::ToShoutySnakeCase;
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use serde::Deserialize;

use crate::flat_level_generator_presets::{
    FlatLevelGeneratorSettingsJson, generate_identifier, generate_settings,
};

#[derive(Deserialize, Debug)]
struct WorldPresetJson {
    dimensions: BTreeMap<String, LevelStemJson>,
}

#[derive(Deserialize, Debug)]
struct LevelStemJson {
    #[serde(rename = "type")]
    dimension_type: String,
    generator: GeneratorJson,
}

#[derive(Deserialize, Debug)]
struct GeneratorJson {
    #[serde(rename = "type")]
    generator_type: String,
    #[serde(default)]
    biome_source: Option<BiomeSourceJson>,
    #[serde(default)]
    settings: Option<serde_json::Value>,
}

#[derive(Deserialize, Debug)]
struct BiomeSourceJson {
    #[serde(rename = "type")]
    source_type: String,
    #[serde(default)]
    preset: Option<String>,
    #[serde(default)]
    biome: Option<String>,
}

/// Vanilla `WorldDimensions.keysInOrder`: overworld, then nether, then end, then anything else.
/// Used so the generated dimension list matches vanilla iteration order deterministically.
fn dimension_rank(key: &str) -> usize {
    match key {
        "minecraft:overworld" => 0,
        "minecraft:the_nether" => 1,
        "minecraft:the_end" => 2,
        _ => 3,
    }
}

fn generate_biome_source(preset_name: &str, source: &BiomeSourceJson) -> TokenStream {
    let source_type = source
        .source_type
        .strip_prefix("minecraft:")
        .unwrap_or(&source.source_type);

    match source_type {
        "multi_noise" => {
            let preset = source.preset.as_deref().unwrap_or_else(|| {
                panic!("{preset_name}: multi_noise biome source is missing a preset")
            });
            let preset = generate_identifier(preset);
            quote! { PresetBiomeSource::MultiNoise { preset: #preset } }
        }
        "fixed" => {
            let biome = source
                .biome
                .as_deref()
                .unwrap_or_else(|| panic!("{preset_name}: fixed biome source is missing a biome"));
            let biome = generate_identifier(biome);
            quote! { PresetBiomeSource::Fixed { biome: #biome } }
        }
        "the_end" => quote! { PresetBiomeSource::TheEnd },
        other => panic!("{preset_name}: unknown biome source type {other}"),
    }
}

fn generate_generator(preset_name: &str, generator: &GeneratorJson) -> TokenStream {
    let generator_type = generator
        .generator_type
        .strip_prefix("minecraft:")
        .unwrap_or(&generator.generator_type);

    match generator_type {
        "noise" => {
            let biome_source = generator.biome_source.as_ref().unwrap_or_else(|| {
                panic!("{preset_name}: noise generator is missing a biome source")
            });
            let settings = generator
                .settings
                .as_ref()
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| {
                    panic!("{preset_name}: noise generator is missing noise settings")
                });

            let biome_source = generate_biome_source(preset_name, biome_source);
            let settings = generate_identifier(settings);

            quote! {
                WorldPresetGenerator::Noise {
                    biome_source: #biome_source,
                    settings: #settings,
                }
            }
        }
        "flat" => {
            let settings = generator
                .settings
                .as_ref()
                .unwrap_or_else(|| panic!("{preset_name}: flat generator is missing settings"));
            let settings: FlatLevelGeneratorSettingsJson = serde_json::from_value(settings.clone())
                .unwrap_or_else(|e| {
                    panic!("{preset_name}: failed to parse flat generator settings: {e}")
                });
            let settings = generate_settings(&settings);

            quote! { WorldPresetGenerator::Flat { settings: #settings } }
        }
        "debug" => quote! { WorldPresetGenerator::Debug },
        other => panic!("{preset_name}: unknown world preset generator type {other}"),
    }
}

pub(crate) fn build() -> TokenStream {
    let preset_dir =
        "../steel-utils/build_assets/builtin_datapacks/minecraft/worldgen/world_preset";
    println!("cargo:rerun-if-changed={preset_dir}");

    let mut presets = Vec::new();
    for entry in fs::read_dir(preset_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }

        let preset_name = path.file_stem().unwrap().to_str().unwrap().to_string();
        let content = fs::read_to_string(&path).unwrap();
        let preset: WorldPresetJson = serde_json::from_str(&content)
            .unwrap_or_else(|e| panic!("Failed to parse world preset {preset_name}: {e}"));

        presets.push((preset_name, preset));
    }

    // Deterministic output order regardless of filesystem iteration order.
    presets.sort_by(|(a, _), (b, _)| a.cmp(b));

    let mut stream = TokenStream::new();

    stream.extend(quote! {
        use std::sync::LazyLock;

        use crate::flat_level_generator_preset::{FlatLayerInfo, FlatLevelGeneratorSettings};
        use crate::world_preset::{
            PresetBiomeSource, WorldPreset, WorldPresetDimension, WorldPresetGenerator,
            WorldPresetRegistry,
        };
        use steel_utils::Identifier;
    });

    let mut register_stream = TokenStream::new();
    for (preset_name, preset) in &presets {
        let preset_ident = Ident::new(&preset_name.to_shouty_snake_case(), Span::call_site());
        let key = generate_identifier(preset_name);

        let mut dimensions: Vec<_> = preset.dimensions.iter().collect();
        // Stable sort: non-builtin dimensions keep the sorted `BTreeMap` order.
        dimensions.sort_by_key(|(a, _)| dimension_rank(a.as_str()));

        let dimensions = dimensions
            .into_iter()
            .map(|(dimension_key, level_stem)| {
                let dimension = generate_identifier(dimension_key);
                let dimension_type = generate_identifier(&level_stem.dimension_type);
                let generator = generate_generator(preset_name, &level_stem.generator);

                quote! {
                    WorldPresetDimension {
                        dimension: #dimension,
                        dimension_type: #dimension_type,
                        generator: #generator,
                    }
                }
            })
            .collect::<Vec<_>>();

        stream.extend(quote! {
            pub static #preset_ident: LazyLock<WorldPreset> = LazyLock::new(|| WorldPreset {
                key: #key,
                dimensions: vec![#(#dimensions),*],
            });
        });

        register_stream.extend(quote! {
            registry.register(&#preset_ident);
        });
    }

    stream.extend(quote! {
        pub fn register_world_presets(registry: &mut WorldPresetRegistry) {
            #register_stream
        }
    });

    stream
}
