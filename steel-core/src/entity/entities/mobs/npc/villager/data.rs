//! Vanilla `VillagerData`, `VillagerDataHolder` and `VillagerType` transforms.

use std::sync::LazyLock;

use steel_registry::biome::BiomeRef;
use steel_registry::entity_data::VillagerData;
use steel_registry::vanilla_biomes;
use steel_registry::vanilla_villager_professions;
use steel_registry::vanilla_villager_types;
use steel_registry::villager_profession::VillagerProfessionRef;
use steel_registry::villager_type::VillagerTypeRef;
use steel_registry::{REGISTRY, RegistryExt};

use simdnbt::borrow::NbtCompound as NbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtTag};

use steel_utils::{BlockPos, Identifier};

use crate::world::World;

/// Vanilla `VillagerData.DEFAULT_TYPE` (`minecraft:plains`).
pub const DEFAULT_VILLAGER_TYPE: VillagerTypeRef = &vanilla_villager_types::PLAINS;

/// Vanilla `VillagerData.MIN_VILLAGER_LEVEL`.
pub const MIN_VILLAGER_LEVEL: i32 = 1;
/// Vanilla `VillagerData.MAX_VILLAGER_LEVEL`.
pub const MAX_VILLAGER_LEVEL: i32 = 5;

/// Vanilla `VillagerData.NEXT_LEVEL_XP_THRESHOLDS`.
const NEXT_LEVEL_XP_THRESHOLDS: [i32; 5] = [0, 10, 70, 150, 250];

/// Vanilla `VillagerData.canLevelUp`.
#[must_use]
pub const fn can_level_up(current_level: i32) -> bool {
    current_level >= MIN_VILLAGER_LEVEL && current_level < MAX_VILLAGER_LEVEL
}

/// Vanilla `VillagerData.getMaxXpPerLevel`.
#[must_use]
pub const fn max_xp_per_level(level: i32) -> i32 {
    if can_level_up(level) {
        NEXT_LEVEL_XP_THRESHOLDS[level as usize]
    } else {
        0
    }
}

/// Vanilla `VillagerDataHolder`: the holder-specific half of `VillagerData`.
///
/// Steel's [`VillagerData`] is a plain registry-index record owned by
/// `steel-registry`, so vanilla's `withType`/`withProfession`/`withLevel` live on
/// [`VillagerDataExt`] here instead of on the record itself.
pub trait VillagerDataHolder {
    /// Vanilla `VillagerDataHolder.getVillagerData`.
    fn villager_data(&self) -> VillagerData;

    /// Vanilla `VillagerDataHolder.setVillagerData`.
    fn set_villager_data(&self, data: VillagerData);

    /// Vanilla `VillagerDataHolder.getVillagerDataFinalized`.
    fn villager_data_finalized(&self) -> bool;

    /// Vanilla `VillagerDataHolder.setVillagerDataFinalized`.
    fn set_villager_data_finalized(&self, villager_data_finalized: bool);

    /// Vanilla `VillagerDataHolder.finalizeVillagerType`: assigns the biome's
    /// villager type once, on the first spawn of a villager.
    fn finalize_villager_type(&self, world: &World, pos: BlockPos) {
        if self.villager_data_finalized() {
            return;
        }

        let villager_type = world
            .biome_at(pos)
            .map_or(DEFAULT_VILLAGER_TYPE, villager_type_by_biome);
        self.set_villager_data(self.villager_data().with_type_ref(villager_type));
        self.set_villager_data_finalized(true);
    }
}
/// Vanilla `VillagerData.withType`/`withProfession`/`withLevel`.
pub trait VillagerDataExt {
    /// Vanilla `VillagerData.withType(Holder<VillagerType>)`.
    fn with_type_ref(self, villager_type: VillagerTypeRef) -> Self;

    /// Vanilla `VillagerData.withProfession(Holder<VillagerProfession>)`.
    fn with_profession_ref(self, profession: VillagerProfessionRef) -> Self;

    /// Vanilla `VillagerData.withLevel`.
    fn with_level(self, level: i32) -> Self;
}

impl VillagerDataExt for VillagerData {
    fn with_type_ref(self, villager_type: VillagerTypeRef) -> Self {
        Self::new(
            villager_type_index(villager_type),
            self.profession,
            self.level,
        )
    }

    fn with_profession_ref(self, profession: VillagerProfessionRef) -> Self {
        Self::new(
            self.villager_type,
            villager_profession_index(profession),
            self.level,
        )
    }

    fn with_level(self, level: i32) -> Self {
        Self::new(
            self.villager_type,
            self.profession,
            level.max(MIN_VILLAGER_LEVEL),
        )
    }
}

/// Vanilla `VillagerType.byBiome`.
///
/// Biome entries are `LazyLock` statics, so the table is built on first use and the
/// borrowed keys stay `'static`.
#[expect(
    clippy::too_many_lines,
    reason = "vanilla `VillagerType.byBiome` is one flat biome-to-type table"
)]
fn by_biome() -> &'static [(&'static Identifier, VillagerTypeRef)] {
    static BY_BIOME: LazyLock<Vec<(&'static Identifier, VillagerTypeRef)>> = LazyLock::new(|| {
        vec![
            (
                &vanilla_biomes::BADLANDS.key,
                &vanilla_villager_types::DESERT,
            ),
            (&vanilla_biomes::DESERT.key, &vanilla_villager_types::DESERT),
            (
                &vanilla_biomes::ERODED_BADLANDS.key,
                &vanilla_villager_types::DESERT,
            ),
            (
                &vanilla_biomes::WOODED_BADLANDS.key,
                &vanilla_villager_types::DESERT,
            ),
            (
                &vanilla_biomes::BAMBOO_JUNGLE.key,
                &vanilla_villager_types::JUNGLE,
            ),
            (&vanilla_biomes::JUNGLE.key, &vanilla_villager_types::JUNGLE),
            (
                &vanilla_biomes::SPARSE_JUNGLE.key,
                &vanilla_villager_types::JUNGLE,
            ),
            (
                &vanilla_biomes::SAVANNA_PLATEAU.key,
                &vanilla_villager_types::SAVANNA,
            ),
            (
                &vanilla_biomes::SAVANNA.key,
                &vanilla_villager_types::SAVANNA,
            ),
            (
                &vanilla_biomes::WINDSWEPT_SAVANNA.key,
                &vanilla_villager_types::SAVANNA,
            ),
            (
                &vanilla_biomes::DEEP_FROZEN_OCEAN.key,
                &vanilla_villager_types::SNOW,
            ),
            (
                &vanilla_biomes::FROZEN_OCEAN.key,
                &vanilla_villager_types::SNOW,
            ),
            (
                &vanilla_biomes::FROZEN_RIVER.key,
                &vanilla_villager_types::SNOW,
            ),
            (
                &vanilla_biomes::ICE_SPIKES.key,
                &vanilla_villager_types::SNOW,
            ),
            (
                &vanilla_biomes::SNOWY_BEACH.key,
                &vanilla_villager_types::SNOW,
            ),
            (
                &vanilla_biomes::SNOWY_TAIGA.key,
                &vanilla_villager_types::SNOW,
            ),
            (
                &vanilla_biomes::SNOWY_PLAINS.key,
                &vanilla_villager_types::SNOW,
            ),
            (&vanilla_biomes::GROVE.key, &vanilla_villager_types::SNOW),
            (
                &vanilla_biomes::SNOWY_SLOPES.key,
                &vanilla_villager_types::SNOW,
            ),
            (
                &vanilla_biomes::FROZEN_PEAKS.key,
                &vanilla_villager_types::SNOW,
            ),
            (
                &vanilla_biomes::JAGGED_PEAKS.key,
                &vanilla_villager_types::SNOW,
            ),
            (&vanilla_biomes::SWAMP.key, &vanilla_villager_types::SWAMP),
            (
                &vanilla_biomes::MANGROVE_SWAMP.key,
                &vanilla_villager_types::SWAMP,
            ),
            (
                &vanilla_biomes::OLD_GROWTH_SPRUCE_TAIGA.key,
                &vanilla_villager_types::TAIGA,
            ),
            (
                &vanilla_biomes::OLD_GROWTH_PINE_TAIGA.key,
                &vanilla_villager_types::TAIGA,
            ),
            (
                &vanilla_biomes::WINDSWEPT_GRAVELLY_HILLS.key,
                &vanilla_villager_types::TAIGA,
            ),
            (
                &vanilla_biomes::WINDSWEPT_HILLS.key,
                &vanilla_villager_types::TAIGA,
            ),
            (&vanilla_biomes::TAIGA.key, &vanilla_villager_types::TAIGA),
            (
                &vanilla_biomes::WINDSWEPT_FOREST.key,
                &vanilla_villager_types::TAIGA,
            ),
        ]
    });

    &BY_BIOME
}

/// Vanilla `VillagerType.byBiome`.
#[must_use]
pub fn villager_type_by_biome(biome: BiomeRef) -> VillagerTypeRef {
    by_biome()
        .iter()
        .find(|(key, _)| **key == biome.key)
        .map_or(DEFAULT_VILLAGER_TYPE, |(_, villager_type)| *villager_type)
}

/// Registry index of a villager type, falling back to the default type's index.
#[must_use]
pub fn villager_type_index(villager_type: VillagerTypeRef) -> i32 {
    REGISTRY
        .villager_types
        .id_from_key(&villager_type.key)
        .map_or_else(default_villager_type_index, |id| id as i32)
}

/// Registry index of the default villager type.
#[must_use]
pub fn default_villager_type_index() -> i32 {
    REGISTRY
        .villager_types
        .id_from_key(&DEFAULT_VILLAGER_TYPE.key)
        .map_or(0, |id| id as i32)
}

/// Registry index of a villager profession, falling back to `minecraft:none`.
#[must_use]
pub fn villager_profession_index(profession: VillagerProfessionRef) -> i32 {
    REGISTRY
        .villager_professions
        .id_from_key(&profession.key)
        .map_or_else(none_profession_index, |id| id as i32)
}

/// Vanilla `VillagerProfession.NONE`.
#[must_use]
pub fn none_profession() -> VillagerProfessionRef {
    &vanilla_villager_professions::NONE
}

/// Registry index of `minecraft:none`.
#[must_use]
pub fn none_profession_index() -> i32 {
    REGISTRY
        .villager_professions
        .id_from_key(&vanilla_villager_professions::NONE.key)
        .map_or(0, |id| id as i32)
}

/// Villager type behind a `VillagerData` registry index.
///
/// Unknown indices (e.g. a save from a datapack that no longer exists) fall back
/// to the default type instead of failing.
#[must_use]
pub fn villager_type_from_index(index: i32) -> VillagerTypeRef {
    usize::try_from(index)
        .ok()
        .and_then(|index| REGISTRY.villager_types.by_id(index))
        .unwrap_or(DEFAULT_VILLAGER_TYPE)
}

/// Villager profession behind a `VillagerData` registry index.
#[must_use]
pub fn profession_from_index(index: i32) -> VillagerProfessionRef {
    usize::try_from(index)
        .ok()
        .and_then(|index| REGISTRY.villager_professions.by_id(index))
        .unwrap_or_else(none_profession)
}

/// Vanilla `Villager.createDefaultVillagerData`.
#[must_use]
pub fn create_default_villager_data() -> VillagerData {
    VillagerData::new(
        default_villager_type_index(),
        none_profession_index(),
        MIN_VILLAGER_LEVEL,
    )
}

/// Writes vanilla `VillagerData.CODEC` (`{type, profession, level}`).
pub fn write_villager_data(data: &VillagerData, nbt: &mut NbtCompound) {
    let mut compound = NbtCompound::new();
    compound.insert(
        "type",
        villager_type_from_index(data.villager_type).key.to_string(),
    );
    compound.insert(
        "profession",
        profession_from_index(data.profession).key.to_string(),
    );
    compound.insert("level", data.level);
    nbt.insert("VillagerData", NbtTag::Compound(compound));
}

/// Reads vanilla `VillagerData.CODEC`; the type/profession default exactly like the
/// codec's `orElseGet`.
#[must_use]
pub fn read_villager_data(nbt: NbtCompoundView<'_, '_>) -> Option<VillagerData> {
    let compound = nbt.compound("VillagerData")?;
    let villager_type = compound
        .string("type")
        .and_then(|key| identifier_from_nbt(key.to_str().as_ref()))
        .and_then(|key| REGISTRY.villager_types.by_key(&key))
        .unwrap_or(DEFAULT_VILLAGER_TYPE);
    let profession = compound
        .string("profession")
        .and_then(|key| identifier_from_nbt(key.to_str().as_ref()))
        .and_then(|key| REGISTRY.villager_professions.by_key(&key))
        .unwrap_or_else(none_profession);

    Some(VillagerData::new(
        villager_type_index(villager_type),
        villager_profession_index(profession),
        compound.int("level").unwrap_or(MIN_VILLAGER_LEVEL),
    ))
}

/// Parses a namespaced identifier out of saved NBT.
fn identifier_from_nbt(value: &str) -> Option<Identifier> {
    match value.split_once(':') {
        Some((namespace, path)) => Some(Identifier::new(namespace.to_owned(), path.to_owned())),
        None if value.is_empty() => None,
        None => Some(Identifier::vanilla(value.to_owned())),
    }
}
