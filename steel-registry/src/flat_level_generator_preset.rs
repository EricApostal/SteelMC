use rustc_hash::FxHashMap;
use steel_utils::Identifier;

/// One layer of a superflat preset, mirroring vanilla `FlatLayerInfo`.
///
/// Vanilla applies the layer's block state with its default properties; Steel keeps the
/// underlying block by key so the block registries stay the source of truth.
#[derive(Debug)]
pub struct FlatLayerInfo {
    /// Block placed by this layer.
    pub block: Identifier,
    /// Number of blocks this layer is tall.
    pub height: u32,
}

/// Superflat generation settings, mirroring vanilla `FlatLevelGeneratorSettings`.
///
/// Vanilla keeps `structure_overrides` as an optional holder set and looks the biome up as an
/// optional holder that falls back to plains. The extracted vanilla data always names a biome,
/// so the biome is modelled directly; a missing `structure_overrides` is normalised to an empty
/// list, and `lakes`/`features` are modelled by their resolved flags (`add_lakes`/`decoration`).
#[derive(Debug)]
pub struct FlatLevelGeneratorSettings {
    /// Biome generated across the flat layers.
    pub biome: Identifier,
    /// Layers ordered bottom to top.
    pub layers: Vec<FlatLayerInfo>,
    /// Structure sets allowed to generate in the flat world.
    pub structure_overrides: Vec<Identifier>,
    /// Whether features generate, from vanilla's `features` flag.
    pub decoration: bool,
    /// Whether lava lakes generate, from vanilla's `lakes` flag.
    pub add_lakes: bool,
}

/// Represents a `flat_level_generator_preset` definition from a data pack JSON file.
#[derive(Debug)]
pub struct FlatLevelGeneratorPreset {
    pub key: Identifier,
    /// Item shown for this preset in the superflat preset selection screen.
    pub display: Identifier,
    pub settings: FlatLevelGeneratorSettings,
}

pub type FlatLevelGeneratorPresetRef = &'static FlatLevelGeneratorPreset;

pub struct FlatLevelGeneratorPresetRegistry {
    tags: crate::RegistryTags,
    flat_level_generator_presets_by_id: Vec<FlatLevelGeneratorPresetRef>,
    flat_level_generator_presets_by_key: FxHashMap<Identifier, usize>,
    allows_registering: bool,
}

impl FlatLevelGeneratorPresetRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            flat_level_generator_presets_by_id: Vec::new(),
            flat_level_generator_presets_by_key: FxHashMap::default(),
            allows_registering: true,
            tags: crate::RegistryTags::default(),
        }
    }
}

crate::impl_standard_methods!(
    FlatLevelGeneratorPresetRegistry,
    FlatLevelGeneratorPresetRef,
    flat_level_generator_presets_by_id,
    flat_level_generator_presets_by_key,
    allows_registering
);

crate::impl_registry!(
    FlatLevelGeneratorPresetRegistry,
    FlatLevelGeneratorPreset,
    flat_level_generator_presets_by_id,
    flat_level_generator_presets_by_key,
    flat_level_generator_presets
);
crate::impl_tagged_registry!(
    FlatLevelGeneratorPresetRegistry,
    flat_level_generator_presets_by_key,
    "preset"
);
