//! Whole-world conversion between Steel's native `.srg` format and vanilla Anvil.
//!
//! `import_world` reads a vanilla world directory (`level.dat`, `region/`, `entities/`, `poi/`,
//! `data/`, `playerdata/`) and writes a Steel world directory (`level.toml`, `region/*.srg`,
//! `data/*.toml`, plus the domain's `<players>/data/<uuid>` files).
//! `export_world` does the reverse. Both directions reuse the live backend codecs, so conversion
//! and the running server cannot drift apart.
//!
//! Player data belongs to the whole domain, not one world: it is written under the domain's
//! `players/` directory next to the `worlds/` folder (see [`super::player_data`]).
//!
//! The dimension height (`min_y`/`height`) cannot be derived from region files alone, so the
//! caller supplies it: overworld `-64 / 384`, nether and end `0 / 256`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use steel_utils::{ChunkPos, Identifier};
use tokio::fs as tokio_fs;

use crate::chunk::status::ChunkStatus;
use crate::chunk_saver::format::{
    CHUNK_TABLE_SIZE, FILE_HEADER_SIZE, FORMAT_VERSION, PersistentChunk, REGION_MAGIC,
    RegionHeader, SECTOR_SIZE,
};
use crate::chunk_saver::{PreparedChunkSave, RegionManager};
use crate::entity::raid::RaidsData;
use crate::level_data::LevelData;
use crate::level_data::anvil;

use super::region::{AnvilRegionFile, chunk_slot, region_file_name, slot_chunk_pos};
use super::saved_data::{
    raids_from_nbt, raids_to_nbt, read_saved_data_file, wandering_trader_from_nbt,
    wandering_trader_to_nbt, weather_to_nbt, write_saved_data_file,
};
use super::storage::{EncodedChunk, decode_persistent_chunk, encode_chunk};

/// Summary of one conversion run.
#[derive(Debug, Default, Clone)]
pub struct AnvilConversionReport {
    /// Chunk columns converted.
    pub chunks: u64,
    /// Full-chunk entities carried across.
    pub entities: u64,
    /// Block entities carried across.
    pub block_entities: u64,
    /// POI ticket records carried across.
    pub pois: u64,
    /// Player data files converted.
    pub players: u64,
    /// Region files converted.
    pub regions: u64,
    /// Fields intentionally dropped (each one also warns once at conversion time).
    pub dropped: Vec<&'static str>,
}

impl AnvilConversionReport {
    fn new() -> Self {
        Self {
            dropped: vec![
                "chunk `InhabitedTime`, `UpgradeData`, `blending_data`, `below_zero_retrogen`",
                "structure starts (Steel stores typed piece payloads, not vanilla StructureStart NBT)",
                "Steel-internal light sections when writing Anvil",
                "`level.dat` bookkeeping Steel does not model (`DataPacks`, `ServerBrands`, `removed_features`, custom dimensions)",
                "saved data without a Steel model: `maps/`, `scoreboard`, `chunk_tickets`, `world_gen_settings` dimensions, command storage",
                "player keys without a Steel model (`Air`, `fall_distance`, `PortalCooldown`, `SleepTimer`, `sleeping_pos`, `LastDeathLocation`, `ShoulderEntityLeft`/`Right`, `warden_spawn_tracker`, `post_effects`, …); each file's present keys are logged and reported by `playerdata` conversion",
                "vanilla `stats/<uuid>.json` and `advancements/<uuid>.json` (separate files, not part of `playerdata/<uuid>.dat`; Steel keeps them in its own player files)",
            ],
            ..Self::default()
        }
    }
}

/// One `r.<x>.<z>.mca` region file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VanillaRegion {
    /// Region X coordinate.
    pub x: i32,
    /// Region Z coordinate.
    pub z: i32,
    /// File path.
    pub path: PathBuf,
}

/// Lists every vanilla region file in a directory, sorted by coordinates.
///
/// # Errors
/// Propagates directory read errors.
pub async fn list_vanilla_regions(directory: &Path) -> io::Result<Vec<VanillaRegion>> {
    let mut regions = Vec::new();
    let mut entries = match tokio_fs::read_dir(directory).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(regions),
        Err(error) => return Err(error),
    };
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        let Some((x, z)) = region_coords(&path, ".mca") else {
            continue;
        };
        regions.push(VanillaRegion { x, z, path });
    }
    regions.sort_by_key(|region| (region.x, region.z));
    Ok(regions)
}

fn region_coords(path: &Path, extension: &str) -> Option<(i32, i32)> {
    let name = path.file_name()?.to_str()?;
    let stem = name.strip_prefix("r.")?.strip_suffix(extension)?;
    let mut parts = stem.split('.');
    let x = parts.next()?.parse::<i32>().ok()?;
    let z = parts.next()?.parse::<i32>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((x, z))
}

/// Iterates every chunk of a Steel `.srg` region file, handing the decoded chunk to `visit`.
///
/// The decoded chunk borrows the decompressed record buffer, so it is only valid inside the
/// callback.
///
/// # Errors
/// Propagates filesystem, decompression, and decode errors.
pub fn for_each_steel_region_chunk(
    path: &Path,
    mut visit: impl FnMut(ChunkPos, ChunkStatus, &PersistentChunk<'_>) -> io::Result<()>,
) -> io::Result<()> {
    let bytes = fs::read(path)?;
    if bytes.len() < FILE_HEADER_SIZE + CHUNK_TABLE_SIZE {
        return Err(invalid(format!(
            "{} is too short to be a region file",
            path.display()
        )));
    }
    if bytes[0..4] != REGION_MAGIC {
        return Err(invalid(format!(
            "{} is not a Steel region file",
            path.display()
        )));
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version != FORMAT_VERSION {
        return Err(invalid(format!(
            "{} uses Steel region version {version}, expected {FORMAT_VERSION}",
            path.display()
        )));
    }
    let header =
        RegionHeader::from_bytes(&bytes[FILE_HEADER_SIZE..FILE_HEADER_SIZE + CHUNK_TABLE_SIZE])
            .map_err(|index| {
                invalid(format!(
                    "{} has an invalid chunk table entry {index}",
                    path.display()
                ))
            })?;
    let (region_x, region_z) = region_coords(path, ".srg").ok_or_else(|| {
        invalid(format!(
            "{} is not a Steel region file name",
            path.display()
        ))
    })?;

    for (index, entry) in header.entries.iter().enumerate() {
        if !entry.exists() {
            continue;
        }
        let start = entry.sector_offset as usize * SECTOR_SIZE;
        let end = start + entry.size_bytes as usize;
        if end > bytes.len() {
            return Err(invalid(format!(
                "{} chunk table entry {index} points past the file",
                path.display()
            )));
        }
        let decoded = zstd::decode_all(&bytes[start..end]).map_err(|error| {
            invalid(format!(
                "{} chunk {index} failed to decompress: {error}",
                path.display()
            ))
        })?;
        let persistent: PersistentChunk<'_> = wincode::deserialize(&decoded).map_err(|error| {
            invalid(format!(
                "{} chunk {index} failed to decode: {error}",
                path.display()
            ))
        })?;
        let (local_x, local_z) = RegionHeader::index_to_local(index);
        let pos = ChunkPos::new(
            region_x * 32 + local_x as i32,
            region_z * 32 + local_z as i32,
        );
        visit(pos, entry.status, &persistent)?;
    }
    Ok(())
}

/// Imports a vanilla world directory into a Steel world directory.
///
/// # Errors
/// Propagates filesystem and decode errors.
pub async fn import_world(
    vanilla_dir: &Path,
    steel_dir: &Path,
    min_y: i32,
    height: i32,
) -> io::Result<AnvilConversionReport> {
    let mut report = AnvilConversionReport::new();
    let pool = conversion_pool()?;
    let manager = RegionManager::new(steel_dir.join("region"));

    let entities_dir = vanilla_dir.join("entities");
    let poi_dir = vanilla_dir.join("poi");

    for region in list_vanilla_regions(&vanilla_dir.join("region")).await? {
        let mut region_file =
            AnvilRegionFile::open(&vanilla_dir.join("region"), region.x, region.z, false)
                .await?
                .ok_or_else(|| io::Error::other("region disappeared during conversion"))?;
        let mut entity_file =
            AnvilRegionFile::open(&entities_dir, region.x, region.z, false).await?;
        let mut poi_file = AnvilRegionFile::open(&poi_dir, region.x, region.z, false).await?;
        report.regions += 1;

        for slot in region_file.occupied_slots() {
            let Some(chunk_bytes) = region_file.read(slot).await? else {
                continue;
            };
            let entity_bytes = match entity_file.as_mut() {
                Some(file) => file.read(slot).await?,
                None => None,
            };
            let poi_bytes = match poi_file.as_mut() {
                Some(file) => file.read(slot).await?,
                None => None,
            };
            let pos = slot_chunk_pos(region.x, region.z, slot);
            let source = format!(
                "{} at chunk {pos:?}",
                vanilla_dir
                    .join("region")
                    .join(region_file_name(region.x, region.z))
                    .display()
            );
            let (persistent, status) = decode_persistent_chunk(
                &chunk_bytes,
                entity_bytes.as_deref(),
                poi_bytes.as_deref(),
                pos,
                min_y,
                height,
                &source,
            )?;
            report.entities += persistent.entities.len() as u64;
            report.block_entities += persistent.block_entities.len() as u64;
            report.pois += persistent.pois.len() as u64;
            manager
                .save_chunk_data(
                    PreparedChunkSave {
                        pos,
                        status,
                        persistent,
                        handled_runtime_entity_ids: Vec::new(),
                    },
                    &pool,
                )
                .await?;
            report.chunks += 1;
        }
    }

    import_level_data(vanilla_dir, steel_dir)?;
    let (players, player_unmapped) =
        super::player_data::import_player_data(vanilla_dir, steel_dir)?;
    report.players += players;
    for key in player_unmapped {
        if !report.dropped.contains(&key) {
            report.dropped.push(key);
        }
    }
    manager.close_all().await?;
    Ok(report)
}

/// Exports a Steel world directory into a vanilla world directory.
///
/// # Errors
/// Propagates filesystem and decode errors.
pub async fn export_world(
    steel_dir: &Path,
    vanilla_dir: &Path,
    min_y: i32,
    height: i32,
) -> io::Result<AnvilConversionReport> {
    let mut report = AnvilConversionReport::new();
    let region_dir = steel_dir.join("region");
    let mut steel_regions = Vec::new();
    let mut entries = tokio_fs::read_dir(&region_dir).await.map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} has no region directory", steel_dir.display()),
            )
        } else {
            error
        }
    })?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().is_some_and(|extension| extension == "srg") {
            steel_regions.push(path);
        }
    }
    steel_regions.sort();

    for path in steel_regions {
        let Some((region_x, region_z)) = region_coords(&path, ".srg") else {
            continue;
        };
        let mut encoded: Vec<(ChunkPos, EncodedChunk)> = Vec::new();
        for_each_steel_region_chunk(&path, |pos, status, persistent| {
            encoded.push((pos, encode_chunk(persistent, pos, status, min_y, height)?));
            Ok(())
        })?;
        report.regions += 1;
        if encoded.is_empty() {
            continue;
        }

        let mut region_file =
            AnvilRegionFile::open(&vanilla_dir.join("region"), region_x, region_z, true)
                .await?
                .ok_or_else(|| io::Error::other("could not create a vanilla region file"))?;
        let mut entity_file =
            AnvilRegionFile::open(&vanilla_dir.join("entities"), region_x, region_z, true).await?;
        let mut poi_file =
            AnvilRegionFile::open(&vanilla_dir.join("poi"), region_x, region_z, true).await?;

        for (pos, (chunk_bytes, entity_bytes, poi_bytes)) in encoded {
            let slot = chunk_slot(pos.0.x, pos.0.y);
            region_file.write(slot, &chunk_bytes).await?;
            if let (Some(file), Some(bytes)) = (entity_file.as_mut(), entity_bytes) {
                file.write(slot, &bytes).await?;
            }
            if let (Some(file), Some(bytes)) = (poi_file.as_mut(), poi_bytes) {
                file.write(slot, &bytes).await?;
            }
            report.chunks += 1;
        }
        region_file.flush().await?;
        if let Some(file) = entity_file.as_mut() {
            file.flush().await?;
        }
        if let Some(file) = poi_file.as_mut() {
            file.flush().await?;
        }
    }

    export_level_data(steel_dir, vanilla_dir)?;
    report.players += super::player_data::export_player_data(steel_dir, vanilla_dir)?;
    Ok(report)
}

fn import_level_data(vanilla_dir: &Path, steel_dir: &Path) -> io::Result<()> {
    let seed = anvil::read_seed(vanilla_dir)?.unwrap_or(0);
    let mut level = LevelData::new_with_seed(seed);
    if let Some(data) = anvil::read_level_dat(vanilla_dir)? {
        anvil::apply_level_dat(&data, vanilla_dir, &mut level)?;
        anvil::warn_unmapped_level_dat();
    }

    let data_dir = vanilla_dir.join("data");
    if let Some(raids) = read_saved_data_file(&data_dir.join("raids.dat"))? {
        write_toml(
            &steel_dir.join("data").join("raids.toml"),
            &raids_from_nbt(&raids),
        )?;
    }
    if let Some(wandering) = read_saved_data_file(&data_dir.join("wandering_trader.dat"))? {
        let (spawn_delay, spawn_chance) = wandering_trader_from_nbt(&wandering);
        write_toml(
            &steel_dir.join("data").join("wandering_trader.toml"),
            &WanderingTraderToml {
                spawn_delay,
                spawn_chance,
            },
        )?;
    }
    if let Some(rules) = read_saved_data_file(&data_dir.join("game_rules.dat"))? {
        level.game_rules = super::saved_data::game_rules_from_nbt(&rules);
        level.load_game_rules();
    }

    write_steel_level_toml(steel_dir, &level)
}

fn export_level_data(steel_dir: &Path, vanilla_dir: &Path) -> io::Result<()> {
    let Some(level) = read_steel_level_toml(steel_dir)? else {
        return Ok(());
    };
    let level_name = steel_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("world");
    let dimension = Identifier::vanilla_static("overworld");
    anvil::write_level_dat(vanilla_dir, &level, level_name, &dimension)?;
    anvil::write_dimension_saved_data(vanilla_dir, &level)?;
    write_saved_data_file(
        &vanilla_dir.join("data").join("weather.dat"),
        &weather_to_nbt(&level.weather),
    )?;

    if let Some(text) = read_text(&steel_dir.join("data").join("raids.toml"))? {
        let raids: RaidsData = toml::from_str(&text)
            .map_err(|error| invalid(format!("data/raids.toml is malformed: {error}")))?;
        write_saved_data_file(
            &vanilla_dir.join("data").join("raids.dat"),
            &raids_to_nbt(&raids),
        )?;
    }
    if let Some(text) = read_text(&steel_dir.join("data").join("wandering_trader.toml"))? {
        let parsed: WanderingTraderToml =
            toml::from_str(&text).map_err(|error| invalid(error.to_string()))?;
        write_saved_data_file(
            &vanilla_dir.join("data").join("wandering_trader.dat"),
            &wandering_trader_to_nbt(parsed.spawn_delay, parsed.spawn_chance),
        )?;
    }
    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize)]
struct WanderingTraderToml {
    spawn_delay: i32,
    spawn_chance: i32,
}

fn write_steel_level_toml(steel_dir: &Path, level: &LevelData) -> io::Result<()> {
    let text = toml::to_string_pretty(level)
        .map_err(|error| invalid(format!("could not encode level.toml: {error}")))?;
    fs::create_dir_all(steel_dir)?;
    fs::write(steel_dir.join("level.toml"), text)
}

fn read_steel_level_toml(steel_dir: &Path) -> io::Result<Option<LevelData>> {
    let Some(text) = read_text(&steel_dir.join("level.toml"))? else {
        return Ok(None);
    };
    toml::from_str(&text)
        .map(Some)
        .map_err(|error| invalid(format!("level.toml is malformed: {error}")))
}

fn write_toml(path: &Path, value: &impl serde::Serialize) -> io::Result<()> {
    let text = toml::to_string_pretty(value).map_err(|error| invalid(format!("{error}")))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, text)
}

fn read_text(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn conversion_pool() -> io::Result<rayon::ThreadPool> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .map_err(|error| {
            io::Error::other(format!(
                "could not build the conversion thread pool: {error}"
            ))
        })
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
