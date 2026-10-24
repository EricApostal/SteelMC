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
//!
//! Deliberately dropped with a warning (Steel has no model for them, and inventing values
//! would be worse than losing them): `UpgradeData`, `blending_data`, `below_zero_retrogen`,
//! `InhabitedTime`, and structure `starts` (Steel persists structure starts as typed piece
//! payloads, not as vanilla `StructureStart` NBT). Structure `References` convert both ways.

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
    PersistentBiomeData, PersistentBlockEntity, PersistentBlockState, PersistentChunk,
    PersistentEntity, PersistentHeightmap, PersistentLightData, PersistentLightSection,
    PersistentPoi, PersistentSection, PersistentStructureReference, PersistentTick,
};

use super::entities::{entity_from_nbt, entity_to_nbt};
use super::nbt::{compound_list, read_raw_nbt, write_raw_nbt};

/// Blocks in one section (vanilla `PalettedContainer` entry count for block states).
const BLOCKS_PER_SECTION: usize = 4096;
/// Biome cells in one section (vanilla `PalettedContainer` entry count for biomes).
const BIOMES_PER_SECTION: usize = 64;

static WARNED_UPGRADE_DATA: AtomicBool = AtomicBool::new(false);
static WARNED_BLENDING_DATA: AtomicBool = AtomicBool::new(false);
static WARNED_BELOW_ZERO: AtomicBool = AtomicBool::new(false);
static WARNED_STRUCTURE_STARTS: AtomicBool = AtomicBool::new(false);
static WARNED_INTERNAL_LIGHT: AtomicBool = AtomicBool::new(false);
static WARNED_Y_POS: AtomicBool = AtomicBool::new(false);

/// Returns the vanilla `Status` string for a Steel chunk status.
#[must_use]
pub(crate) fn status_name(status: ChunkStatus) -> &'static str {
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
pub(crate) fn chunk_to_nbt(
    persistent: &PersistentChunk<'_>,
    pos: ChunkPos,
    status: ChunkStatus,
    min_y: i32,
    height: i32,
    include_entities: bool,
) -> NbtCompound {
    if !persistent.structure_starts.is_empty() {
        warn_once(
            &WARNED_STRUCTURE_STARTS,
            "Steel structure starts have no vanilla `StructureStart` encoding and are dropped on Anvil write",
        );
    }

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
        structures_to_nbt(&persistent.structure_references),
    );
    nbt
}

/// Decodes vanilla chunk NBT into a persistent chunk and its status.
///
/// # Errors
/// Returns `InvalidData` for structurally invalid payloads (bad palette widths, malformed
/// records) so the caller can discard and regenerate the chunk.
pub(crate) fn nbt_to_chunk(
    nbt: &BorrowedNbtCompound<'_, '_>,
    pos: ChunkPos,
    min_y: i32,
    height: i32,
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
    crate::datafixer::ensure_current(
        crate::datafixer::version_of_borrowed(nbt),
        crate::datafixer::DataFixType::Chunk,
        &format!("chunk {pos:?}"),
    )?;
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
    if structures
        .as_ref()
        .and_then(|structures| structures.compound("starts"))
        .is_some_and(|starts| !starts.is_empty())
    {
        warn_once(
            &WARNED_STRUCTURE_STARTS,
            "Vanilla structure starts are dropped: Steel persists structure starts as typed piece payloads, not vanilla StructureStart NBT",
        );
    }

    let mut postprocessing = Vec::new();
    if let Some(lists) = nbt.list("PostProcessing").and_then(|list| list.lists()) {
        for index in 0..lists.len() {
            postprocessing.push(
                lists
                    .get(index)
                    .and_then(|list| list.shorts())
                    .map(|values| values.iter().map(|value| *value as u16).collect())
                    .unwrap_or_default(),
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
            structure_starts: Vec::new(),
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
        let biome_id = match key.and_then(|key| REGISTRY.biomes.id_from_key(&key)) {
            Some(biome) => biome,
            None => {
                tracing::warn!("Vanilla biome palette entry could not be resolved; using plains");
                vanilla_biomes::PLAINS.id()
            }
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
            .map(|index| match chunk.block_states.get(usize::from(*index)) {
                Some(state) => block_state_entry(state),
                None => {
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
            .map(|index| match chunk.biomes.get(usize::from(*index)) {
                Some(biome) => biome.to_string(),
                None => {
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
    fn len(&self) -> usize {
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
    let block_entity_type = match &entity.entity_type {
        Some(entity_type) => entity_type.clone(),
        None => {
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
        }
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

fn structures_to_nbt(references: &[PersistentStructureReference]) -> NbtTag {
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
    structures.insert("starts", NbtTag::Compound(NbtCompound::new()));
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

/// Accumulates chunk-level block-state and biome palettes while decoding sections.
struct PaletteBuilder {
    block_states: Vec<PersistentBlockState<'static>>,
    biomes: Vec<Identifier>,
}

impl PaletteBuilder {
    fn new() -> Self {
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

fn heightmap_key(map_type: HeightmapType) -> &'static str {
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
