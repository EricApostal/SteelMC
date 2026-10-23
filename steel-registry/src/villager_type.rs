use rustc_hash::FxHashMap;
use steel_utils::Identifier;

use crate::RegistryTags;
use crate::biome::BiomeRef;
use crate::vanilla_villager_types;

#[derive(Debug)]
pub struct VillagerType {
    pub key: Identifier,
}

pub type VillagerTypeRef = &'static VillagerType;

impl VillagerType {
    /// Vanilla `VillagerType.byBiome`: maps a biome to the villager type it
    /// spawns, defaulting to `plains` for biomes outside the map.
    #[must_use]
    pub fn by_biome(biome: BiomeRef) -> VillagerTypeRef {
        let villager_type = match biome.key.path.as_ref() {
            "badlands" | "desert" | "eroded_badlands" | "wooded_badlands" => {
                &vanilla_villager_types::DESERT
            }
            "bamboo_jungle" | "jungle" | "sparse_jungle" => &vanilla_villager_types::JUNGLE,
            "savanna_plateau" | "savanna" | "windswept_savanna" => &vanilla_villager_types::SAVANNA,
            "deep_frozen_ocean" | "frozen_ocean" | "frozen_river" | "ice_spikes"
            | "snowy_beach" | "snowy_taiga" | "snowy_plains" | "grove" | "snowy_slopes"
            | "frozen_peaks" | "jagged_peaks" => &vanilla_villager_types::SNOW,
            "swamp" | "mangrove_swamp" => &vanilla_villager_types::SWAMP,
            "old_growth_spruce_taiga"
            | "old_growth_pine_taiga"
            | "windswept_gravelly_hills"
            | "windswept_hills"
            | "taiga"
            | "windswept_forest" => &vanilla_villager_types::TAIGA,
            _ => &vanilla_villager_types::PLAINS,
        };
        villager_type
    }
}

pub struct VillagerTypeRegistry {
    villager_types_by_id: Vec<VillagerTypeRef>,
    villager_types_by_key: FxHashMap<Identifier, usize>,
    tags: RegistryTags,
    allows_registering: bool,
}

impl VillagerTypeRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            villager_types_by_id: Vec::new(),
            villager_types_by_key: FxHashMap::default(),
            tags: RegistryTags::default(),
            allows_registering: true,
        }
    }
}

crate::impl_standard_methods!(
    VillagerTypeRegistry,
    VillagerTypeRef,
    villager_types_by_id,
    villager_types_by_key,
    allows_registering
);

crate::impl_registry!(
    VillagerTypeRegistry,
    VillagerType,
    villager_types_by_id,
    villager_types_by_key,
    villager_types
);
crate::impl_tagged_registry!(VillagerTypeRegistry, villager_types_by_key, "villager type");
