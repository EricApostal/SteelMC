//! Build-time generation for vanilla registry tags.

mod block;
mod common;
mod fluid;
mod item;

use proc_macro2::TokenStream;

pub(crate) use block::build as block;
pub(crate) use fluid::build as fluid;
pub(crate) use item::build as item;

macro_rules! simple_tag_builder {
    ($function:ident, $tag_subdir:literal, $registry_module:literal, $registry_type:literal) => {
        pub(crate) fn $function() -> TokenStream {
            common::build_simple_tags($tag_subdir, $registry_module, $registry_type)
        }
    };
}

/// Like [`simple_tag_builder`], but names the generated tag struct after the
/// vanilla tag holder instead of deriving it from the registry module.
macro_rules! simple_tag_builder_named {
    (
        $function:ident,
        $tag_subdir:literal,
        $registry_module:literal,
        $registry_type:literal,
        $tag_struct:literal
    ) => {
        pub(crate) fn $function() -> TokenStream {
            common::build_simple_tags_with_name(
                $tag_subdir,
                $registry_module,
                $registry_type,
                Some($tag_struct),
            )
        }
    };
}

simple_tag_builder_named!(
    game_event,
    "game_event",
    "game_events",
    "GameEventRegistry",
    "GameEventTag"
);
// Vanilla 26.3 renamed the configured-feature registry to `worldgen/feature`
// (`Registries.FEATURE`); its tag directory follows the vanilla name while the
// Steel registry module keeps the historical `feature` name.
simple_tag_builder_named!(
    feature,
    "worldgen/feature",
    "feature",
    "ConfiguredFeatureRegistry",
    "FeatureTag"
);
simple_tag_builder_named!(
    flat_level_generator_preset,
    "worldgen/flat_level_generator_preset",
    "flat_level_generator_preset",
    "FlatLevelGeneratorPresetRegistry",
    "FlatLevelGeneratorPresetTag"
);
simple_tag_builder_named!(
    world_preset,
    "worldgen/world_preset",
    "world_preset",
    "WorldPresetRegistry",
    "WorldPresetTag"
);

simple_tag_builder!(
    banner_pattern,
    "banner_pattern",
    "banner_pattern",
    "BannerPatternRegistry"
);
simple_tag_builder!(biome, "worldgen/biome", "biome", "BiomeRegistry");
simple_tag_builder!(
    damage_type,
    "damage_type",
    "damage_type",
    "DamageTypeRegistry"
);
simple_tag_builder!(dialog, "dialog", "dialog", "DialogRegistry");
simple_tag_builder!(
    enchantment,
    "enchantment",
    "enchantment",
    "EnchantmentRegistry"
);
simple_tag_builder!(
    entity_type,
    "entity_type",
    "entity_type",
    "EntityTypeRegistry"
);
simple_tag_builder!(instrument, "instrument", "instrument", "InstrumentRegistry");
simple_tag_builder!(
    painting_variant,
    "painting_variant",
    "painting_variant",
    "PaintingVariantRegistry"
);
simple_tag_builder!(poi_type, "point_of_interest_type", "poi", "PoiTypeRegistry");
simple_tag_builder!(potion, "potion", "potion", "PotionRegistry");
simple_tag_builder!(
    structure,
    "worldgen/structure",
    "structure",
    "StructureRegistry"
);
simple_tag_builder!(timeline, "timeline", "timeline", "TimelineRegistry");
