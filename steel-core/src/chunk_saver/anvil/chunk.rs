//! Vanilla chunk NBT ⇄ `PersistentChunk`.
//!
//! The mapping follows vanilla `SerializableChunkData` exactly:
//!
//! * `Status` is the chunk-status registry key, so the mapping is lossless name-for-name.
//! * `sections[]` carry `Y`, `block_states`, `biomes`, `BlockLight` and `SkyLight`; light is
//!   indexed by section Y, so light sections one below/above the world range get their own tag.
//! * Palette containers use vanilla's per-container on-disk width (`4..=8` for block states up
//!   to 256 entries, `ceil_log2` beyond that, and `ceil_log2` for biomes); Steel re-packs them
//!   into its power-of-two widths.
//! * `Heightmaps` are named `long[]` values relative to `min_y`, packed with
//!   `ceil_log2(height + 1)` bits — identical to Steel's own packing.
//! * `entities` is written inline only for protochunks; full chunks use `entities/*.mca`.
//! * `structures` carries `References` and `starts`; the start/piece schema is vanilla's
//!   `StructureStart.createTag`/`StructurePiece.createTag` (see the section comment further
//!   down for the per-family keys).
//!
//! Deliberately dropped with a warning (Steel has no model for them, and inventing values
//! would be worse than losing them): `UpgradeData`, `blending_data`, `below_zero_retrogen`,
//! and `InhabitedTime`.

use std::collections::BTreeMap;
use std::io;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};

use simdnbt::borrow::NbtCompound as BorrowedNbtCompound;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::{
    REGISTRY, RegistryEntry as _, RegistryExt as _, vanilla_biomes, vanilla_blocks,
};
use steel_utils::version::DATA_VERSION;
use steel_utils::{BlockStateId, ChunkPos, Identifier, PackedChunkPos};

use crate::chunk::heightmap::HeightmapType;
use crate::chunk::light::DATA_LAYER_SIZE;
use crate::chunk::status::ChunkStatus;
use crate::chunk_saver::bit_pack::{bits_for_palette_len, pack_indices_from_iter, unpack_indices};
use crate::chunk_saver::format::{
    PersistentBiomeData, PersistentBlockEntity, PersistentBlockState, PersistentBoundingBox,
    PersistentChunk, PersistentDesertPyramidPieceData, PersistentEntity, PersistentHeightmap,
    PersistentJigsawJunction, PersistentJigsawPieceData, PersistentJungleTemplePieceData,
    PersistentLightData, PersistentLightSection, PersistentMineshaftPieceData,
    PersistentMineshaftPieceKind, PersistentNetherFortressPieceData,
    PersistentOceanMonumentPieceData, PersistentPoi, PersistentPoolElement,
    PersistentProceduralPieceData, PersistentProcessorList, PersistentSection,
    PersistentStrongholdPieceData, PersistentStrongholdSmallDoorType, PersistentStructurePiece,
    PersistentStructurePiecePayload, PersistentStructureReference, PersistentStructureStart,
    PersistentSwampHutPieceData, PersistentTemplatePieceData,
    PersistentTemplatePlacementAdjustment, PersistentTemplateProcessorList, PersistentTick,
};
use crate::datafixer::{DataFixType, ensure_current, version_of_borrowed};
use crate::worldgen::template::StructureTemplate;

use super::entities::{entity_from_nbt, entity_to_nbt};
use super::nbt::{compound_list, read_raw_nbt, write_raw_nbt};

/// Blocks in one section (vanilla `PalettedContainer` entry count for block states).
const BLOCKS_PER_SECTION: usize = 4096;
/// Biome cells in one section (vanilla `PalettedContainer` entry count for biomes).
const BIOMES_PER_SECTION: usize = 64;

static WARNED_UPGRADE_DATA: AtomicBool = AtomicBool::new(false);
static WARNED_BLENDING_DATA: AtomicBool = AtomicBool::new(false);
static WARNED_BELOW_ZERO: AtomicBool = AtomicBool::new(false);
static WARNED_INTERNAL_LIGHT: AtomicBool = AtomicBool::new(false);
static WARNED_Y_POS: AtomicBool = AtomicBool::new(false);
static WARNED_UNSUPPORTED_STRUCTURE_PIECE: AtomicBool = AtomicBool::new(false);
static WARNED_MONUMENT_CHILDREN: AtomicBool = AtomicBool::new(false);

/// Returns the vanilla `Status` string for a Steel chunk status.
#[must_use]
pub(crate) const fn status_name(status: ChunkStatus) -> &'static str {
    match status {
        ChunkStatus::Empty => "minecraft:empty",
        ChunkStatus::StructureStarts => "minecraft:structure_starts",
        ChunkStatus::StructureReferences => "minecraft:structure_references",
        ChunkStatus::Biomes => "minecraft:biomes",
        ChunkStatus::Terrain => "minecraft:terrain",
        ChunkStatus::Features => "minecraft:features",
        ChunkStatus::InitializeLight => "minecraft:initialize_light",
        ChunkStatus::Light => "minecraft:light",
        ChunkStatus::Spawn => "minecraft:spawn",
        ChunkStatus::Full => "minecraft:full",
    }
}

/// Resolves a vanilla `Status` string, ignoring an optional namespace.
#[must_use]
pub(crate) fn status_from_name(name: &str) -> Option<ChunkStatus> {
    let path = name.strip_prefix("minecraft:").unwrap_or(name);
    Some(match path {
        "empty" => ChunkStatus::Empty,
        "structure_starts" => ChunkStatus::StructureStarts,
        "structure_references" => ChunkStatus::StructureReferences,
        "biomes" => ChunkStatus::Biomes,
        "terrain" => ChunkStatus::Terrain,
        "features" => ChunkStatus::Features,
        "initialize_light" => ChunkStatus::InitializeLight,
        "light" => ChunkStatus::Light,
        "spawn" => ChunkStatus::Spawn,
        "full" => ChunkStatus::Full,
        _ => return None,
    })
}

/// Encodes one persistent chunk as vanilla chunk NBT.
///
/// `include_entities` mirrors vanilla's `ChunkType`: protochunks keep their entity list inline,
/// full chunks have theirs written to `entities/*.mca` by the storage backend. `height` is the
/// dimension height, needed to pick the vanilla heightmap bit width.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "mirrors vanilla `SerializableChunkData.write` field-for-field; splitting it would obscure the tag-by-tag mapping"
)]
pub(crate) fn chunk_to_nbt(
    persistent: &PersistentChunk<'_>,
    pos: ChunkPos,
    status: ChunkStatus,
    min_y: i32,
    height: i32,
    include_entities: bool,
) -> NbtCompound {
    let min_section_y = min_y.div_euclid(16);
    let mut nbt = NbtCompound::new();
    nbt.insert("DataVersion", DATA_VERSION);
    nbt.insert("xPos", pos.0.x);
    nbt.insert("yPos", min_section_y);
    nbt.insert("zPos", pos.0.y);
    nbt.insert("LastUpdate", i64::from(persistent.last_modified) * 1000);
    nbt.insert("InhabitedTime", persistent.inhabited_time);
    nbt.insert("Status", status_name(status));
    nbt.insert("isLightOn", NbtTag::Byte(1));

    let mut tags: BTreeMap<i32, NbtCompound> = BTreeMap::new();
    for (index, section) in persistent.sections.iter().enumerate() {
        let y = min_section_y + index as i32;
        let mut tag = NbtCompound::new();
        tag.insert("Y", NbtTag::Byte(y as i8));
        tag.insert(
            "block_states",
            NbtTag::Compound(block_states_to_nbt(persistent, section)),
        );
        tag.insert(
            "biomes",
            NbtTag::Compound(biomes_to_nbt(persistent, section)),
        );
        tags.insert(y, tag);
    }

    for (key, sections) in [
        ("BlockLight", persistent.light.block.as_slice()),
        ("SkyLight", persistent.light.sky.as_slice()),
    ] {
        for light in sections {
            let bytes = match light {
                PersistentLightSection::Uninitialized { .. } => vec![0u8; DATA_LAYER_SIZE],
                PersistentLightSection::Initialized { data, .. } => data.clone(),
                PersistentLightSection::Internal { .. } => {
                    warn_once(
                        &WARNED_INTERNAL_LIGHT,
                        "Steel-internal light sections have no vanilla equivalent and are dropped on Anvil write",
                    );
                    continue;
                }
            };
            if bytes.len() != DATA_LAYER_SIZE {
                tracing::warn!(
                    "Light section {} has {} bytes, expected {DATA_LAYER_SIZE}; dropping it",
                    light.section_index(),
                    bytes.len()
                );
                continue;
            }
            let y = min_section_y - 1 + light.section_index() as i32;
            let tag = tags.entry(y).or_insert_with(|| {
                let mut tag = NbtCompound::new();
                tag.insert("Y", NbtTag::Byte(y as i8));
                tag
            });
            tag.insert(key, NbtTag::ByteArray(bytes));
        }
    }

    nbt.insert("sections", NbtList::Compound(tags.into_values().collect()));

    nbt.insert(
        "block_entities",
        NbtList::Compound(
            persistent
                .block_entities
                .iter()
                .filter_map(|entity| block_entity_to_nbt(entity, persistent, pos, min_y))
                .collect(),
        ),
    );

    if include_entities && !persistent.entities.is_empty() {
        nbt.insert(
            "entities",
            NbtList::Compound(persistent.entities.iter().map(entity_to_nbt).collect()),
        );
    }

    if !persistent.block_ticks.is_empty() {
        nbt.insert("block_ticks", ticks_to_nbt(&persistent.block_ticks, pos));
    }
    if !persistent.fluid_ticks.is_empty() {
        nbt.insert("fluid_ticks", ticks_to_nbt(&persistent.fluid_ticks, pos));
    }

    nbt.insert(
        "PostProcessing",
        NbtList::List(
            persistent
                .postprocessing
                .iter()
                .map(|offsets| {
                    NbtList::Short(offsets.iter().map(|offset| *offset as i16).collect())
                })
                .collect(),
        ),
    );

    let mut heightmaps = NbtCompound::new();
    for heightmap in &persistent.heightmaps {
        let Some(map_type) = HeightmapType::from_persistence_id(heightmap.heightmap_type) else {
            tracing::warn!(
                "Persistent heightmap has unknown type id {}; dropping it",
                heightmap.heightmap_type
            );
            continue;
        };
        heightmaps.insert(
            heightmap_key(map_type),
            NbtTag::LongArray(pack_vanilla(
                heightmap.data.iter().map(|value| u32::from(*value)),
                heightmap_bits_for_height(height),
                heightmap.data.len(),
            )),
        );
    }
    nbt.insert("Heightmaps", NbtTag::Compound(heightmaps));
    nbt.insert(
        "structures",
        structures_to_nbt(
            &persistent.structure_references,
            &persistent.structure_starts,
        ),
    );
    nbt
}

/// Decodes vanilla chunk NBT into a persistent chunk and its status.
///
/// `source` identifies the record for the version gate — normally the region file path plus
/// the chunk position — so a refused chunk says which file it came from.
///
/// # Errors
/// Returns `Unsupported` when the record's `DataVersion` is not this build's
/// (`crate::datafixer`); the caller must propagate that instead of regenerating the chunk.
/// Returns `InvalidData` for structurally invalid payloads (bad palette widths, malformed
/// records) so the caller can discard and regenerate the chunk.
#[expect(
    clippy::too_many_lines,
    reason = "mirrors vanilla `SerializableChunkData.read` field-for-field; splitting it would obscure the tag-by-tag mapping"
)]
pub(crate) fn nbt_to_chunk(
    nbt: &BorrowedNbtCompound<'_, '_>,
    pos: ChunkPos,
    min_y: i32,
    height: i32,
    source: &str,
) -> io::Result<(PersistentChunk<'static>, ChunkStatus)> {
    if height <= 0 || height % 16 != 0 || min_y % 16 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "chunk world range must be section-aligned, got min_y={min_y}, height={height}"
            ),
        ));
    }

    let status = nbt
        .string("Status")
        .and_then(|name| status_from_name(name.to_str().as_ref()))
        .unwrap_or(ChunkStatus::Empty);
    ensure_current(version_of_borrowed(nbt), DataFixType::Chunk, source)?;
    if let Some(y_pos) = nbt.int("yPos")
        && y_pos != min_y.div_euclid(16)
    {
        warn_once(
            &WARNED_Y_POS,
            "Anvil chunk `yPos` disagrees with the loaded dimension height; section placement follows the dimension",
        );
    }
    let inhabited_time = nbt.long("InhabitedTime").unwrap_or(0);
    if nbt.get("UpgradeData").is_some() {
        warn_once(
            &WARNED_UPGRADE_DATA,
            "Vanilla `UpgradeData` is dropped: Steel has no block-upgrade model",
        );
    }
    if nbt.get("blending_data").is_some() {
        warn_once(
            &WARNED_BLENDING_DATA,
            "Vanilla `blending_data` is dropped: Steel has no old-world blending model",
        );
    }
    if nbt.get("below_zero_retrogen").is_some() {
        warn_once(
            &WARNED_BELOW_ZERO,
            "Vanilla `below_zero_retrogen` is dropped: Steel has no retrogen model",
        );
    }

    let min_section_y = min_y.div_euclid(16);
    let section_count = (height / 16) as usize;
    let mut builder = PaletteBuilder::new();
    let air = builder.block_index(vanilla_blocks::AIR.default_state());
    let plains = builder.biome_index(vanilla_biomes::PLAINS.id() as u16)?;

    let mut sections: Vec<Option<PersistentSection>> = (0..section_count).map(|_| None).collect();
    let mut block_light: Vec<PersistentLightSection> = Vec::new();
    let mut sky_light: Vec<PersistentLightSection> = Vec::new();

    let sections_list = nbt.list("sections").map_or_else(Vec::new, compound_list);
    for section in sections_list {
        let y = i32::from(section.byte("Y").unwrap_or(0));
        let index = y - min_section_y;
        if (0..section_count as i32).contains(&index) {
            sections[index as usize] = Some(decode_section(&section, &mut builder, air, plains)?);
        } else if section.compound("block_states").is_some() {
            tracing::warn!(
                "Vanilla chunk {pos:?} has a block-state section at Y={y} outside the dimension range; dropping its blocks"
            );
        }

        let light_index = y - min_section_y + 1;
        if light_index < 0 || light_index >= section_count as i32 + 2 {
            continue;
        }
        for (layer, target) in [
            ("BlockLight", &mut block_light),
            ("SkyLight", &mut sky_light),
        ] {
            let Some(bytes) = section.get(layer).and_then(|tag| tag.byte_array()) else {
                continue;
            };
            if bytes.len() != DATA_LAYER_SIZE {
                tracing::warn!(
                    "Vanilla chunk {pos:?} section Y={y} has a {}-byte {layer} array, expected {DATA_LAYER_SIZE}; dropping it",
                    bytes.len()
                );
                continue;
            }
            target.push(PersistentLightSection::Initialized {
                section_index: light_index as u32,
                data: bytes.to_vec(),
            });
        }
    }

    let sections: Vec<PersistentSection> = sections
        .into_iter()
        .map(|section| {
            section.unwrap_or(PersistentSection::Homogeneous {
                block_state: air,
                biomes: PersistentBiomeData::Homogeneous { biome: plains },
            })
        })
        .collect();

    let mut heightmaps = Vec::new();
    if let Some(compound) = nbt.compound("Heightmaps") {
        for map_type in HeightmapType::all_types() {
            let Some(values) = compound
                .get(heightmap_key(*map_type))
                .and_then(|tag| tag.long_array())
            else {
                continue;
            };
            let unpacked = unpack_vanilla(&values, heightmap_bits_for_height(height), 256);
            if unpacked.len() != 256 {
                tracing::warn!(
                    "Vanilla chunk {pos:?} has a short {} heightmap; dropping it",
                    heightmap_key(*map_type)
                );
                continue;
            }
            let mut data = Vec::with_capacity(256);
            let mut invalid = false;
            for value in unpacked {
                if value as i32 > height {
                    invalid = true;
                    break;
                }
                data.push(value as u16);
            }
            if invalid {
                tracing::warn!(
                    "Vanilla chunk {pos:?} has an out-of-range {} heightmap; dropping it",
                    heightmap_key(*map_type)
                );
                continue;
            }
            heightmaps.push(PersistentHeightmap {
                heightmap_type: map_type.persistence_id(),
                data,
            });
        }
    }

    let block_ticks = ticks_from_nbt(nbt, "block_ticks", pos);
    let fluid_ticks = ticks_from_nbt(nbt, "fluid_ticks", pos);

    let mut block_entities = Vec::new();
    for entity in nbt
        .list("block_entities")
        .map_or_else(Vec::new, compound_list)
    {
        block_entities.push(block_entity_from_nbt(&entity, pos)?);
    }

    let mut entities: Vec<PersistentEntity> = Vec::new();
    for entity in nbt.list("entities").map_or_else(Vec::new, compound_list) {
        entities.push(entity_from_nbt(&entity)?);
    }

    let structures = nbt.compound("structures");
    let structure_references = structures
        .as_ref()
        .map_or_else(Vec::new, references_from_nbt);
    let structure_starts = structures.as_ref().map_or_else(Vec::new, |structures| {
        structure_starts_from_nbt(structures, pos)
    });

    let mut postprocessing = Vec::new();
    if let Some(lists) = nbt.list("PostProcessing").and_then(|list| list.lists()) {
        for index in 0..lists.len() {
            postprocessing.push(
                lists
                    .get(index)
                    .and_then(|list| list.shorts())
                    .map_or_default(|values| values.iter().map(|value| *value as u16).collect()),
            );
        }
    }

    let last_modified = nbt
        .long("LastUpdate")
        .map_or(0, |millis| (millis.max(0) / 1000) as u32);

    Ok((
        PersistentChunk {
            last_modified,
            block_states: builder.block_states,
            biomes: builder.biomes,
            sections,
            block_entities,
            entities,
            block_ticks,
            fluid_ticks,
            heightmaps,
            light: PersistentLightData {
                block: block_light,
                sky: sky_light,
            },
            postprocessing,
            structure_starts,
            structure_references,
            inhabited_time,
            pois: Vec::new(),
        },
        status,
    ))
}

/// Resolves the block state at a chunk-relative position inside a persistent chunk.
///
/// Used to derive information vanilla stores explicitly (POI types, pending block entities).
#[must_use]
pub(crate) fn block_state_at(
    chunk: &PersistentChunk<'_>,
    x: u8,
    y: i16,
    z: u8,
    min_y: i32,
) -> Option<BlockStateId> {
    let index = (i32::from(y) - min_y).div_euclid(16);
    let section = chunk.sections.get(usize::try_from(index).ok()?)?;
    let cell = ((y.rem_euclid(16) as usize) << 8) | (usize::from(z) << 4) | usize::from(x);
    let local = match section {
        PersistentSection::Homogeneous { block_state, .. } => usize::from(*block_state),
        PersistentSection::Heterogeneous {
            palette,
            bits_per_entry,
            block_data,
            ..
        } => {
            *palette.get(unpack_indices(block_data, *bits_per_entry).nth(cell)? as usize)? as usize
        }
    };
    let state = chunk.block_states.get(local)?;
    REGISTRY
        .blocks
        .state_id_from_properties(&state.name, &state.properties)
}

/// Returns the POI type id derived from the block state at a persistent POI's position.
#[must_use]
pub(crate) fn poi_type_at(
    chunk: &PersistentChunk<'_>,
    poi: &PersistentPoi,
    min_y: i32,
) -> Option<usize> {
    let state = block_state_at(chunk, poi.x, poi.y, poi.z, min_y)?;
    REGISTRY.poi_types.type_id_for_state(state)
}

/// Returns the vanilla block entity type valid for the block at a chunk-relative position.
#[must_use]
pub(crate) fn block_entity_type_at(
    chunk: &PersistentChunk<'_>,
    entity: &PersistentBlockEntity,
    min_y: i32,
) -> Option<Identifier> {
    let state = block_state_at(chunk, entity.x, entity.y, entity.z, min_y)?;
    let block = state.get_block();
    REGISTRY
        .block_entity_types
        .iter()
        .find(|(_, entity_type)| entity_type.is_valid(block))
        .map(|(_, entity_type)| entity_type.key.clone())
}

fn decode_section(
    section: &BorrowedNbtCompound<'_, '_>,
    builder: &mut PaletteBuilder,
    air: u16,
    plains: u16,
) -> io::Result<PersistentSection> {
    let block_state = match section.compound("block_states") {
        Some(blocks) => decode_packed(
            &blocks,
            read_vanilla_palette(&blocks, builder, true)?,
            BLOCKS_PER_SECTION,
            block_state_storage_bits,
            air,
        )?,
        None => DecodedPalette::Single(air),
    };
    let biomes = match section.compound("biomes") {
        Some(biomes) => decode_packed(
            &biomes,
            read_vanilla_palette(&biomes, builder, false)?,
            BIOMES_PER_SECTION,
            biome_storage_bits,
            plains,
        )?,
        None => DecodedPalette::Single(plains),
    };

    let biomes = match biomes {
        DecodedPalette::Single(biome) => PersistentBiomeData::Homogeneous { biome },
        DecodedPalette::Packed {
            palette,
            data,
            entry_count,
        } => {
            let (bits_per_entry, biome_data) =
                persistent_packed(data, palette.len(), entry_count, "biome")?;
            PersistentBiomeData::Heterogeneous {
                palette,
                bits_per_entry,
                biome_data,
            }
        }
    };

    match block_state {
        DecodedPalette::Single(block_state) => Ok(PersistentSection::Homogeneous {
            block_state,
            biomes,
        }),
        DecodedPalette::Packed {
            palette,
            data,
            entry_count,
        } => {
            let (bits_per_entry, block_data) =
                persistent_packed(data, palette.len(), entry_count, "block-state")?;
            Ok(PersistentSection::Heterogeneous {
                palette,
                bits_per_entry,
                block_data,
                biomes,
            })
        }
    }
}

/// One decoded vanilla palette container whose palette holds *chunk-level* indices.
enum DecodedPalette {
    Single(u16),
    Packed {
        palette: Vec<u16>,
        data: Vec<u32>,
        entry_count: usize,
    },
}

fn read_vanilla_palette(
    compound: &BorrowedNbtCompound<'_, '_>,
    builder: &mut PaletteBuilder,
    block_states: bool,
) -> io::Result<Vec<u16>> {
    let Some(palette) = compound.list("palette") else {
        return Ok(Vec::new());
    };
    let mut indices = Vec::new();
    if block_states {
        // Vanilla's block-state container codec reads `BlockState.CODEC`, a compound per
        // palette entry.
        for entry in compound_list(palette) {
            let state = read_block_state(&entry).unwrap_or_else(|| {
                tracing::warn!(
                    "Vanilla block-state palette entry could not be resolved; using air"
                );
                vanilla_blocks::AIR.default_state()
            });
            indices.push(builder.block_index(state));
        }
        return Ok(indices);
    }

    // Vanilla's biome container codec reads `Registry.holderByNameCodec`, so every biome
    // palette entry is the biome's id as a string.
    for name in palette.strings().unwrap_or_default() {
        let key = Identifier::from_str(name.to_str().as_ref()).ok();
        let biome_id = if let Some(biome) = key.and_then(|key| REGISTRY.biomes.id_from_key(&key)) {
            biome
        } else {
            tracing::warn!("Vanilla biome palette entry could not be resolved; using plains");
            vanilla_biomes::PLAINS.id()
        };
        indices.push(builder.biome_index(u16::try_from(biome_id).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "biome id does not fit in u16")
        })?)?);
    }
    Ok(indices)
}

fn decode_packed(
    compound: &BorrowedNbtCompound<'_, '_>,
    palette: Vec<u16>,
    entry_count: usize,
    bits_for: fn(usize) -> u8,
    default_index: u16,
) -> io::Result<DecodedPalette> {
    match palette.len() {
        0 => return Ok(DecodedPalette::Single(default_index)),
        1 => return Ok(DecodedPalette::Single(palette[0])),
        _ => {}
    }
    let Some(data) = compound.get("data").and_then(|tag| tag.long_array()) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "vanilla palette container has more than one entry but no `data` array",
        ));
    };
    let unpacked =
        unpack_vanilla_values(&data, bits_for(palette.len()), entry_count, palette.len())?;
    Ok(DecodedPalette::Packed {
        palette,
        data: unpacked,
        entry_count,
    })
}

fn unpack_vanilla_values(
    data: &[i64],
    bits: u8,
    entry_count: usize,
    palette_len: usize,
) -> io::Result<Vec<u32>> {
    if bits == 0 {
        return Ok(vec![0; entry_count]);
    }
    let values_per_word = 64 / usize::from(bits);
    let expected_words = entry_count.div_ceil(values_per_word);
    if data.len() != expected_words {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "vanilla palette container `data` has {} longs, expected {expected_words}",
                data.len()
            ),
        ));
    }
    let mut out = Vec::with_capacity(entry_count);
    for word in data {
        let word = *word as u64;
        for slot in 0..values_per_word {
            if out.len() == entry_count {
                break;
            }
            let value = ((word >> (slot * usize::from(bits))) & ((1u64 << bits) - 1)) as u32;
            if usize::try_from(value).expect("u32 always fits usize") >= palette_len {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("vanilla palette container index {value} is outside its palette"),
                ));
            }
            out.push(value);
        }
    }
    Ok(out)
}

/// Re-packs vanilla indices into Steel's power-of-two packed representation.
fn persistent_packed(
    indices: Vec<u32>,
    palette_len: usize,
    entry_count: usize,
    label: &str,
) -> io::Result<(u8, Box<[u64]>)> {
    let bits = bits_for_palette_len(palette_len).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{label} palette has fewer than two entries"),
        )
    })?;
    let values_per_word = 64 / usize::from(bits);
    let data = pack_indices_from_iter(indices.into_iter(), bits);
    if data.len() != entry_count.div_ceil(values_per_word) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("repacked {label} palette data has an unexpected length"),
        ));
    }
    Ok((bits, data))
}

fn read_block_state(entry: &BorrowedNbtCompound<'_, '_>) -> Option<BlockStateId> {
    let name = entry.string("Name").or_else(|| entry.string("name"))?;
    let key = Identifier::from_str(name.to_str().as_ref()).ok()?;
    let mut properties: Vec<(String, String)> = Vec::new();
    if let Some(encoded) = entry
        .compound("Properties")
        .or_else(|| entry.compound("properties"))
    {
        for (name, value) in encoded.iter() {
            properties.push((
                name.to_str().into_owned(),
                value.string()?.to_str().into_owned(),
            ));
        }
    }
    let block = REGISTRY.blocks.by_key(&key)?;
    REGISTRY
        .blocks
        .state_id_from_block_defaulted_properties(
            block,
            properties
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
        )
        .or_else(|| Some(block.default_state()))
}

fn block_states_to_nbt(chunk: &PersistentChunk<'_>, section: &PersistentSection) -> NbtCompound {
    let (palette, data, entry_count) = match section {
        PersistentSection::Homogeneous { block_state, .. } => {
            (vec![*block_state], None, BLOCKS_PER_SECTION)
        }
        PersistentSection::Heterogeneous {
            palette,
            bits_per_entry,
            block_data,
            ..
        } => (
            palette.clone(),
            Some(
                unpack_indices(block_data, *bits_per_entry)
                    .take(BLOCKS_PER_SECTION)
                    .collect(),
            ),
            BLOCKS_PER_SECTION,
        ),
    };
    let entries = PaletteEntries::Blocks(
        palette
            .iter()
            .map(|index| {
                if let Some(state) = chunk.block_states.get(usize::from(*index)) {
                    block_state_entry(state)
                } else {
                    tracing::warn!(
                        "Block-state palette index {index} is missing from the chunk palette; using air"
                    );
                    air_entry()
                }
            })
            .collect(),
    );
    palette_container(entries, data, entry_count, block_state_storage_bits)
}

fn biomes_to_nbt(chunk: &PersistentChunk<'_>, section: &PersistentSection) -> NbtCompound {
    let biomes = match section {
        PersistentSection::Homogeneous { biomes, .. }
        | PersistentSection::Heterogeneous { biomes, .. } => biomes,
    };
    let (palette, data) = match biomes {
        PersistentBiomeData::Homogeneous { biome } => (vec![*biome], None),
        PersistentBiomeData::Heterogeneous {
            palette,
            bits_per_entry,
            biome_data,
        } => (
            palette.clone(),
            Some(
                unpack_indices(biome_data, *bits_per_entry)
                    .take(BIOMES_PER_SECTION)
                    .collect(),
            ),
        ),
    };
    let entries = PaletteEntries::Biomes(
        palette
            .iter()
            .map(|index| {
                if let Some(biome) = chunk.biomes.get(usize::from(*index)) {
                    biome.to_string()
                } else {
                    tracing::warn!(
                        "Biome palette index {index} is missing from the chunk palette; using plains"
                    );
                    "minecraft:plains".to_owned()
                }
            })
            .collect(),
    );
    palette_container(entries, data, BIOMES_PER_SECTION, biome_storage_bits)
}

fn palette_container(
    entries: PaletteEntries,
    data: Option<Vec<u32>>,
    entry_count: usize,
    bits_for: fn(usize) -> u8,
) -> NbtCompound {
    let palette_len = entries.len();
    let mut compound = NbtCompound::new();
    compound.insert("palette", entries.into_list());
    if palette_len > 1
        && let Some(indices) = data
    {
        compound.insert(
            "data",
            NbtTag::LongArray(pack_vanilla(
                indices.into_iter(),
                bits_for(palette_len),
                entry_count,
            )),
        );
    }
    compound
}

/// Palette entry payload: block states are compounds, biomes are identifier strings.
enum PaletteEntries {
    Blocks(Vec<NbtCompound>),
    Biomes(Vec<String>),
}

impl PaletteEntries {
    const fn len(&self) -> usize {
        match self {
            Self::Blocks(entries) => entries.len(),
            Self::Biomes(entries) => entries.len(),
        }
    }

    fn into_list(self) -> NbtList {
        match self {
            Self::Blocks(entries) => NbtList::Compound(entries),
            Self::Biomes(entries) => NbtList::String(entries.into_iter().map(Into::into).collect()),
        }
    }
}

fn block_state_entry(state: &PersistentBlockState<'_>) -> NbtCompound {
    let mut entry = NbtCompound::new();
    entry.insert("Name", state.name.to_string());
    if !state.properties.is_empty() {
        let mut properties = NbtCompound::new();
        for (name, value) in &state.properties {
            properties.insert(*name, *value);
        }
        entry.insert("Properties", NbtTag::Compound(properties));
    }
    entry
}

fn air_entry() -> NbtCompound {
    block_state_entry(&PersistentBlockState {
        name: vanilla_blocks::AIR.key.clone(),
        properties: Vec::new(),
    })
}

fn ticks_to_nbt(ticks: &[PersistentTick], chunk: ChunkPos) -> NbtList {
    NbtList::Compound(
        ticks
            .iter()
            .map(|tick| {
                let mut nbt = NbtCompound::new();
                nbt.insert("i", tick.tick_type.to_string());
                nbt.insert("x", chunk.0.x * 16 + i32::from(tick.x));
                nbt.insert("y", i32::from(tick.y));
                nbt.insert("z", chunk.0.y * 16 + i32::from(tick.z));
                nbt.insert("t", tick.delay);
                nbt.insert("p", i32::from(tick.priority));
                nbt
            })
            .collect(),
    )
}

fn ticks_from_nbt(
    nbt: &BorrowedNbtCompound<'_, '_>,
    key: &str,
    chunk: ChunkPos,
) -> Vec<PersistentTick> {
    nbt.list(key)
        .map_or_else(Vec::new, compound_list)
        .iter()
        .filter_map(|tick| decode_tick(tick, chunk))
        .collect()
}

fn decode_tick(tick: &BorrowedNbtCompound<'_, '_>, chunk: ChunkPos) -> Option<PersistentTick> {
    let Some(tick_type) = tick
        .string("i")
        .and_then(|value| Identifier::from_str(value.to_str().as_ref()).ok())
    else {
        tracing::warn!("Vanilla scheduled tick has no parsable `i` type; dropping it");
        return None;
    };
    let (Some(x), Some(y), Some(z)) = (tick.int("x"), tick.int("y"), tick.int("z")) else {
        tracing::warn!("Vanilla scheduled tick is missing coordinates; dropping it");
        return None;
    };
    let local_x = x - chunk.0.x * 16;
    let local_z = z - chunk.0.y * 16;
    if !(0..16).contains(&local_x) || !(0..16).contains(&local_z) {
        tracing::warn!(
            "Vanilla scheduled tick at ({x}, {y}, {z}) is outside chunk {chunk:?}; dropping it"
        );
        return None;
    }
    let Ok(y) = i16::try_from(y) else {
        tracing::warn!("Vanilla scheduled tick Y={y} does not fit in i16; dropping it");
        return None;
    };
    Some(PersistentTick {
        x: local_x as u8,
        y,
        z: local_z as u8,
        delay: tick.int("t").unwrap_or(0),
        priority: tick.int("p").unwrap_or(0).clamp(-3, 3) as i8,
        tick_type,
    })
}

fn block_entity_to_nbt(
    entity: &PersistentBlockEntity,
    chunk: &PersistentChunk<'_>,
    pos: ChunkPos,
    min_y: i32,
) -> Option<NbtCompound> {
    let mut nbt = if entity.nbt_data.is_empty() {
        NbtCompound::new()
    } else {
        read_raw_nbt(&entity.nbt_data).ok()?
    };
    let block_entity_type = if let Some(entity_type) = &entity.entity_type {
        entity_type.clone()
    } else {
        // Vanilla stores a *pending* tag for block entities that were never materialized.
        // Steel's pending marker carries no type, so re-derive it from the block state.
        let Some(derived) = block_entity_type_at(chunk, entity, min_y) else {
            tracing::warn!(
                "Pending block entity at ({}, {}, {}) has no block entity type for its block; dropping it",
                entity.x,
                entity.y,
                entity.z
            );
            return None;
        };
        nbt.insert("keepPacked", NbtTag::Byte(1));
        derived
    };
    nbt.insert("id", block_entity_type.to_string());
    nbt.insert("x", pos.0.x * 16 + i32::from(entity.x));
    nbt.insert("y", i32::from(entity.y));
    nbt.insert("z", pos.0.y * 16 + i32::from(entity.z));
    Some(nbt)
}

fn block_entity_from_nbt(
    nbt: &BorrowedNbtCompound<'_, '_>,
    pos: ChunkPos,
) -> io::Result<PersistentBlockEntity> {
    let entity_type = nbt
        .string("id")
        .and_then(|value| Identifier::from_str(value.to_str().as_ref()).ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "vanilla block entity has no parsable `id`",
            )
        })?;
    let x = nbt.int("x").unwrap_or(0) - pos.0.x * 16;
    let z = nbt.int("z").unwrap_or(0) - pos.0.y * 16;
    let y = nbt.int("y").unwrap_or(0);
    if !(0..16).contains(&x) || !(0..16).contains(&z) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("vanilla block entity at ({x}, {y}, {z}) lies outside chunk {pos:?}"),
        ));
    }

    let mut nbt_data = NbtCompound::new();
    for (key, value) in nbt.iter() {
        let key = key.to_str();
        if matches!(key.as_ref(), "id" | "x" | "y" | "z" | "keepPacked") {
            continue;
        }
        nbt_data.insert(key.as_ref(), value.to_owned());
    }

    Ok(PersistentBlockEntity {
        x: x as u8,
        y: i16::try_from(y).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "block entity Y is out of range")
        })?,
        z: z as u8,
        entity_type: Some(entity_type),
        nbt_data: if nbt_data.is_empty() {
            Vec::new()
        } else {
            write_raw_nbt(&nbt_data)
        },
    })
}

fn structures_to_nbt(
    references: &[PersistentStructureReference],
    starts: &[PersistentStructureStart],
) -> NbtTag {
    let mut references_compound = NbtCompound::new();
    for reference in references {
        if reference.references.is_empty() {
            continue;
        }
        references_compound.insert(
            reference.structure.to_string(),
            NbtTag::LongArray(
                reference
                    .references
                    .iter()
                    .map(|packed| packed.as_raw())
                    .collect(),
            ),
        );
    }
    let mut structures = NbtCompound::new();
    structures.insert("starts", NbtTag::Compound(structure_starts_to_nbt(starts)));
    structures.insert("References", NbtTag::Compound(references_compound));
    NbtTag::Compound(structures)
}

fn references_from_nbt(
    structures: &BorrowedNbtCompound<'_, '_>,
) -> Vec<PersistentStructureReference> {
    let Some(references) = structures.compound("References") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, value) in references.iter() {
        let Ok(structure) = Identifier::from_str(key.to_str().as_ref()) else {
            continue;
        };
        let Some(packed) = value.long_array() else {
            continue;
        };
        out.push(PersistentStructureReference {
            structure,
            references: packed.into_iter().map(PackedChunkPos::from_raw).collect(),
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Structure starts (`structures.starts`)
// ---------------------------------------------------------------------------
//
// `vanilla` `StructureStart.createTag` writes `id` (the structure registry key), `ChunkX`,
// `ChunkZ`, `references`, and `Children` (a list of `StructurePiece.createTag` compounds).
// `StructurePiece.createTag` writes `id` (the `StructurePieceType` registry key), `BB`
// (`BoundingBox.CODEC`, an int array of `minX, minY, minZ, maxX, maxY, maxZ`), `O`
// (`Direction.get2DDataValue`, `-1` for none — an int, *not* a direction name), and `GD`,
// then delegates to the family's `addAdditionalSaveData`.
//
// The family keys below were read from the vanilla classes themselves:
//
// * `PoolElementStructurePiece` — `PosX`/`PosY`/`PosZ`, `ground_level_delta`, `pool_element`
//   (`StructurePoolElement.CODEC`), `rotation` (`Rotation.LEGACY_CODEC`), `junctions`,
//   `liquid_settings` (only when it differs from `JigsawStructure.DEFAULT_LIQUID_SETTINGS`).
// * `TemplateStructurePiece` — `TPX`/`TPY`/`TPZ`, `Template`; subclasses add `Rot`
//   (`IglooPiece`, `EndCityPiece`, `OceanRuinPiece`, `NetherFossilPiece`, `ShipwreckPiece`,
//   `WoodlandMansionPiece`), `OW` (end city), `Mi` (mansion), `isBeached` and
//   `height_adjusted` (shipwreck), `Integrity`/`BiomeType`/`IsLarge` (ocean ruin), and
//   `Rotation`/`Mirror`/`VerticalPlacement`/`Properties` (ruined portal).
// * `ScatteredFeaturePiece` — `Width`, `Height`, `Depth`, `HPos`; subclasses add
//   `hasPlacedChest0..3` (desert pyramid), `placedMainChest`/`placedHiddenChest`/`placedTrap1`/
//   `placedTrap2` (jungle temple), `Witch`/`Cat` (swamp hut).
// * `MineshaftPieces` — `MST`, `Entrances`, `hr`/`sc`/`hps`/`Num`, `tf`/`D`.
// * `NetherFortressPieces` — `Seed`, `Chest`, `Mob`.
// * `StrongholdPieces` — `EntryDoor`, `Chest`, `Type`, `Tall`, `Mob`, `Steps`, `Left`/`Right`,
//   `leftLow`/`leftHigh`/`rightLow`/`rightHigh`, `Source`.
// * `OceanMonumentPieces.MonumentBuilding` and `BuriedTreasurePieces.BuriedTreasurePiece` add
//   nothing: vanilla regenerates monument rooms from the world seed after load.
//
// `Rotation.LEGACY_CODEC`/`Mirror.LEGACY_CODEC` are `ExtraCodecs.legacyEnum`, so they spell the
// enum constant name (`CLOCKWISE_180`, `FRONT_BACK`) rather than the lowercase
// `StringRepresentable` name; projection, liquid settings, vertical placement, and the
// ocean-ruin type use the lowercase names.
//
// Template pieces store only the template id/position plus the family's own keys. Everything
// else sits in the vanilla piece constructor, so decoding reconstructs those values from the
// same per-family tables (`IglooPieces.PIVOTS`/`OFFSETS`, `ShipwreckPieces.PIVOT`, the
// `BlockIgnoreProcessor` each `makeSettings` adds, ...).

/// Vanilla `StructurePieceType` key for a jigsaw pool piece.
const PIECE_TYPE_JIGSAW: &str = "minecraft:jigsaw";

/// Vanilla `StructurePoolElementType` keys.
const POOL_ELEMENT_SINGLE: &str = "minecraft:single_pool_element";
const POOL_ELEMENT_LEGACY_SINGLE: &str = "minecraft:legacy_single_pool_element";
const POOL_ELEMENT_FEATURE: &str = "minecraft:feature_pool_element";
const POOL_ELEMENT_EMPTY: &str = "minecraft:empty_pool_element";
const POOL_ELEMENT_LIST: &str = "minecraft:list_pool_element";

/// Registry key of vanilla's empty `StructureProcessorList`, the default `processors` value.
const EMPTY_PROCESSOR_LIST: &str = "minecraft:empty";

// Persisted template-payload encodings used when reconstructing fields vanilla does not store;
// the numbering is [`PersistentTemplatePieceData`]'s (documented on each field).
const BLOCK_IGNORE_NONE: i8 = 0;
const BLOCK_IGNORE_STRUCTURE_BLOCK: i8 = 1;
const BLOCK_IGNORE_STRUCTURE_AND_AIR: i8 = 2;
const LIQUID_SETTINGS_APPLY: i8 = 0;
const LIQUID_SETTINGS_IGNORE: i8 = 1;
const MARKER_IGNORE: i8 = 0;
const MARKER_SHIPWRECK: i8 = 2;
const MARKER_IGLOO: i8 = 3;
const MARKER_OCEAN_RUIN_SMALL: i8 = 4;
const MARKER_OCEAN_RUIN_LARGE: i8 = 5;
const MARKER_END_CITY: i8 = 6;
const MARKER_WOODLAND_MANSION: i8 = 7;
const CLIP_CENTER_CHUNK: i8 = 0;
const CLIP_EXPANDED_TO_TEMPLATE: i8 = 1;
const CLIP_CONTAINS_CENTER_EXPANDED_TO_TEMPLATE: i8 = 2;
const POST_PROCESS_NONE: i8 = 0;
const POST_PROCESS_NETHER_FOSSIL: i8 = 1;
const POST_PROCESS_IGLOO_TOP: i8 = 2;
const POST_PROCESS_RUINED_PORTAL: i8 = 3;
const ROTATION_NONE: i8 = 0;
const MIRROR_NONE: i8 = 0;
const PROJECTION_NONE: i8 = -1;
const PROJECTION_RIGID: i8 = 0;
const PROJECTION_TERRAIN_MATCHING: i8 = 1;
const OCEAN_RUIN_WARM: i8 = 0;
const OCEAN_RUIN_COLD: i8 = 1;
const VERTICAL_PLACEMENT_ON_LAND_SURFACE: i8 = 0;

/// `Rotation.LEGACY_CODEC` spelling of a persisted rotation id.
#[must_use]
const fn rotation_name(rotation: i8) -> &'static str {
    match rotation {
        1 => "CLOCKWISE_90",
        2 => "CLOCKWISE_180",
        3 => "COUNTERCLOCKWISE_90",
        _ => "NONE",
    }
}

/// Parses a `Rotation.LEGACY_CODEC` name into a persisted rotation id.
#[must_use]
fn rotation_from_name(name: &str) -> Option<i8> {
    Some(match name {
        "NONE" => ROTATION_NONE,
        "CLOCKWISE_90" => 1,
        "CLOCKWISE_180" => 2,
        "COUNTERCLOCKWISE_90" => 3,
        _ => return None,
    })
}

/// `Mirror.LEGACY_CODEC` spelling of a persisted mirror id.
#[must_use]
const fn mirror_name(mirror: i8) -> &'static str {
    match mirror {
        1 => "FRONT_BACK",
        2 => "LEFT_RIGHT",
        _ => "NONE",
    }
}

/// Parses a `Mirror.LEGACY_CODEC` name into a persisted mirror id.
#[must_use]
fn mirror_from_name(name: &str) -> Option<i8> {
    Some(match name {
        "NONE" => MIRROR_NONE,
        "FRONT_BACK" => 1,
        "LEFT_RIGHT" => 2,
        _ => return None,
    })
}

/// `StructureTemplatePool.Projection.CODEC` spelling of a persisted projection id.
#[must_use]
const fn projection_name(projection: i8) -> &'static str {
    match projection {
        PROJECTION_TERRAIN_MATCHING => "terrain_matching",
        _ => "rigid",
    }
}

/// Parses a projection name into a persisted projection id.
#[must_use]
fn projection_from_name(name: &str) -> Option<i8> {
    Some(match name {
        "rigid" => PROJECTION_RIGID,
        "terrain_matching" => PROJECTION_TERRAIN_MATCHING,
        _ => return None,
    })
}

/// `LiquidSettings.CODEC` spelling of persisted liquid settings.
#[must_use]
const fn liquid_settings_name(settings: i8) -> &'static str {
    match settings {
        1 => "ignore_waterlogging",
        _ => "apply_waterlogging",
    }
}

/// Parses a `LiquidSettings.CODEC` name into persisted liquid settings.
#[must_use]
fn liquid_settings_from_name(name: &str) -> Option<i8> {
    Some(match name {
        "apply_waterlogging" => LIQUID_SETTINGS_APPLY,
        "ignore_waterlogging" => LIQUID_SETTINGS_IGNORE,
        _ => return None,
    })
}

/// `RuinedPortalPiece.VerticalPlacement.CODEC` spelling of a persisted placement id.
#[must_use]
const fn vertical_placement_name(placement: i8) -> &'static str {
    match placement {
        1 => "partly_buried",
        2 => "underground",
        3 => "in_mountain",
        4 => "on_ocean_floor",
        5 => "in_nether",
        _ => "on_land_surface",
    }
}

/// Parses a `VerticalPlacement` name into a persisted placement id.
#[must_use]
fn vertical_placement_from_name(name: &str) -> Option<i8> {
    Some(match name {
        "on_land_surface" => VERTICAL_PLACEMENT_ON_LAND_SURFACE,
        "partly_buried" => 1,
        "underground" => 2,
        "in_mountain" => 3,
        "on_ocean_floor" => 4,
        "in_nether" => 5,
        _ => return None,
    })
}

/// `OceanRuinStructure.Type.LEGACY_CODEC` spelling of a persisted biome temperature.
#[must_use]
const fn ocean_ruin_biome_temp_name(biome_temp: i8) -> &'static str {
    match biome_temp {
        1 => "COLD",
        _ => "WARM",
    }
}

/// Parses an ocean-ruin `BiomeType` name into a persisted biome temperature.
#[must_use]
fn ocean_ruin_biome_temp_from_name(name: &str) -> Option<i8> {
    Some(match name {
        "WARM" => OCEAN_RUIN_WARM,
        "COLD" => OCEAN_RUIN_COLD,
        _ => return None,
    })
}

/// `SmallDoorType.LEGACY_CODEC` spelling of a persisted stronghold entry door.
#[must_use]
const fn stronghold_door_name(door: &PersistentStrongholdSmallDoorType) -> &'static str {
    match door {
        PersistentStrongholdSmallDoorType::Opening => "OPENING",
        PersistentStrongholdSmallDoorType::WoodDoor => "WOOD_DOOR",
        PersistentStrongholdSmallDoorType::Grates => "GRATES",
        PersistentStrongholdSmallDoorType::IronDoor => "IRON_DOOR",
    }
}

/// Parses a `SmallDoorType.LEGACY_CODEC` name into a persisted stronghold entry door.
#[must_use]
fn stronghold_door_from_name(name: &str) -> Option<PersistentStrongholdSmallDoorType> {
    Some(match name {
        "OPENING" => PersistentStrongholdSmallDoorType::Opening,
        "WOOD_DOOR" => PersistentStrongholdSmallDoorType::WoodDoor,
        "GRATES" => PersistentStrongholdSmallDoorType::Grates,
        "IRON_DOOR" => PersistentStrongholdSmallDoorType::IronDoor,
        _ => return None,
    })
}

/// Encodes structure starts as vanilla's `structures.starts` compound.
fn structure_starts_to_nbt(starts: &[PersistentStructureStart]) -> NbtCompound {
    let mut compound = NbtCompound::new();
    for start in starts {
        compound.insert(
            start.structure.to_string(),
            NbtTag::Compound(start_to_nbt(start)),
        );
    }
    compound
}

/// Encodes one start exactly as `StructureStart.createTag` does.
fn start_to_nbt(start: &PersistentStructureStart) -> NbtCompound {
    let mut tag = NbtCompound::new();
    tag.insert("id", start.structure.to_string());
    tag.insert("ChunkX", start.chunk_x);
    tag.insert("ChunkZ", start.chunk_z);
    tag.insert("references", start.references);
    tag.insert(
        "Children",
        NbtList::Compound(start.pieces.iter().map(piece_to_nbt).collect()),
    );
    tag
}

/// Encodes one piece as `StructurePiece.createTag` plus its family's `addAdditionalSaveData`.
fn piece_to_nbt(piece: &PersistentStructurePiece) -> NbtCompound {
    let mut tag = NbtCompound::new();
    let piece_type = piece.piece_type.to_string();
    tag.insert("id", piece_type.clone());
    tag.insert(
        "BB",
        NbtTag::IntArray(vec![
            piece.bounding_box.min_x,
            piece.bounding_box.min_y,
            piece.bounding_box.min_z,
            piece.bounding_box.max_x,
            piece.bounding_box.max_y,
            piece.bounding_box.max_z,
        ]),
    );
    tag.insert("O", i32::from(piece.orientation));
    tag.insert("GD", piece.gen_depth);
    match &piece.payload {
        PersistentStructurePiecePayload::Jigsaw(data) => {
            tag.insert("PosX", data.position[0]);
            tag.insert("PosY", data.position[1]);
            tag.insert("PosZ", data.position[2]);
            tag.insert("ground_level_delta", piece.ground_level_delta);
            tag.insert(
                "pool_element",
                NbtTag::Compound(pool_element_to_nbt(&data.pool_element)),
            );
            tag.insert("rotation", rotation_name(data.rotation));
            tag.insert(
                "junctions",
                NbtList::Compound(piece.junctions.iter().map(junction_to_nbt).collect()),
            );
            if data.liquid_settings != LIQUID_SETTINGS_APPLY {
                tag.insert(
                    "liquid_settings",
                    liquid_settings_name(data.liquid_settings),
                );
            }
        }
        PersistentStructurePiecePayload::Template(data) => {
            template_piece_to_nbt(&piece_type, data, &mut tag);
        }
        PersistentStructurePiecePayload::Procedural(data) => {
            procedural_piece_to_nbt(&piece_type, piece.bounding_box, data, &mut tag);
        }
    }
    tag
}

/// Adds `TemplateStructurePiece` keys plus the template family's own keys.
fn template_piece_to_nbt(
    piece_type: &str,
    data: &PersistentTemplatePieceData,
    tag: &mut NbtCompound,
) {
    tag.insert("TPX", data.template_position[0]);
    tag.insert("TPY", data.template_position[1]);
    tag.insert("TPZ", data.template_position[2]);
    tag.insert("Template", data.template_id.to_string());
    match piece_type {
        "minecraft:iglu" | "minecraft:nefos" => {
            tag.insert("Rot", rotation_name(data.rotation));
        }
        "minecraft:shipwreck" => {
            let (is_beached, height_adjusted) = match data.placement_adjustment {
                PersistentTemplatePlacementAdjustment::Shipwreck {
                    is_beached,
                    height_adjusted,
                } => (is_beached, height_adjusted),
                _ => (false, false),
            };
            tag.insert("isBeached", is_beached);
            tag.insert("Rot", rotation_name(data.rotation));
            tag.insert("height_adjusted", height_adjusted);
        }
        "minecraft:orp" => {
            let (biome_temp, integrity) = match data.processors {
                PersistentTemplateProcessorList::OceanRuin {
                    biome_temp,
                    integrity,
                } => (biome_temp, integrity),
                _ => (OCEAN_RUIN_WARM, 1.0),
            };
            tag.insert("Rot", rotation_name(data.rotation));
            tag.insert("Integrity", integrity);
            tag.insert("BiomeType", ocean_ruin_biome_temp_name(biome_temp));
            tag.insert("IsLarge", data.marker_handling == MARKER_OCEAN_RUIN_LARGE);
        }
        "minecraft:rupo" => {
            let (placement, properties) = match data.processors {
                PersistentTemplateProcessorList::RuinedPortal {
                    vertical_placement,
                    cold,
                    mossiness,
                    air_pocket,
                    overgrown,
                    vines,
                    replace_with_blackstone,
                } => (
                    vertical_placement,
                    RuinedPortalPropertiesFields {
                        cold,
                        mossiness,
                        air_pocket,
                        overgrown,
                        vines,
                        replace_with_blackstone,
                    },
                ),
                _ => (
                    VERTICAL_PLACEMENT_ON_LAND_SURFACE,
                    RuinedPortalPropertiesFields {
                        cold: false,
                        mossiness: 1.0,
                        air_pocket: false,
                        overgrown: false,
                        vines: false,
                        replace_with_blackstone: false,
                    },
                ),
            };
            tag.insert("Rotation", rotation_name(data.rotation));
            tag.insert("Mirror", mirror_name(data.mirror));
            tag.insert("VerticalPlacement", vertical_placement_name(placement));
            let mut properties_tag = NbtCompound::new();
            properties_tag.insert("cold", properties.cold);
            properties_tag.insert("mossiness", properties.mossiness);
            properties_tag.insert("air_pocket", properties.air_pocket);
            properties_tag.insert("overgrown", properties.overgrown);
            properties_tag.insert("vines", properties.vines);
            properties_tag.insert(
                "replace_with_blackstone",
                properties.replace_with_blackstone,
            );
            tag.insert("Properties", NbtTag::Compound(properties_tag));
        }
        "minecraft:ecp" => {
            tag.insert("Rot", rotation_name(data.rotation));
            tag.insert("OW", data.block_ignore == BLOCK_IGNORE_STRUCTURE_BLOCK);
        }
        "minecraft:wmp" => {
            tag.insert("Rot", rotation_name(data.rotation));
            tag.insert("Mi", mirror_name(data.mirror));
        }
        other => {
            warn_once(
                &WARNED_UNSUPPORTED_STRUCTURE_PIECE,
                "A template structure piece has no vanilla piece type; only its base keys are written",
            );
            tracing::debug!(
                piece_type = other,
                "template piece outside the vanilla template families"
            );
        }
    }
}

/// The `RuinedPortalPiece.Properties` record fields.
struct RuinedPortalPropertiesFields {
    cold: bool,
    mossiness: f32,
    air_pocket: bool,
    overgrown: bool,
    vines: bool,
    replace_with_blackstone: bool,
}

/// Adds the code-generated families' keys, mirroring each vanilla `addAdditionalSaveData`.
#[expect(
    clippy::too_many_lines,
    reason = "mirrors the code-generated piece families' vanilla `addAdditionalSaveData` methods 1:1; splitting it would break the correspondence"
)]
fn procedural_piece_to_nbt(
    piece_type: &str,
    bounding_box: PersistentBoundingBox,
    data: &PersistentProceduralPieceData,
    tag: &mut NbtCompound,
) {
    let (width, height, depth) = scattered_feature_dimensions(bounding_box);
    match data {
        PersistentProceduralPieceData::Unimplemented => {
            tracing::debug!(
                piece_type,
                "structure piece has no captured placement state; only base keys are written"
            );
        }
        PersistentProceduralPieceData::BuriedTreasure => {}
        PersistentProceduralPieceData::DesertPyramid(data) => {
            scattered_feature_keys(width, height, depth, data.height_position, tag);
            for (index, placed) in data.has_placed_chest.iter().enumerate() {
                tag.insert(format!("hasPlacedChest{index}"), *placed);
            }
        }
        PersistentProceduralPieceData::JungleTemple(data) => {
            scattered_feature_keys(width, height, depth, data.height_position, tag);
            tag.insert("placedMainChest", data.placed_main_chest);
            tag.insert("placedHiddenChest", data.placed_hidden_chest);
            tag.insert("placedTrap1", data.placed_trap1);
            tag.insert("placedTrap2", data.placed_trap2);
        }
        PersistentProceduralPieceData::SwampHut(data) => {
            scattered_feature_keys(width, height, depth, data.height_position, tag);
            tag.insert("Witch", data.spawned_witch);
            tag.insert("Cat", data.spawned_cat);
        }
        PersistentProceduralPieceData::Mineshaft(data) => {
            tag.insert("MST", i32::from(data.mineshaft_type));
            match &data.kind {
                PersistentMineshaftPieceKind::Room {
                    child_entrance_boxes,
                } => {
                    tag.insert(
                        "Entrances",
                        NbtList::IntArray(
                            child_entrance_boxes
                                .iter()
                                .map(|entry| {
                                    vec![
                                        entry.min_x,
                                        entry.min_y,
                                        entry.min_z,
                                        entry.max_x,
                                        entry.max_y,
                                        entry.max_z,
                                    ]
                                })
                                .collect(),
                        ),
                    );
                }
                PersistentMineshaftPieceKind::Corridor {
                    has_rails,
                    spider_corridor,
                    has_placed_spider,
                    num_sections,
                } => {
                    tag.insert("hr", *has_rails);
                    tag.insert("sc", *spider_corridor);
                    tag.insert("hps", *has_placed_spider);
                    tag.insert("Num", *num_sections);
                }
                PersistentMineshaftPieceKind::Crossing {
                    direction,
                    is_two_floored,
                } => {
                    tag.insert("tf", *is_two_floored);
                    tag.insert("D", *direction);
                }
                PersistentMineshaftPieceKind::Stairs => {}
            }
        }
        PersistentProceduralPieceData::NetherFortress(data) => match data {
            PersistentNetherFortressPieceData::BridgeEndFiller { self_seed } => {
                tag.insert("Seed", *self_seed);
            }
            PersistentNetherFortressPieceData::CastleSmallCorridorLeftTurn { is_needing_chest }
            | PersistentNetherFortressPieceData::CastleSmallCorridorRightTurn {
                is_needing_chest,
            } => {
                tag.insert("Chest", *is_needing_chest);
            }
            PersistentNetherFortressPieceData::MonsterThrone { has_placed_spawner } => {
                tag.insert("Mob", *has_placed_spawner);
            }
            PersistentNetherFortressPieceData::BridgeCrossing
            | PersistentNetherFortressPieceData::BridgeStraight
            | PersistentNetherFortressPieceData::CastleCorridorStairs
            | PersistentNetherFortressPieceData::CastleCorridorTBalcony
            | PersistentNetherFortressPieceData::CastleEntrance
            | PersistentNetherFortressPieceData::CastleSmallCorridorCrossing
            | PersistentNetherFortressPieceData::CastleSmallCorridor
            | PersistentNetherFortressPieceData::CastleStalkRoom
            | PersistentNetherFortressPieceData::RoomCrossing
            | PersistentNetherFortressPieceData::StairsRoom => {}
        },
        PersistentProceduralPieceData::OceanMonument(_) => {
            warn_once(
                &WARNED_MONUMENT_CHILDREN,
                "Vanilla writes no keys for ocean-monument rooms and regenerates them from the world seed, so Steel's persisted child rooms cannot be written to Anvil",
            );
        }
        PersistentProceduralPieceData::Stronghold(data) => {
            stronghold_piece_to_nbt(data, tag);
        }
    }
}

/// Writes `ScatteredFeaturePiece`'s own keys.
///
/// Vanilla's constructor builds the piece box with `makeBoundingBox(..., width, height, depth)`
/// and stores those same three values, so the box extents are the persisted dimensions.
fn scattered_feature_keys(
    width: i32,
    height: i32,
    depth: i32,
    height_position: i32,
    tag: &mut NbtCompound,
) {
    tag.insert("Width", width);
    tag.insert("Height", height);
    tag.insert("Depth", depth);
    tag.insert("HPos", height_position);
}

/// `(width, height, depth)` of a scattered-feature piece, taken from its bounding box.
const fn scattered_feature_dimensions(bounding_box: PersistentBoundingBox) -> (i32, i32, i32) {
    (
        bounding_box.max_x - bounding_box.min_x + 1,
        bounding_box.max_y - bounding_box.min_y + 1,
        bounding_box.max_z - bounding_box.min_z + 1,
    )
}

/// Writes a stronghold piece's `EntryDoor` plus its variant's own keys.
///
/// Vanilla has no `EntryDoor`-less stronghold piece: `StrongholdPiece`'s worldgen constructor
/// defaults the field to `SmallDoorType.OPENING`, and `PortalRoom`/`FillerCorridor` never
/// change it, so those two write `OPENING`.
fn stronghold_piece_to_nbt(data: &PersistentStrongholdPieceData, tag: &mut NbtCompound) {
    let entry_door = match data {
        PersistentStrongholdPieceData::Straight {
            entry_door,
            left_child,
            right_child,
        } => {
            tag.insert("Left", *left_child);
            tag.insert("Right", *right_child);
            Some(entry_door)
        }
        PersistentStrongholdPieceData::PrisonHall { entry_door }
        | PersistentStrongholdPieceData::LeftTurn { entry_door }
        | PersistentStrongholdPieceData::RightTurn { entry_door }
        | PersistentStrongholdPieceData::StraightStairsDown { entry_door } => Some(entry_door),
        PersistentStrongholdPieceData::RoomCrossing {
            entry_door,
            crossing_type,
        } => {
            tag.insert("Type", *crossing_type);
            Some(entry_door)
        }
        PersistentStrongholdPieceData::StairsDown {
            entry_door,
            is_source,
        } => {
            tag.insert("Source", *is_source);
            Some(entry_door)
        }
        PersistentStrongholdPieceData::FiveCrossing {
            entry_door,
            left_low,
            left_high,
            right_low,
            right_high,
        } => {
            tag.insert("leftLow", *left_low);
            tag.insert("leftHigh", *left_high);
            tag.insert("rightLow", *right_low);
            tag.insert("rightHigh", *right_high);
            Some(entry_door)
        }
        PersistentStrongholdPieceData::ChestCorridor {
            entry_door,
            has_placed_chest,
        } => {
            tag.insert("Chest", *has_placed_chest);
            Some(entry_door)
        }
        PersistentStrongholdPieceData::Library {
            entry_door,
            is_tall,
        } => {
            tag.insert("Tall", *is_tall);
            Some(entry_door)
        }
        PersistentStrongholdPieceData::PortalRoom { has_placed_spawner } => {
            tag.insert("Mob", *has_placed_spawner);
            None
        }
        PersistentStrongholdPieceData::FillerCorridor { steps } => {
            tag.insert("Steps", *steps);
            None
        }
    };
    tag.insert(
        "EntryDoor",
        stronghold_door_name(entry_door.unwrap_or(&PersistentStrongholdSmallDoorType::Opening)),
    );
}

/// Encodes a persisted pool element with `StructurePoolElement.CODEC`.
fn pool_element_to_nbt(element: &PersistentPoolElement) -> NbtCompound {
    let mut tag = NbtCompound::new();
    match element {
        PersistentPoolElement::Single {
            location,
            processors,
            projection,
        } => {
            tag.insert("element_type", POOL_ELEMENT_SINGLE);
            tag.insert("location", location.to_string());
            tag.insert("processors", processor_list_name(processors));
            tag.insert("projection", projection_name(*projection));
        }
        PersistentPoolElement::LegacySingle {
            location,
            processors,
            projection,
        } => {
            tag.insert("element_type", POOL_ELEMENT_LEGACY_SINGLE);
            tag.insert("location", location.to_string());
            tag.insert("processors", processor_list_name(processors));
            tag.insert("projection", projection_name(*projection));
        }
        PersistentPoolElement::Feature {
            feature,
            projection,
        } => {
            tag.insert("element_type", POOL_ELEMENT_FEATURE);
            tag.insert("feature", feature.to_string());
            tag.insert("projection", projection_name(*projection));
        }
        PersistentPoolElement::Empty => {
            tag.insert("element_type", POOL_ELEMENT_EMPTY);
        }
        PersistentPoolElement::List {
            elements,
            projection,
        } => {
            tag.insert("element_type", POOL_ELEMENT_LIST);
            tag.insert(
                "elements",
                NbtList::Compound(elements.iter().map(pool_element_to_nbt).collect()),
            );
            tag.insert("projection", projection_name(*projection));
        }
    }
    tag
}

/// Registry key of a persisted processor list.
fn processor_list_name(processors: &PersistentProcessorList) -> String {
    match processors {
        PersistentProcessorList::Empty => EMPTY_PROCESSOR_LIST.to_owned(),
        PersistentProcessorList::Registry(id) => id.to_string(),
    }
}

/// Encodes one jigsaw junction with `JigsawJunction.serialize`'s keys.
fn junction_to_nbt(junction: &PersistentJigsawJunction) -> NbtCompound {
    let mut tag = NbtCompound::new();
    tag.insert("source_x", junction.source_x);
    tag.insert("source_ground_y", junction.source_ground_y);
    tag.insert("source_z", junction.source_z);
    tag.insert("delta_y", junction.delta_y);
    tag.insert("dest_proj", projection_name(junction.dest_projection));
    tag
}

/// Decodes `structures.starts`, dropping starts that cannot be rebuilt.
///
/// Vanilla `StructureStart.loadStaticStart` returns `null` (and logs) for a start whose
/// structure or pieces it cannot resolve; the same starts are skipped here so a chunk never
/// carries a half-built piece.
fn structure_starts_from_nbt(
    structures: &BorrowedNbtCompound<'_, '_>,
    pos: ChunkPos,
) -> Vec<PersistentStructureStart> {
    let Some(starts) = structures.compound("starts") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, value) in starts.iter() {
        let structure = key.to_str();
        let Some(start_tag) = value.compound() else {
            tracing::warn!(
                chunk = ?pos,
                structure = structure.as_ref(),
                "Vanilla structure start is not a compound; skipping it"
            );
            continue;
        };
        match start_from_nbt(structure.as_ref(), &start_tag) {
            Ok(start) => out.push(start),
            Err(reason) => {
                tracing::warn!(
                    chunk = ?pos,
                    structure = structure.as_ref(),
                    reason = reason.as_str(),
                    "Vanilla structure start cannot be rebuilt; skipping it"
                );
            }
        }
    }
    out
}

/// Decodes one `StructureStart.createTag` payload.
fn start_from_nbt(
    structure_key: &str,
    tag: &BorrowedNbtCompound<'_, '_>,
) -> Result<PersistentStructureStart, String> {
    let structure = Identifier::from_str(structure_key)
        .map_err(|_| format!("structure key `{structure_key}` is not an identifier"))?;
    if REGISTRY.structures.by_key(&structure).is_none() {
        return Err(format!("unknown structure `{structure}`"));
    }
    let mut pieces = Vec::new();
    for piece in tag.list("Children").map_or_else(Vec::new, compound_list) {
        pieces.push(piece_from_nbt(&piece)?);
    }
    Ok(PersistentStructureStart {
        structure,
        chunk_x: tag.int("ChunkX").unwrap_or(0),
        chunk_z: tag.int("ChunkZ").unwrap_or(0),
        references: tag.int("references").unwrap_or(0),
        pieces,
    })
}

/// Decodes one `StructurePiece.createTag` payload plus its family's `addAdditionalSaveData`.
#[expect(
    clippy::too_many_lines,
    reason = "mirrors vanilla `StructurePiece.createTag` plus each family's `addAdditionalSaveData` 1:1; splitting it would break the correspondence"
)]
fn piece_from_nbt(tag: &BorrowedNbtCompound<'_, '_>) -> Result<PersistentStructurePiece, String> {
    let piece_type = tag
        .string("id")
        .and_then(|value| Identifier::from_str(value.to_str().as_ref()).ok())
        .ok_or_else(|| "structure piece has no parsable `id`".to_owned())?;
    let values = tag
        .int_array("BB")
        .ok_or_else(|| format!("structure piece `{piece_type}` has no `BB` int array"))?;
    if values.len() != 6 {
        return Err(format!(
            "structure piece `{piece_type}` has a {}-entry `BB`, expected 6",
            values.len()
        ));
    }
    let bounding_box = PersistentBoundingBox {
        min_x: values[0],
        min_y: values[1],
        min_z: values[2],
        max_x: values[3],
        max_y: values[4],
        max_z: values[5],
    };
    let orientation = match tag.int("O").unwrap_or(-1) {
        -1 => -1,
        value @ 0..=3 => value as i8,
        value => {
            return Err(format!(
                "structure piece `{piece_type}` has out-of-range orientation `O` {value}"
            ));
        }
    };
    let gen_depth = tag.int("GD").unwrap_or(0);
    let piece_type_key = piece_type.to_string();
    let (payload, ground_level_delta, projection, junctions) = match piece_type_key.as_str() {
        PIECE_TYPE_JIGSAW => jigsaw_piece_fields(tag, &piece_type)?,
        "minecraft:iglu"
        | "minecraft:shipwreck"
        | "minecraft:orp"
        | "minecraft:rupo"
        | "minecraft:ecp"
        | "minecraft:wmp"
        | "minecraft:nefos" => (
            PersistentStructurePiecePayload::Template(template_payload_from_nbt(
                &piece_type_key,
                tag,
            )?),
            0,
            PROJECTION_NONE,
            Vec::new(),
        ),
        "minecraft:tedp"
        | "minecraft:tejp"
        | "minecraft:tesh"
        | "minecraft:btp"
        | "minecraft:msroom"
        | "minecraft:mscorridor"
        | "minecraft:mscrossing"
        | "minecraft:msstairs"
        | "minecraft:nebcr"
        | "minecraft:nebef"
        | "minecraft:nebs"
        | "minecraft:neccs"
        | "minecraft:nectb"
        | "minecraft:nece"
        | "minecraft:nescsc"
        | "minecraft:nesclt"
        | "minecraft:nesc"
        | "minecraft:nescrt"
        | "minecraft:necsr"
        | "minecraft:nemt"
        | "minecraft:nerc"
        | "minecraft:nesr"
        | "minecraft:omb"
        | "minecraft:shstart"
        | "minecraft:shsd"
        | "minecraft:shs"
        | "minecraft:shph"
        | "minecraft:shlt"
        | "minecraft:shrt"
        | "minecraft:shrc"
        | "minecraft:shssd"
        | "minecraft:sh5c"
        | "minecraft:shcc"
        | "minecraft:shli"
        | "minecraft:shpr"
        | "minecraft:shfc" => (
            PersistentStructurePiecePayload::Procedural(procedural_payload_from_nbt(
                &piece_type_key,
                tag,
            )?),
            0,
            PROJECTION_NONE,
            Vec::new(),
        ),
        _ => return Err(format!("unsupported structure piece type `{piece_type}`")),
    };
    Ok(PersistentStructurePiece {
        piece_type,
        bounding_box,
        gen_depth,
        orientation,
        payload,
        ground_level_delta,
        projection,
        junctions,
    })
}

/// Decodes a jigsaw pool piece's payload and the piece-level fields vanilla keeps on it.
fn jigsaw_piece_fields(
    tag: &BorrowedNbtCompound<'_, '_>,
    piece_type: &Identifier,
) -> Result<
    (
        PersistentStructurePiecePayload,
        i32,
        i8,
        Vec<PersistentJigsawJunction>,
    ),
    String,
> {
    let element = tag
        .compound("pool_element")
        .ok_or_else(|| format!("jigsaw piece `{piece_type}` has no `pool_element`"))?;
    let pool_element = pool_element_from_nbt(&element)?;
    let rotation = tag
        .string("rotation")
        .and_then(|name| rotation_from_name(name.to_str().as_ref()))
        .ok_or_else(|| format!("jigsaw piece `{piece_type}` has no known `rotation`"))?;
    let liquid_settings = match tag.string("liquid_settings") {
        None => LIQUID_SETTINGS_APPLY,
        Some(name) => liquid_settings_from_name(name.to_str().as_ref()).ok_or_else(|| {
            format!(
                "jigsaw piece `{piece_type}` has unknown `liquid_settings` `{}`",
                name.to_str()
            )
        })?,
    };
    let mut junctions = Vec::new();
    for junction in tag.list("junctions").map_or_else(Vec::new, compound_list) {
        junctions.push(junction_from_nbt(&junction, piece_type)?);
    }
    let projection = pool_element_projection(&pool_element);
    Ok((
        PersistentStructurePiecePayload::Jigsaw(PersistentJigsawPieceData {
            pool_element,
            position: [
                tag.int("PosX").unwrap_or(0),
                tag.int("PosY").unwrap_or(0),
                tag.int("PosZ").unwrap_or(0),
            ],
            rotation,
            liquid_settings,
        }),
        tag.int("ground_level_delta").unwrap_or(0),
        projection,
        junctions,
    ))
}

/// The projection a persisted pool element carries; `Empty` has none, so it behaves as `RIGID`.
const fn pool_element_projection(element: &PersistentPoolElement) -> i8 {
    match element {
        PersistentPoolElement::Single { projection, .. }
        | PersistentPoolElement::LegacySingle { projection, .. }
        | PersistentPoolElement::Feature { projection, .. }
        | PersistentPoolElement::List { projection, .. } => *projection,
        PersistentPoolElement::Empty => PROJECTION_RIGID,
    }
}

/// Decodes one `JigsawJunction.serialize` compound.
fn junction_from_nbt(
    tag: &BorrowedNbtCompound<'_, '_>,
    piece_type: &Identifier,
) -> Result<PersistentJigsawJunction, String> {
    let dest_projection = tag
        .string("dest_proj")
        .and_then(|name| projection_from_name(name.to_str().as_ref()))
        .ok_or_else(|| format!("jigsaw piece `{piece_type}` has an unknown `dest_proj`"))?;
    Ok(PersistentJigsawJunction {
        source_x: tag.int("source_x").unwrap_or(0),
        source_ground_y: tag.int("source_ground_y").unwrap_or(0),
        source_z: tag.int("source_z").unwrap_or(0),
        delta_y: tag.int("delta_y").unwrap_or(0),
        dest_projection,
    })
}

/// Decodes a `StructurePoolElement.CODEC` payload.
fn pool_element_from_nbt(
    tag: &BorrowedNbtCompound<'_, '_>,
) -> Result<PersistentPoolElement, String> {
    let element_type = tag
        .string("element_type")
        .map(|value| value.to_str().into_owned())
        .ok_or_else(|| "pool element has no `element_type`".to_owned())?;
    let element_type = Identifier::from_str(&element_type)
        .map_err(|_| format!("pool element `element_type` `{element_type}` is not an identifier"))?
        .to_string();
    let projection = match tag.string("projection") {
        None => PROJECTION_RIGID,
        Some(name) => projection_from_name(name.to_str().as_ref())
            .ok_or_else(|| format!("pool element has unknown projection `{}`", name.to_str()))?,
    };
    match element_type.as_str() {
        POOL_ELEMENT_SINGLE => Ok(PersistentPoolElement::Single {
            location: identifier_field(tag, "location", &element_type)?,
            processors: processor_list_from_nbt(tag),
            projection,
        }),
        POOL_ELEMENT_LEGACY_SINGLE => Ok(PersistentPoolElement::LegacySingle {
            location: identifier_field(tag, "location", &element_type)?,
            processors: processor_list_from_nbt(tag),
            projection,
        }),
        POOL_ELEMENT_FEATURE => Ok(PersistentPoolElement::Feature {
            feature: identifier_field(tag, "feature", &element_type)?,
            projection,
        }),
        POOL_ELEMENT_EMPTY => Ok(PersistentPoolElement::Empty),
        POOL_ELEMENT_LIST => {
            let mut elements = Vec::new();
            for element in tag.list("elements").map_or_else(Vec::new, compound_list) {
                elements.push(pool_element_from_nbt(&element)?);
            }
            Ok(PersistentPoolElement::List {
                elements,
                projection,
            })
        }
        _ => Err(format!("unsupported pool element type `{element_type}`")),
    }
}

/// Reads a required identifier-valued key.
fn identifier_field(
    tag: &BorrowedNbtCompound<'_, '_>,
    key: &str,
    context: &str,
) -> Result<Identifier, String> {
    tag.string(key)
        .and_then(|value| Identifier::from_str(value.to_str().as_ref()).ok())
        .ok_or_else(|| format!("`{context}` has no parsable `{key}`"))
}

/// Decodes a pool element's `processors` holder.
fn processor_list_from_nbt(tag: &BorrowedNbtCompound<'_, '_>) -> PersistentProcessorList {
    match tag.string("processors") {
        None => PersistentProcessorList::Empty,
        Some(name) => {
            let name = name.to_str();
            if name.as_ref() == EMPTY_PROCESSOR_LIST {
                PersistentProcessorList::Empty
            } else {
                Identifier::from_str(name.as_ref()).map_or(
                    PersistentProcessorList::Empty,
                    PersistentProcessorList::Registry,
                )
            }
        }
    }
}

/// Decodes a template piece's payload, rebuilding the placement fields the vanilla constructor
/// derives rather than stores.
#[expect(
    clippy::too_many_lines,
    reason = "mirrors the vanilla template piece payload codec 1:1, including the placement fields the vanilla constructor derives rather than stores; splitting it would break the correspondence"
)]
fn template_payload_from_nbt(
    piece_type: &str,
    tag: &BorrowedNbtCompound<'_, '_>,
) -> Result<PersistentTemplatePieceData, String> {
    let template_id = identifier_field(tag, "Template", piece_type)?;
    let template_position = [
        tag.int("TPX").unwrap_or(0),
        tag.int("TPY").unwrap_or(0),
        tag.int("TPZ").unwrap_or(0),
    ];
    let rotation = required_rotation(tag, piece_type)?;
    let mut data = PersistentTemplatePieceData {
        template_id,
        template_position,
        rotation,
        mirror: MIRROR_NONE,
        rotation_pivot: [0, 0, 0],
        block_ignore: BLOCK_IGNORE_NONE,
        late_block_ignore: BLOCK_IGNORE_NONE,
        processors: PersistentTemplateProcessorList::Empty,
        liquid_settings: LIQUID_SETTINGS_APPLY,
        marker_handling: MARKER_IGNORE,
        placement_adjustment: PersistentTemplatePlacementAdjustment::None,
        placement_clip: CLIP_CENTER_CHUNK,
        post_process: POST_PROCESS_NONE,
    };
    match piece_type {
        "minecraft:iglu" => {
            let (pivot, offset, post_process) = igloo_template_constants(&data.template_id);
            data.rotation_pivot = pivot;
            data.block_ignore = BLOCK_IGNORE_STRUCTURE_BLOCK;
            data.liquid_settings = LIQUID_SETTINGS_IGNORE;
            data.marker_handling = MARKER_IGLOO;
            data.placement_adjustment = PersistentTemplatePlacementAdjustment::Igloo {
                template_offset: offset,
            };
            data.post_process = post_process;
        }
        "minecraft:shipwreck" => {
            data.rotation_pivot = [4, 0, 15];
            data.block_ignore = BLOCK_IGNORE_STRUCTURE_AND_AIR;
            data.marker_handling = MARKER_SHIPWRECK;
            data.placement_adjustment = PersistentTemplatePlacementAdjustment::Shipwreck {
                is_beached: tag.byte("isBeached").is_some_and(|value| value != 0),
                height_adjusted: tag.byte("height_adjusted").is_some_and(|value| value != 0),
            };
        }
        "minecraft:orp" => {
            let integrity = tag.float("Integrity").unwrap_or(1.0);
            let biome_temp = tag
                .string("BiomeType")
                .and_then(|name| ocean_ruin_biome_temp_from_name(name.to_str().as_ref()))
                .unwrap_or(OCEAN_RUIN_WARM);
            let is_large = tag.byte("IsLarge").is_some_and(|value| value != 0);
            data.block_ignore = BLOCK_IGNORE_NONE;
            data.late_block_ignore = BLOCK_IGNORE_STRUCTURE_AND_AIR;
            data.processors = PersistentTemplateProcessorList::OceanRuin {
                biome_temp,
                integrity,
            };
            data.marker_handling = if is_large {
                MARKER_OCEAN_RUIN_LARGE
            } else {
                MARKER_OCEAN_RUIN_SMALL
            };
            data.placement_adjustment = PersistentTemplatePlacementAdjustment::OceanRuin;
        }
        "minecraft:rupo" => {
            data.mirror = tag
                .string("Mirror")
                .and_then(|name| mirror_from_name(name.to_str().as_ref()))
                .ok_or_else(|| "ruined portal piece has no known `Mirror`".to_owned())?;
            data.rotation = tag
                .string("Rotation")
                .and_then(|name| rotation_from_name(name.to_str().as_ref()))
                .ok_or_else(|| "ruined portal piece has no known `Rotation`".to_owned())?;
            let vertical_placement = tag
                .string("VerticalPlacement")
                .and_then(|name| vertical_placement_from_name(name.to_str().as_ref()))
                .ok_or_else(|| "ruined portal piece has no known `VerticalPlacement`".to_owned())?;
            let properties = tag
                .compound("Properties")
                .ok_or_else(|| "ruined portal piece has no `Properties`".to_owned())?;
            let air_pocket = properties
                .byte("air_pocket")
                .is_some_and(|value| value != 0);
            data.rotation_pivot = ruined_portal_rotation_pivot(&data.template_id);
            data.block_ignore = if air_pocket {
                BLOCK_IGNORE_STRUCTURE_BLOCK
            } else {
                BLOCK_IGNORE_STRUCTURE_AND_AIR
            };
            data.processors = PersistentTemplateProcessorList::RuinedPortal {
                vertical_placement,
                cold: properties.byte("cold").is_some_and(|value| value != 0),
                mossiness: properties.float("mossiness").unwrap_or(1.0),
                air_pocket,
                overgrown: properties.byte("overgrown").is_some_and(|value| value != 0),
                vines: properties.byte("vines").is_some_and(|value| value != 0),
                replace_with_blackstone: properties
                    .byte("replace_with_blackstone")
                    .is_some_and(|value| value != 0),
            };
            data.placement_clip = CLIP_CONTAINS_CENTER_EXPANDED_TO_TEMPLATE;
            data.post_process = POST_PROCESS_RUINED_PORTAL;
        }
        "minecraft:ecp" => {
            data.block_ignore = if tag.byte("OW").is_some_and(|value| value != 0) {
                BLOCK_IGNORE_STRUCTURE_BLOCK
            } else {
                BLOCK_IGNORE_STRUCTURE_AND_AIR
            };
            data.marker_handling = MARKER_END_CITY;
        }
        "minecraft:wmp" => {
            data.mirror = tag
                .string("Mi")
                .and_then(|name| mirror_from_name(name.to_str().as_ref()))
                .ok_or_else(|| "woodland mansion piece has no known `Mi`".to_owned())?;
            data.block_ignore = BLOCK_IGNORE_STRUCTURE_BLOCK;
            data.marker_handling = MARKER_WOODLAND_MANSION;
        }
        "minecraft:nefos" => {
            data.block_ignore = BLOCK_IGNORE_STRUCTURE_AND_AIR;
            data.placement_clip = CLIP_EXPANDED_TO_TEMPLATE;
            data.post_process = POST_PROCESS_NETHER_FOSSIL;
        }
        other => return Err(format!("unsupported template piece type `{other}`")),
    }
    Ok(data)
}

/// Reads the required `Rot` key of a template piece.
fn required_rotation(tag: &BorrowedNbtCompound<'_, '_>, piece_type: &str) -> Result<i8, String> {
    tag.string("Rot")
        .and_then(|name| rotation_from_name(name.to_str().as_ref()))
        .ok_or_else(|| format!("template piece `{piece_type}` has no known `Rot`"))
}

/// `IglooPieces.PIVOTS`/`OFFSETS` plus Steel's post-process marker for the template id.
fn igloo_template_constants(template_id: &Identifier) -> ([i32; 3], [i32; 3], i8) {
    match template_id.to_string().as_str() {
        "minecraft:igloo/top" => ([3, 5, 5], [0, 0, 0], POST_PROCESS_IGLOO_TOP),
        "minecraft:igloo/middle" => ([1, 3, 1], [2, -3, 4], POST_PROCESS_NONE),
        "minecraft:igloo/bottom" => ([3, 6, 7], [0, -3, -2], POST_PROCESS_NONE),
        _ => ([0, 0, 0], [0, 0, 0], POST_PROCESS_NONE),
    }
}

/// Vanilla derives a ruined portal's rotation pivot from the template size; a template that is
/// not bundled yields the empty template (size zero), matching vanilla's
/// `StructureTemplateManager.getOrCreate`.
fn ruined_portal_rotation_pivot(template_id: &Identifier) -> [i32; 3] {
    StructureTemplate::load_vanilla(&REGISTRY, template_id).map_or([0, 0, 0], |template| {
        let size = template.unrotated_size();
        [size.x / 2, 0, size.z / 2]
    })
}

/// Decodes a code-generated piece family's payload.
fn procedural_payload_from_nbt(
    piece_type: &str,
    tag: &BorrowedNbtCompound<'_, '_>,
) -> Result<PersistentProceduralPieceData, String> {
    let height_position = tag.int("HPos").unwrap_or(-1);
    Ok(match piece_type {
        "minecraft:tedp" => {
            PersistentProceduralPieceData::DesertPyramid(PersistentDesertPyramidPieceData {
                height_position,
                has_placed_chest: [0, 1, 2, 3].map(|index| {
                    tag.byte(&format!("hasPlacedChest{index}"))
                        .is_some_and(|value| value != 0)
                }),
            })
        }
        "minecraft:tejp" => {
            PersistentProceduralPieceData::JungleTemple(PersistentJungleTemplePieceData {
                height_position,
                placed_main_chest: flag(tag, "placedMainChest"),
                placed_hidden_chest: flag(tag, "placedHiddenChest"),
                placed_trap1: flag(tag, "placedTrap1"),
                placed_trap2: flag(tag, "placedTrap2"),
            })
        }
        "minecraft:tesh" => PersistentProceduralPieceData::SwampHut(PersistentSwampHutPieceData {
            height_position,
            spawned_witch: flag(tag, "Witch"),
            spawned_cat: flag(tag, "Cat"),
        }),
        "minecraft:btp" => PersistentProceduralPieceData::BuriedTreasure,
        "minecraft:msroom"
        | "minecraft:mscorridor"
        | "minecraft:mscrossing"
        | "minecraft:msstairs" => {
            PersistentProceduralPieceData::Mineshaft(PersistentMineshaftPieceData {
                mineshaft_type: tag.int("MST").unwrap_or(0) as i8,
                kind: mineshaft_kind_from_nbt(piece_type, tag)?,
            })
        }
        "minecraft:omb" => {
            warn_once(
                &WARNED_MONUMENT_CHILDREN,
                "Vanilla stores no ocean-monument rooms and regenerates them from the world seed, so a loaded monument piece carries no Steel child rooms",
            );
            PersistentProceduralPieceData::OceanMonument(PersistentOceanMonumentPieceData {
                child_pieces: Vec::new(),
            })
        }
        other if other.starts_with("minecraft:ne") => {
            PersistentProceduralPieceData::NetherFortress(fortress_payload_from_nbt(other, tag))
        }
        other if other.starts_with("minecraft:sh") => {
            PersistentProceduralPieceData::Stronghold(stronghold_payload_from_nbt(other, tag)?)
        }
        other => return Err(format!("unsupported procedural piece type `{other}`")),
    })
}

/// Reads a boolean key written with `CompoundTag.putBoolean`.
fn flag(tag: &BorrowedNbtCompound<'_, '_>, key: &str) -> bool {
    tag.byte(key).is_some_and(|value| value != 0)
}

/// Decodes one mineshaft piece kind (`MineShaftRoom`, `MineShaftCorridor`, ...).
fn mineshaft_kind_from_nbt(
    piece_type: &str,
    tag: &BorrowedNbtCompound<'_, '_>,
) -> Result<PersistentMineshaftPieceKind, String> {
    Ok(match piece_type {
        "minecraft:msroom" => {
            let mut child_entrance_boxes = Vec::new();
            if let Some(entrances) = tag.list("Entrances")
                && let Some(entries) = entrances.int_arrays()
            {
                for entry in entries {
                    let values = entry.to_vec();
                    if values.len() == 6 {
                        child_entrance_boxes.push(PersistentBoundingBox {
                            min_x: values[0],
                            min_y: values[1],
                            min_z: values[2],
                            max_x: values[3],
                            max_y: values[4],
                            max_z: values[5],
                        });
                    }
                }
            }
            PersistentMineshaftPieceKind::Room {
                child_entrance_boxes,
            }
        }
        "minecraft:mscorridor" => PersistentMineshaftPieceKind::Corridor {
            has_rails: flag(tag, "hr"),
            spider_corridor: flag(tag, "sc"),
            has_placed_spider: flag(tag, "hps"),
            num_sections: tag.int("Num").unwrap_or(0),
        },
        "minecraft:mscrossing" => {
            let direction = tag.byte("D").unwrap_or(0);
            if !(0..=3).contains(&direction) {
                return Err(format!("mineshaft crossing has `D` {direction}"));
            }
            PersistentMineshaftPieceKind::Crossing {
                direction,
                is_two_floored: flag(tag, "tf"),
            }
        }
        "minecraft:msstairs" => PersistentMineshaftPieceKind::Stairs,
        other => return Err(format!("unsupported mineshaft piece type `{other}`")),
    })
}

/// Decodes one nether-fortress piece kind.
fn fortress_payload_from_nbt(
    piece_type: &str,
    tag: &BorrowedNbtCompound<'_, '_>,
) -> PersistentNetherFortressPieceData {
    match piece_type {
        "minecraft:nebef" => PersistentNetherFortressPieceData::BridgeEndFiller {
            self_seed: tag.int("Seed").unwrap_or(0),
        },
        "minecraft:nebs" => PersistentNetherFortressPieceData::BridgeStraight,
        "minecraft:neccs" => PersistentNetherFortressPieceData::CastleCorridorStairs,
        "minecraft:nectb" => PersistentNetherFortressPieceData::CastleCorridorTBalcony,
        "minecraft:nece" => PersistentNetherFortressPieceData::CastleEntrance,
        "minecraft:nescsc" => PersistentNetherFortressPieceData::CastleSmallCorridorCrossing,
        "minecraft:nesclt" => PersistentNetherFortressPieceData::CastleSmallCorridorLeftTurn {
            is_needing_chest: flag(tag, "Chest"),
        },
        "minecraft:nesc" => PersistentNetherFortressPieceData::CastleSmallCorridor,
        "minecraft:nescrt" => PersistentNetherFortressPieceData::CastleSmallCorridorRightTurn {
            is_needing_chest: flag(tag, "Chest"),
        },
        "minecraft:necsr" => PersistentNetherFortressPieceData::CastleStalkRoom,
        "minecraft:nemt" => PersistentNetherFortressPieceData::MonsterThrone {
            has_placed_spawner: flag(tag, "Mob"),
        },
        "minecraft:nerc" => PersistentNetherFortressPieceData::RoomCrossing,
        "minecraft:nesr" => PersistentNetherFortressPieceData::StairsRoom,
        // Unreachable: `procedural_payload_from_nbt` only routes nether-fortress ids here.
        _ => PersistentNetherFortressPieceData::BridgeCrossing,
    }
}

/// Decodes one stronghold piece kind.
fn stronghold_payload_from_nbt(
    piece_type: &str,
    tag: &BorrowedNbtCompound<'_, '_>,
) -> Result<PersistentStrongholdPieceData, String> {
    let entry_door = tag
        .string("EntryDoor")
        .and_then(|name| stronghold_door_from_name(name.to_str().as_ref()))
        .ok_or_else(|| format!("stronghold piece `{piece_type}` has no known `EntryDoor`"))?;
    Ok(match piece_type {
        "minecraft:shstart" | "minecraft:shsd" => PersistentStrongholdPieceData::StairsDown {
            entry_door,
            is_source: flag(tag, "Source"),
        },
        "minecraft:shs" => PersistentStrongholdPieceData::Straight {
            entry_door,
            left_child: flag(tag, "Left"),
            right_child: flag(tag, "Right"),
        },
        "minecraft:shph" => PersistentStrongholdPieceData::PrisonHall { entry_door },
        "minecraft:shlt" => PersistentStrongholdPieceData::LeftTurn { entry_door },
        "minecraft:shrt" => PersistentStrongholdPieceData::RightTurn { entry_door },
        "minecraft:shrc" => PersistentStrongholdPieceData::RoomCrossing {
            entry_door,
            crossing_type: tag.int("Type").unwrap_or(0),
        },
        "minecraft:shssd" => PersistentStrongholdPieceData::StraightStairsDown { entry_door },
        "minecraft:sh5c" => PersistentStrongholdPieceData::FiveCrossing {
            entry_door,
            left_low: flag(tag, "leftLow"),
            left_high: flag(tag, "leftHigh"),
            right_low: flag(tag, "rightLow"),
            right_high: flag(tag, "rightHigh"),
        },
        "minecraft:shcc" => PersistentStrongholdPieceData::ChestCorridor {
            entry_door,
            has_placed_chest: flag(tag, "Chest"),
        },
        "minecraft:shli" => PersistentStrongholdPieceData::Library {
            entry_door,
            is_tall: flag(tag, "Tall"),
        },
        "minecraft:shpr" => PersistentStrongholdPieceData::PortalRoom {
            has_placed_spawner: flag(tag, "Mob"),
        },
        "minecraft:shfc" => PersistentStrongholdPieceData::FillerCorridor {
            steps: tag.int("Steps").unwrap_or(0),
        },
        other => return Err(format!("unsupported stronghold piece type `{other}`")),
    })
}

/// Accumulates chunk-level block-state and biome palettes while decoding sections.
struct PaletteBuilder {
    block_states: Vec<PersistentBlockState<'static>>,
    biomes: Vec<Identifier>,
}

impl PaletteBuilder {
    const fn new() -> Self {
        Self {
            block_states: Vec::new(),
            biomes: Vec::new(),
        }
    }

    fn block_index(&mut self, state: BlockStateId) -> u16 {
        let persistent = PersistentBlockState {
            name: state.get_block().key.clone(),
            properties: REGISTRY.blocks.properties(state).collect(),
        };
        if let Some(index) = self
            .block_states
            .iter()
            .position(|entry| *entry == persistent)
        {
            return index as u16;
        }
        let index = self.block_states.len();
        self.block_states.push(persistent);
        index as u16
    }

    fn biome_index(&mut self, biome_id: u16) -> io::Result<u16> {
        let biome = REGISTRY
            .biomes
            .by_id(usize::from(biome_id))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("biome id {biome_id} is not registered"),
                )
            })?;
        if let Some(index) = self.biomes.iter().position(|key| *key == biome.key) {
            return Ok(index as u16);
        }
        let index = self.biomes.len();
        self.biomes.push(biome.key.clone());
        Ok(index as u16)
    }
}

const fn ceil_log2(value: u32) -> u8 {
    if value <= 1 {
        0
    } else {
        (32 - (value - 1).leading_zeros()) as u8
    }
}

/// Vanilla `PalettedContainer` on-disk width for a block-state palette.
const fn block_state_storage_bits(palette_len: usize) -> u8 {
    let bits = ceil_log2(palette_len as u32);
    if bits == 0 {
        0
    } else if bits <= 4 {
        4
    } else {
        bits
    }
}

/// Vanilla `PalettedContainer` on-disk width for a biome palette.
const fn biome_storage_bits(palette_len: usize) -> u8 {
    ceil_log2(palette_len as u32)
}

/// Heightmap values need `ceil_log2(height + 1)` bits in vanilla.
const fn heightmap_bits_for_height(height: i32) -> u8 {
    ceil_log2(height as u32 + 1)
}

fn unpack_vanilla(data: &[i64], bits: u8, count: usize) -> Vec<u32> {
    if bits == 0 {
        return Vec::new();
    }
    let bits = usize::from(bits);
    let values_per_word = 64 / bits;
    let mask = (1u64 << bits) - 1;
    let mut out = Vec::with_capacity(count);
    for word in data {
        let word = *word as u64;
        for slot in 0..values_per_word {
            if out.len() == count {
                return out;
            }
            out.push(((word >> (slot * bits)) & mask) as u32);
        }
    }
    out
}

fn pack_vanilla(indices: impl Iterator<Item = u32>, bits: u8, count: usize) -> Vec<i64> {
    if bits == 0 || count == 0 {
        return Vec::new();
    }
    let bits = usize::from(bits);
    let values_per_word = 64 / bits;
    let mask = (1u64 << bits) - 1;
    let mut out = vec![0i64; count.div_ceil(values_per_word)];
    for (index, value) in indices.enumerate().take(count) {
        out[index / values_per_word] |=
            ((u64::from(value) & mask) << ((index % values_per_word) * bits)) as i64;
    }
    out
}

const fn heightmap_key(map_type: HeightmapType) -> &'static str {
    match map_type {
        HeightmapType::WorldSurface => "WORLD_SURFACE",
        HeightmapType::MotionBlocking => "MOTION_BLOCKING",
        HeightmapType::MotionBlockingNoLeaves => "MOTION_BLOCKING_NO_LEAVES",
        HeightmapType::OceanFloor => "OCEAN_FLOOR",
        HeightmapType::WorldSurfaceWg => "WORLD_SURFACE_WG",
        HeightmapType::OceanFloorWg => "OCEAN_FLOOR_WG",
    }
}

fn warn_once(flag: &AtomicBool, message: &str) {
    if !flag.swap(true, Ordering::Relaxed) {
        tracing::warn!("{message}");
    }
}

/// Round-trip tests for the `structures.starts` codec.
///
/// The expected values encode vanilla's schema (e.g. `Rotation.LEGACY_CODEC` spelling `Rot` as
/// the uppercase enum name), read from the vanilla jar, not from memory.
#[cfg(test)]
mod structure_start_tests {
    use std::io::Cursor;
    use std::slice;

    use simdnbt::borrow::NbtCompound as BorrowedNbtCompound;
    use simdnbt::borrow::read_compound;
    use steel_registry::init_vanilla_registry;
    use steel_utils::Identifier;

    use super::*;
    use crate::chunk_saver::anvil::nbt::write_raw_nbt;

    /// Re-parses encoded starts the way `nbt_to_chunk` sees them: bytes out, bytes back in.
    fn decode_starts(
        start: &PersistentStructureStart,
        pos: ChunkPos,
    ) -> Vec<PersistentStructureStart> {
        let mut structures = NbtCompound::new();
        structures.insert(
            "starts",
            NbtTag::Compound(structure_starts_to_nbt(slice::from_ref(start))),
        );
        let bytes = write_raw_nbt(&structures);
        let mut cursor = Cursor::new(bytes.as_slice());
        let parsed = read_compound(&mut cursor).expect("writes valid NBT");
        let borrowed: BorrowedNbtCompound<'_, '_> = BorrowedNbtCompound::from(&parsed);
        structure_starts_from_nbt(&borrowed, pos)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one round-trip fixture covering every structure-NBT payload family; splitting it would break the single-table correspondence with the codec"
    )]
    #[test]
    fn structure_starts_round_trip_all_payload_families() {
        let _ = init_vanilla_registry();
        let start = PersistentStructureStart {
            structure: Identifier::new_static("minecraft", "desert_pyramid"),
            chunk_x: 3,
            chunk_z: -7,
            references: 2,
            pieces: vec![
                PersistentStructurePiece {
                    piece_type: Identifier::new_static("minecraft", "jigsaw"),
                    bounding_box: PersistentBoundingBox {
                        min_x: 10,
                        min_y: 60,
                        min_z: 10,
                        max_x: 21,
                        max_y: 70,
                        max_z: 21,
                    },
                    gen_depth: 1,
                    orientation: 2,
                    payload: PersistentStructurePiecePayload::Jigsaw(PersistentJigsawPieceData {
                        pool_element: PersistentPoolElement::Single {
                            location: Identifier::new_static(
                                "minecraft",
                                "village/plains/town_centers/plains_meeting_point_1",
                            ),
                            processors: PersistentProcessorList::Registry(Identifier::new_static(
                                "minecraft",
                                "mossify_10_percent",
                            )),
                            projection: 1,
                        },
                        position: [4, 61, 4],
                        rotation: 2,
                        liquid_settings: 1,
                    }),
                    ground_level_delta: -3,
                    projection: 1,
                    junctions: vec![PersistentJigsawJunction {
                        source_x: 10,
                        source_ground_y: 63,
                        source_z: 10,
                        delta_y: 2,
                        dest_projection: 1,
                    }],
                },
                PersistentStructurePiece {
                    piece_type: Identifier::new_static("minecraft", "iglu"),
                    bounding_box: PersistentBoundingBox {
                        min_x: 0,
                        min_y: 64,
                        min_z: 0,
                        max_x: 6,
                        max_y: 68,
                        max_z: 7,
                    },
                    gen_depth: 0,
                    orientation: 2,
                    payload: PersistentStructurePiecePayload::Template(
                        PersistentTemplatePieceData {
                            template_id: Identifier::new_static("minecraft", "igloo/top"),
                            template_position: [16, 90, 16],
                            rotation: 1,
                            mirror: 0,
                            rotation_pivot: [3, 5, 5],
                            block_ignore: 1,
                            late_block_ignore: 0,
                            processors: PersistentTemplateProcessorList::Empty,
                            liquid_settings: 1,
                            marker_handling: 3,
                            placement_adjustment: PersistentTemplatePlacementAdjustment::Igloo {
                                template_offset: [0, 0, 0],
                            },
                            placement_clip: 0,
                            post_process: 2,
                        },
                    ),
                    ground_level_delta: 0,
                    projection: -1,
                    junctions: Vec::new(),
                },
                PersistentStructurePiece {
                    piece_type: Identifier::new_static("minecraft", "tedp"),
                    bounding_box: PersistentBoundingBox {
                        min_x: 0,
                        min_y: 64,
                        min_z: 0,
                        max_x: 20,
                        max_y: 78,
                        max_z: 20,
                    },
                    gen_depth: 0,
                    orientation: 3,
                    payload: PersistentStructurePiecePayload::Procedural(
                        PersistentProceduralPieceData::DesertPyramid(
                            PersistentDesertPyramidPieceData {
                                height_position: 63,
                                has_placed_chest: [true, false, false, true],
                            },
                        ),
                    ),
                    ground_level_delta: 0,
                    projection: -1,
                    junctions: Vec::new(),
                },
                PersistentStructurePiece {
                    piece_type: Identifier::new_static("minecraft", "mscorridor"),
                    bounding_box: PersistentBoundingBox {
                        min_x: 0,
                        min_y: 20,
                        min_z: 0,
                        max_x: 4,
                        max_y: 24,
                        max_z: 29,
                    },
                    gen_depth: 2,
                    orientation: 0,
                    payload: PersistentStructurePiecePayload::Procedural(
                        PersistentProceduralPieceData::Mineshaft(PersistentMineshaftPieceData {
                            mineshaft_type: 1,
                            kind: PersistentMineshaftPieceKind::Corridor {
                                has_rails: true,
                                spider_corridor: false,
                                has_placed_spider: true,
                                num_sections: 3,
                            },
                        }),
                    ),
                    ground_level_delta: 0,
                    projection: -1,
                    junctions: Vec::new(),
                },
                PersistentStructurePiece {
                    piece_type: Identifier::new_static("minecraft", "nebef"),
                    bounding_box: PersistentBoundingBox {
                        min_x: 0,
                        min_y: 40,
                        min_z: 0,
                        max_x: 8,
                        max_y: 50,
                        max_z: 12,
                    },
                    gen_depth: 3,
                    orientation: 1,
                    payload: PersistentStructurePiecePayload::Procedural(
                        PersistentProceduralPieceData::NetherFortress(
                            PersistentNetherFortressPieceData::BridgeEndFiller { self_seed: 7 },
                        ),
                    ),
                    ground_level_delta: 0,
                    projection: -1,
                    junctions: Vec::new(),
                },
                PersistentStructurePiece {
                    piece_type: Identifier::new_static("minecraft", "shcc"),
                    bounding_box: PersistentBoundingBox {
                        min_x: 4,
                        min_y: 20,
                        min_z: 4,
                        max_x: 8,
                        max_y: 24,
                        max_z: 12,
                    },
                    gen_depth: 4,
                    orientation: 2,
                    payload: PersistentStructurePiecePayload::Procedural(
                        PersistentProceduralPieceData::Stronghold(
                            PersistentStrongholdPieceData::ChestCorridor {
                                entry_door: PersistentStrongholdSmallDoorType::IronDoor,
                                has_placed_chest: true,
                            },
                        ),
                    ),
                    ground_level_delta: 0,
                    projection: -1,
                    junctions: Vec::new(),
                },
            ],
        };

        let decoded = decode_starts(&start, ChunkPos::new(3, -7));
        assert_eq!(decoded, vec![start]);
    }

    /// Asserts the emitted tag uses vanilla's key names and types, read from the jar:
    /// `BB` is an int array, `O`/`GD` are ints, `Rot`/`rotation`/`projection` are strings
    /// spelled the way each vanilla codec spells them, and `junctions` is a list of compounds.
    #[expect(
        clippy::too_many_lines,
        reason = "one fixture asserting every vanilla structure-start key and type; splitting it would break the single-table correspondence with the jar's schema"
    )]
    #[test]
    fn structure_start_tag_uses_vanilla_keys_and_types() {
        let _ = init_vanilla_registry();
        let start = PersistentStructureStart {
            structure: Identifier::new_static("minecraft", "desert_pyramid"),
            chunk_x: 1,
            chunk_z: 2,
            references: 5,
            pieces: vec![
                PersistentStructurePiece {
                    piece_type: Identifier::new_static("minecraft", "jigsaw"),
                    bounding_box: PersistentBoundingBox {
                        min_x: -1,
                        min_y: 60,
                        min_z: 2,
                        max_x: 10,
                        max_y: 70,
                        max_z: 13,
                    },
                    gen_depth: 1,
                    orientation: -1,
                    payload: PersistentStructurePiecePayload::Jigsaw(PersistentJigsawPieceData {
                        pool_element: PersistentPoolElement::Single {
                            location: Identifier::new_static(
                                "minecraft",
                                "village/plains/houses/plains_small_house_1",
                            ),
                            processors: PersistentProcessorList::Empty,
                            projection: 1,
                        },
                        position: [0, 61, 0],
                        rotation: 2,
                        liquid_settings: 0,
                    }),
                    ground_level_delta: -1,
                    projection: 1,
                    junctions: vec![PersistentJigsawJunction {
                        source_x: 1,
                        source_ground_y: 62,
                        source_z: 1,
                        delta_y: 3,
                        dest_projection: 0,
                    }],
                },
                PersistentStructurePiece {
                    piece_type: Identifier::new_static("minecraft", "iglu"),
                    bounding_box: PersistentBoundingBox {
                        min_x: 0,
                        min_y: 64,
                        min_z: 0,
                        max_x: 6,
                        max_y: 68,
                        max_z: 7,
                    },
                    gen_depth: 0,
                    orientation: 2,
                    payload: PersistentStructurePiecePayload::Template(
                        PersistentTemplatePieceData {
                            template_id: Identifier::new_static("minecraft", "igloo/top"),
                            template_position: [16, 90, 16],
                            rotation: 3,
                            mirror: 0,
                            rotation_pivot: [3, 5, 5],
                            block_ignore: 1,
                            late_block_ignore: 0,
                            processors: PersistentTemplateProcessorList::Empty,
                            liquid_settings: 1,
                            marker_handling: 3,
                            placement_adjustment: PersistentTemplatePlacementAdjustment::Igloo {
                                template_offset: [0, 0, 0],
                            },
                            placement_clip: 0,
                            post_process: 2,
                        },
                    ),
                    ground_level_delta: 0,
                    projection: -1,
                    junctions: Vec::new(),
                },
                PersistentStructurePiece {
                    piece_type: Identifier::new_static("minecraft", "tedp"),
                    bounding_box: PersistentBoundingBox {
                        min_x: 0,
                        min_y: 64,
                        min_z: 0,
                        max_x: 20,
                        max_y: 83,
                        max_z: 20,
                    },
                    gen_depth: 0,
                    orientation: 3,
                    payload: PersistentStructurePiecePayload::Procedural(
                        PersistentProceduralPieceData::DesertPyramid(
                            PersistentDesertPyramidPieceData {
                                height_position: 64,
                                has_placed_chest: [false, true, false, false],
                            },
                        ),
                    ),
                    ground_level_delta: 0,
                    projection: -1,
                    junctions: Vec::new(),
                },
            ],
        };

        let written = structure_starts_to_nbt(slice::from_ref(&start));
        let start_tag = written
            .compound("minecraft:desert_pyramid")
            .expect("start keyed by structure id");
        assert_eq!(
            start_tag.string("id").map(|id| id.to_str().into_owned()),
            Some("minecraft:desert_pyramid".to_owned())
        );
        assert_eq!(start_tag.int("ChunkX"), Some(1));
        assert_eq!(start_tag.int("ChunkZ"), Some(2));
        assert_eq!(start_tag.int("references"), Some(5));

        let children = start_tag.list("Children").expect("`Children` list");
        let pieces = children.compounds().expect("compound children");
        assert_eq!(pieces.len(), 3);

        let jigsaw = &pieces[0];
        assert_eq!(
            jigsaw.string("id").map(|id| id.to_str().into_owned()),
            Some("minecraft:jigsaw".to_owned())
        );
        // `BoundingBox.CODEC` is `Codec.INT_STREAM` over `minX, minY, minZ, maxX, maxY, maxZ`.
        assert_eq!(jigsaw.int_array("BB"), Some(&[-1, 60, 2, 10, 70, 13][..]));
        // `StructurePiece.createTag` writes `O` with `putInt` (2D data value, `-1` for none).
        assert_eq!(jigsaw.int("O"), Some(-1));
        assert_eq!(jigsaw.int("GD"), Some(1));
        assert_eq!(jigsaw.int("PosX"), Some(0));
        assert_eq!(jigsaw.int("PosY"), Some(61));
        assert_eq!(jigsaw.int("PosZ"), Some(0));
        assert_eq!(jigsaw.int("ground_level_delta"), Some(-1));
        // `Rotation.LEGACY_CODEC` is `ExtraCodecs.legacyEnum`: the enum constant name.
        assert_eq!(
            jigsaw
                .string("rotation")
                .map(|name| name.to_str().into_owned()),
            Some("CLOCKWISE_180".to_owned())
        );
        // `liquid_settings` matches `JigsawStructure.DEFAULT_LIQUID_SETTINGS`, so it is omitted.
        assert!(jigsaw.string("liquid_settings").is_none());
        let element = jigsaw.compound("pool_element").expect("`pool_element`");
        assert_eq!(
            element
                .string("element_type")
                .map(|value| value.to_str().into_owned()),
            Some("minecraft:single_pool_element".to_owned())
        );
        assert_eq!(
            element
                .string("location")
                .map(|value| value.to_str().into_owned()),
            Some("minecraft:village/plains/houses/plains_small_house_1".to_owned())
        );
        assert_eq!(
            element
                .string("processors")
                .map(|value| value.to_str().into_owned()),
            Some("minecraft:empty".to_owned())
        );
        // `Projection.CODEC` is `StringRepresentable`: the lowercase serialized name.
        assert_eq!(
            element
                .string("projection")
                .map(|value| value.to_str().into_owned()),
            Some("terrain_matching".to_owned())
        );
        let junctions = jigsaw
            .list("junctions")
            .and_then(|list| list.compounds().map(|list| list.first()))
            .flatten()
            .expect("one junction");
        assert_eq!(junctions.int("source_x"), Some(1));
        assert_eq!(junctions.int("source_ground_y"), Some(62));
        assert_eq!(junctions.int("source_z"), Some(1));
        assert_eq!(junctions.int("delta_y"), Some(3));
        assert_eq!(
            junctions
                .string("dest_proj")
                .map(|value| value.to_str().into_owned()),
            Some("rigid".to_owned())
        );

        let igloo = &pieces[1];
        assert_eq!(igloo.int("O"), Some(2));
        assert_eq!(
            igloo
                .string("Template")
                .map(|value| value.to_str().into_owned()),
            Some("minecraft:igloo/top".to_owned())
        );
        assert_eq!(igloo.int("TPX"), Some(16));
        assert_eq!(igloo.int("TPY"), Some(90));
        assert_eq!(igloo.int("TPZ"), Some(16));
        assert_eq!(
            igloo.string("Rot").map(|name| name.to_str().into_owned()),
            Some("COUNTERCLOCKWISE_90".to_owned())
        );

        let pyramid = &pieces[2];
        assert_eq!(pyramid.int("O"), Some(3));
        assert_eq!(pyramid.int("GD"), Some(0));
        // `ScatteredFeaturePiece` stores the same dimensions it built the box from.
        assert_eq!(pyramid.int("Width"), Some(21));
        assert_eq!(pyramid.int("Height"), Some(20));
        assert_eq!(pyramid.int("Depth"), Some(21));
        assert_eq!(pyramid.int("HPos"), Some(64));
        assert_eq!(pyramid.byte("hasPlacedChest1"), Some(1));
        assert_eq!(pyramid.byte("hasPlacedChest0"), Some(0));
    }
}
