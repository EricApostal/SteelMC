#![expect(
    clippy::unwrap_used,
    reason = "build script must fail immediately on invalid extracted flat level generator preset data"
)]

use std::fs;

use heck::ToShoutySnakeCase;
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use serde::Deserialize;

/// Superflat settings as extracted from a data pack, shared between the flat level generator
/// presets and the inline `flat` generators inside world presets.
#[derive(Deserialize, Debug)]
pub(crate) struct FlatLevelGeneratorSettingsJson {
    biome: String,
    #[serde(default)]
    features: bool,
    #[serde(default)]
    lakes: bool,
    layers: Vec<FlatLayerJson>,
    #[serde(default)]
    structure_overrides: Option<StructureOverridesJson>,
}

/// One superflat layer as extracted from a data pack.
#[derive(Deserialize, Debug)]
pub(crate) struct FlatLayerJson {
    block: String,
    height: u32,
}

/// Vanilla serialises `structure_overrides` either as a single structure-set id or a list of
/// them, so both shapes are accepted and normalised to a list.
#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub(crate) enum StructureOverridesJson {
    Single(String),
    Multiple(Vec<String>),
}

#[derive(Deserialize, Debug)]
struct FlatLevelGeneratorPresetJson {
    display: String,
    settings: FlatLevelGeneratorSettingsJson,
}

/// Emits an `Identifier` literal for a resource location, dropping the `minecraft:` namespace
/// prefix that vanilla data packs always write.
pub(crate) fn generate_identifier(resource: &str) -> TokenStream {
    let path = resource.strip_prefix("minecraft:").unwrap_or(resource);
    quote! { Identifier::vanilla_static(#path) }
}

/// Emits a `FlatLevelGeneratorSettings` value from extracted JSON.
pub(crate) fn generate_settings(settings: &FlatLevelGeneratorSettingsJson) -> TokenStream {
    let biome = generate_identifier(&settings.biome);

    let layers = settings
        .layers
        .iter()
        .map(|layer| {
            let block = generate_identifier(&layer.block);
            let height = layer.height;
            quote! { FlatLayerInfo { block: #block, height: #height } }
        })
        .collect::<Vec<_>>();

    // Source order is kept: layer order is semantic and structure overrides are a set.
    let structure_overrides = match settings.structure_overrides.as_ref() {
        Some(StructureOverridesJson::Single(resource)) => vec![generate_identifier(resource)],
        Some(StructureOverridesJson::Multiple(resources)) => resources
            .iter()
            .map(|resource| generate_identifier(resource))
            .collect(),
        None => Vec::new(),
    };

    let decoration = settings.features;
    let add_lakes = settings.lakes;

    quote! {
        FlatLevelGeneratorSettings {
            biome: #biome,
            layers: vec![#(#layers),*],
            structure_overrides: vec![#(#structure_overrides),*],
            decoration: #decoration,
            add_lakes: #add_lakes,
        }
    }
}

pub(crate) fn build() -> TokenStream {
    let preset_dir = "../steel-utils/build_assets/builtin_datapacks/minecraft/worldgen/flat_level_generator_preset";
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
        let preset: FlatLevelGeneratorPresetJson =
            serde_json::from_str(&content).unwrap_or_else(|e| {
                panic!("Failed to parse flat level generator preset {preset_name}: {e}")
            });

        presets.push((preset_name, preset));
    }

    // Deterministic output order regardless of filesystem iteration order.
    presets.sort_by(|(a, _), (b, _)| a.cmp(b));

    let mut stream = TokenStream::new();

    stream.extend(quote! {
        use std::sync::LazyLock;

        use crate::flat_level_generator_preset::{
            FlatLayerInfo, FlatLevelGeneratorPreset, FlatLevelGeneratorPresetRegistry,
            FlatLevelGeneratorSettings,
        };
        use steel_utils::Identifier;
    });

    let mut register_stream = TokenStream::new();
    for (preset_name, preset) in &presets {
        let preset_ident = Ident::new(&preset_name.to_shouty_snake_case(), Span::call_site());
        let key = generate_identifier(preset_name);
        let display = generate_identifier(&preset.display);
        let settings = generate_settings(&preset.settings);

        stream.extend(quote! {
            pub static #preset_ident: LazyLock<FlatLevelGeneratorPreset> =
                LazyLock::new(|| FlatLevelGeneratorPreset {
                    key: #key,
                    display: #display,
                    settings: #settings,
                });
        });

        register_stream.extend(quote! {
            registry.register(&#preset_ident);
        });
    }

    stream.extend(quote! {
        pub fn register_flat_level_generator_presets(
            registry: &mut FlatLevelGeneratorPresetRegistry,
        ) {
            #register_stream
        }
    });

    stream
}
