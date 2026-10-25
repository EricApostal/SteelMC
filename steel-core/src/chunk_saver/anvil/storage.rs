//! Vanilla Anvil storage backend.
//!
//! Layout, matching vanilla's dimension folder:
//!
//! ```text
//! <world>/level.toml            Steel level data (native)
//! <world>/level.dat             vanilla level data (mirrored when enabled)
//! <world>/region/r.X.Z.mca      chunk NBT
//! <world>/entities/r.X.Z.mca    full-chunk entity lists
//! <world>/poi/r.X.Z.mca         point-of-interest sections
//! <world>/data/*                saved data
//! ```
//!
//! The backend exposes the same method surface as [`crate::chunk_saver::RegionManager`] and is
//! reached through `ChunkStorage::Anvil`. Full chunks keep their entities in `entities/*.mca`
//! (vanilla's `LEVELCHUNK` behaviour); protochunks keep them inline. Fields Steel cannot model
//! are dropped with an explicit warning — see `chunk.rs` for the list.

use std::collections::hash_map::Entry;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Weak;
use std::sync::atomic::{AtomicI32, Ordering};

use rustc_hash::FxHashMap;
use simdnbt::borrow::{BaseNbtCompound, read_compound as read_borrowed_compound};
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_registry::{REGISTRY, RegistryExt as _};
use steel_utils::version::DATA_VERSION;
use steel_utils::{ChunkPos, locks::AsyncRwLock};
use tokio::sync::oneshot;

use crate::chunk::status::ChunkStatus;
use crate::chunk_saver::format::RegionPos;
use crate::chunk_saver::{
    ChunkStorage, LoadedChunk, PersistentChunk, PreparedChunkSave, format::PersistentPoi,
};
use crate::datafixer::{DataFixType, ensure_current, is_refusal, version_of_borrowed};
use crate::world::World;

use super::chunk::{chunk_to_nbt, nbt_to_chunk, poi_type_at};
use super::entities::{entity_from_nbt, entity_to_nbt};
use super::nbt::{compound_list, write_raw_nbt};
use super::poi::{VanillaPoi, poi_from_nbt, poi_to_nbt};
use super::region::{AnvilRegionFile, chunk_slot, region_file_name};

/// Fallback minimum Y when `level.toml` is not available yet (overworld default).
const DEFAULT_MIN_Y: i32 = -64;
/// Fallback dimension height when `level.toml` is not available yet (overworld default).
const DEFAULT_HEIGHT: i32 = 384;

/// Disk backend storing a dimension in vanilla's Anvil format.
pub struct AnvilStorage {
    region_dir: PathBuf,
    entities_dir: PathBuf,
    poi_dir: PathBuf,
    min_y: AtomicI32,
    height: AtomicI32,
    regions: AsyncRwLock<FxHashMap<RegionPos, RegionHandle>>,
}

struct RegionHandle {
    region: AnvilRegionFile,
    entities: Option<AnvilRegionFile>,
    poi: Option<AnvilRegionFile>,
    loaded_chunk_count: usize,
}

impl AnvilStorage {
    /// Creates an Anvil backend rooted at a Steel world directory (which doubles as the vanilla
    /// dimension folder). The dimension height is read from `level.toml` when present, and
    /// refreshed from the live world by [`Self::load_chunk`].
    #[must_use]
    pub fn new(world_dir: impl Into<PathBuf>) -> Self {
        let world_dir = world_dir.into();
        let (min_y, height) =
            read_level_height(&world_dir).unwrap_or((DEFAULT_MIN_Y, DEFAULT_HEIGHT));
        Self {
            region_dir: world_dir.join("region"),
            entities_dir: world_dir.join("entities"),
            poi_dir: world_dir.join("poi"),
            min_y: AtomicI32::new(min_y),
            height: AtomicI32::new(height),
            regions: AsyncRwLock::new(FxHashMap::default()),
        }
    }

    /// Returns the directory holding `region/*.mca`.
    #[must_use]
    pub fn region_dir(&self) -> &Path {
        &self.region_dir
    }

    const fn region_pos(pos: ChunkPos) -> RegionPos {
        RegionPos::from_chunk(pos.0.x, pos.0.y)
    }

    async fn open_region(
        &self,
        region: RegionPos,
        create: bool,
    ) -> io::Result<Option<RegionHandle>> {
        let Some(region_file) =
            AnvilRegionFile::open(&self.region_dir, region.x, region.z, create).await?
        else {
            return Ok(None);
        };
        // Entity and POI regions are created on demand by the save path so that reading a
        // vanilla world never materializes empty side files.
        let entities = AnvilRegionFile::open(&self.entities_dir, region.x, region.z, false).await?;
        let poi = AnvilRegionFile::open(&self.poi_dir, region.x, region.z, false).await?;
        Ok(Some(RegionHandle {
            region: region_file,
            entities,
            poi,
            loaded_chunk_count: 0,
        }))
    }

    /// Acquires a chunk, opening (and creating) its region files and bumping the reference
    /// count. Returns whether the chunk already exists on disk.
    ///
    /// # Errors
    /// Propagates filesystem errors.
    ///
    /// # Panics
    /// Panics if the region handle is missing right after it was inserted.
    pub async fn acquire_chunk(&self, pos: ChunkPos) -> io::Result<bool> {
        let region = Self::region_pos(pos);
        let slot = chunk_slot(pos.0.x, pos.0.y);
        let mut regions = self.regions.write().await;
        if let Entry::Vacant(entry) = regions.entry(region) {
            let Some(handle) = self.open_region(region, true).await? else {
                return Ok(false);
            };
            entry.insert(handle);
        }
        let handle = regions.get_mut(&region).expect("region was just inserted");
        let exists = handle.region.contains(slot);
        handle.loaded_chunk_count += 1;
        Ok(exists)
    }

    /// Releases a chunk, closing the region files once no chunk needs them.
    ///
    /// # Errors
    /// Propagates filesystem errors.
    pub async fn release_chunk(&self, pos: ChunkPos) -> io::Result<()> {
        let region = Self::region_pos(pos);
        let mut regions = self.regions.write().await;
        {
            let Some(handle) = regions.get_mut(&region) else {
                return Ok(());
            };
            handle.loaded_chunk_count = handle.loaded_chunk_count.saturating_sub(1);
            if handle.loaded_chunk_count > 0 {
                return Ok(());
            }
        }
        let Some(mut handle) = regions.remove(&region) else {
            return Ok(());
        };
        handle.region.flush().await?;
        if let Some(entities) = handle.entities.as_mut() {
            entities.flush().await?;
        }
        if let Some(poi) = handle.poi.as_mut() {
            poi.flush().await?;
        }
        Ok(())
    }

    /// Checks whether a chunk record exists without reading its payload.
    ///
    /// # Errors
    /// Propagates filesystem errors.
    pub async fn chunk_exists(&self, pos: ChunkPos) -> io::Result<bool> {
        let region = Self::region_pos(pos);
        let slot = chunk_slot(pos.0.x, pos.0.y);
        {
            let regions = self.regions.read().await;
            if let Some(handle) = regions.get(&region) {
                return Ok(handle.region.contains(slot));
            }
        }
        let Some(file) = AnvilRegionFile::open(&self.region_dir, region.x, region.z, false).await?
        else {
            return Ok(false);
        };
        Ok(file.contains(slot))
    }

    /// Loads a chunk, merging the entity and POI side files like vanilla does.
    ///
    /// Returns `Ok(None)` when the chunk is absent or its payload was discarded as corrupt.
    ///
    /// # Errors
    /// Propagates filesystem and geometry errors.
    pub async fn load_chunk(
        &self,
        pos: ChunkPos,
        min_y: i32,
        height: i32,
        level: Weak<World>,
        thread_pool: &rayon::ThreadPool,
    ) -> io::Result<Option<LoadedChunk>> {
        if height <= 0 || height % 16 != 0 || min_y % 16 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "chunk world range must be section-aligned, got min_y={min_y}, height={height}"
                ),
            ));
        }
        self.min_y.store(min_y, Ordering::Relaxed);
        self.height.store(height, Ordering::Relaxed);

        let region = Self::region_pos(pos);
        let slot = chunk_slot(pos.0.x, pos.0.y);

        let (chunk_bytes, entity_bytes, poi_bytes) = {
            let mut regions = self.regions.write().await;
            let Some(handle) = regions.get_mut(&region) else {
                tracing::warn!("load_chunk called without acquire_chunk for region {region:?}");
                return Ok(None);
            };
            let chunk_bytes = match handle.region.read(slot).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    tracing::error!(?pos, "Failed to read Anvil chunk record: {error}");
                    return Ok(None);
                }
            };
            let entity_bytes = match handle.entities.as_mut() {
                Some(entities) => entities.read(slot).await.unwrap_or_else(|error| {
                    tracing::error!(?pos, "Failed to read Anvil entity record: {error}");
                    None
                }),
                None => None,
            };
            let poi_bytes = match handle.poi.as_mut() {
                Some(poi) => poi.read(slot).await.unwrap_or_else(|error| {
                    tracing::error!(?pos, "Failed to read Anvil POI record: {error}");
                    None
                }),
                None => None,
            };
            (chunk_bytes, entity_bytes, poi_bytes)
        };

        let Some(chunk_bytes) = chunk_bytes else {
            return Ok(None);
        };

        let source = format!(
            "{} at chunk {pos:?}",
            self.region_dir
                .join(region_file_name(region.x, region.z))
                .display()
        );

        let (sender, receiver) = oneshot::channel();
        thread_pool.spawn(move || {
            let result = decode_loaded_chunk(
                &chunk_bytes,
                entity_bytes.as_deref(),
                poi_bytes.as_deref(),
                pos,
                min_y,
                height,
                level,
                &source,
            );
            if sender.send(result).is_err() {
                tracing::trace!(
                    ?pos,
                    "Discarding decoded Anvil chunk after its load was canceled"
                );
            }
        });

        let decoded = receiver
            .await
            .map_err(|_| io::Error::other("chunk decode task ended without returning a result"))?;

        match decoded {
            Ok(loaded) => Ok(Some(loaded)),
            Err(error) => {
                // A version refusal is not corruption: the record is intact and is simply
                // unreadable here. Clearing the slot would delete a chunk we must not touch,
                // so the refusal propagates and the load is aborted instead.
                if is_refusal(&error) {
                    return Err(error);
                }
                tracing::error!(
                    ?pos,
                    "Discarding corrupt Anvil chunk and regenerating it: {error}"
                );
                let mut regions = self.regions.write().await;
                if let Some(handle) = regions.get_mut(&region) {
                    handle.region.clear(slot).await?;
                    handle.region.flush().await?;
                }
                Ok(None)
            }
        }
    }

    /// Saves a prepared chunk into the region file plus the entity and POI side files.
    ///
    /// # Errors
    /// Propagates encoding and filesystem errors.
    ///
    /// # Panics
    /// Panics if the region handle is missing right after it was inserted.
    pub async fn save_chunk_data(
        &self,
        prepared: PreparedChunkSave,
        thread_pool: &rayon::ThreadPool,
    ) -> io::Result<bool> {
        let pos = prepared.pos;
        let status = prepared.status;
        let min_y = self.min_y.load(Ordering::Relaxed);
        let height = self.height.load(Ordering::Relaxed);
        let region = Self::region_pos(pos);
        let slot = chunk_slot(pos.0.x, pos.0.y);

        let (sender, receiver) = oneshot::channel();
        thread_pool.spawn(move || {
            let result = encode_chunk(&prepared.persistent, pos, status, min_y, height);
            if sender.send(result).is_err() {
                tracing::trace!(
                    ?pos,
                    "Discarding encoded Anvil chunk after its save was canceled"
                );
            }
        });
        let (chunk_bytes, entity_bytes, poi_bytes) = receiver.await.map_err(|_| {
            io::Error::other("chunk encode task ended without returning a result")
        })??;

        let mut regions = self.regions.write().await;
        if let Entry::Vacant(entry) = regions.entry(region) {
            let Some(handle) = self.open_region(region, true).await? else {
                return Err(io::Error::other("could not create Anvil region files"));
            };
            entry.insert(handle);
        }
        let handle = regions.get_mut(&region).expect("region was just inserted");

        handle.region.write(slot, &chunk_bytes).await?;

        if let Some(entity_bytes) = entity_bytes {
            if handle.entities.is_none() {
                let opened = AnvilRegionFile::open(&self.entities_dir, region.x, region.z, true)
                    .await?
                    .ok_or_else(|| io::Error::other("could not create Anvil entity region file"))?;
                let _ = handle.entities.insert(opened);
            }
            handle
                .entities
                .as_mut()
                .expect("entity region was just opened")
                .write(slot, &entity_bytes)
                .await?;
        }

        if let Some(poi_bytes) = poi_bytes {
            if handle.poi.is_none() {
                let opened = AnvilRegionFile::open(&self.poi_dir, region.x, region.z, true)
                    .await?
                    .ok_or_else(|| io::Error::other("could not create Anvil POI region file"))?;
                let _ = handle.poi.insert(opened);
            }
            handle
                .poi
                .as_mut()
                .expect("POI region was just opened")
                .write(slot, &poi_bytes)
                .await?;
        } else if let Some(poi) = handle.poi.as_mut()
            && poi.contains(slot)
        {
            poi.clear(slot).await?;
        }

        Ok(true)
    }

    /// Flushes every open region header.
    ///
    /// # Errors
    /// Propagates filesystem errors.
    pub async fn flush_all(&self) -> io::Result<()> {
        let mut regions = self.regions.write().await;
        for handle in regions.values_mut() {
            handle.region.flush().await?;
            if let Some(entities) = handle.entities.as_mut() {
                entities.flush().await?;
            }
            if let Some(poi) = handle.poi.as_mut() {
                poi.flush().await?;
            }
        }
        Ok(())
    }

    /// Flushes and closes every open region file.
    ///
    /// # Errors
    /// Propagates filesystem errors.
    pub async fn close_all(&self) -> io::Result<()> {
        let mut regions = self.regions.write().await;
        let handles: Vec<RegionHandle> = regions.drain().map(|(_, handle)| handle).collect();
        drop(regions);
        for handle in handles {
            handle.region.close().await?;
            if let Some(entities) = handle.entities {
                entities.close().await?;
            }
            if let Some(poi) = handle.poi {
                poi.close().await?;
            }
        }
        Ok(())
    }
}

/// `(chunk NBT, full-chunk entity NBT, POI NBT)` for one chunk.
pub(crate) type EncodedChunk = (Vec<u8>, Option<Vec<u8>>, Option<Vec<u8>>);

#[expect(
    clippy::unnecessary_wraps,
    reason = "keeps the fallible signature of the native backend's `encode_chunk` so both save paths compose with `?`"
)]
pub(crate) fn encode_chunk(
    persistent: &PersistentChunk<'_>,
    pos: ChunkPos,
    status: ChunkStatus,
    min_y: i32,
    height: i32,
) -> io::Result<EncodedChunk> {
    let full = status == ChunkStatus::Full;
    let chunk_nbt = chunk_to_nbt(persistent, pos, status, min_y, height, !full);
    let entity_bytes = full.then(|| entities_to_nbt(persistent, pos));
    Ok((
        write_raw_nbt(&chunk_nbt),
        entity_bytes,
        poi_to_bytes(persistent, pos, min_y),
    ))
}

fn entities_to_nbt(persistent: &PersistentChunk<'_>, pos: ChunkPos) -> Vec<u8> {
    let mut nbt = NbtCompound::new();
    nbt.insert("DataVersion", DATA_VERSION);
    nbt.insert("Position", NbtTag::IntArray(vec![pos.0.x, pos.0.y]));
    nbt.insert(
        "Entities",
        NbtList::Compound(persistent.entities.iter().map(entity_to_nbt).collect()),
    );
    write_raw_nbt(&nbt)
}

fn poi_to_bytes(persistent: &PersistentChunk<'_>, pos: ChunkPos, min_y: i32) -> Option<Vec<u8>> {
    if persistent.pois.is_empty() {
        return None;
    }
    let entries: Vec<VanillaPoi> = persistent
        .pois
        .iter()
        .filter_map(|poi| {
            let Some(type_id) = poi_type_at(persistent, poi, min_y) else {
                tracing::warn!(
                    "Persistent POI at ({}, {}, {}) has no POI type for its block state; dropping it",
                    poi.x,
                    poi.y,
                    poi.z
                );
                return None;
            };
            let poi_type = REGISTRY.poi_types.by_id(type_id)?;
            let free_tickets = poi.free_tickets.min(poi_type.ticket_count);
            if free_tickets != poi.free_tickets {
                tracing::warn!(
                    "Persistent POI at ({}, {}, {}) had {} free tickets, above the {} allowed by {}; clamping",
                    poi.x,
                    poi.y,
                    poi.z,
                    poi.free_tickets,
                    poi_type.ticket_count,
                    poi_type.key
                );
            }
            Some(VanillaPoi {
                pos: [
                    pos.0.x * 16 + i32::from(poi.x),
                    i32::from(poi.y),
                    pos.0.y * 16 + i32::from(poi.z),
                ],
                poi_type: Some(poi_type.key.clone()),
                free_tickets,
            })
        })
        .collect();
    if entries.is_empty() {
        return None;
    }
    Some(write_raw_nbt(&poi_to_nbt(&entries)))
}

/// Decodes the chunk/entity/POI record triple at `pos` into a live chunk.
///
/// `source` labels the records for the version gate.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors the record triple plus the geometry and world the decode needs; \
              bundling them into a struct would only move the same fields elsewhere"
)]
fn decode_loaded_chunk(
    chunk_bytes: &[u8],
    entity_bytes: Option<&[u8]>,
    poi_bytes: Option<&[u8]>,
    pos: ChunkPos,
    min_y: i32,
    height: i32,
    level: Weak<World>,
    source: &str,
) -> io::Result<LoadedChunk> {
    let (persistent, status) = decode_persistent_chunk(
        chunk_bytes,
        entity_bytes,
        poi_bytes,
        pos,
        min_y,
        height,
        source,
    )?;
    ChunkStorage::try_persistent_to_chunk(&persistent, pos, status, min_y, height, level)
}

/// Decodes a vanilla chunk record (plus its entity and POI side records) into a persistent
/// chunk that owns every string it references.
pub(crate) fn decode_persistent_chunk(
    chunk_bytes: &[u8],
    entity_bytes: Option<&[u8]>,
    poi_bytes: Option<&[u8]>,
    pos: ChunkPos,
    min_y: i32,
    height: i32,
    source: &str,
) -> io::Result<(PersistentChunk<'static>, ChunkStatus)> {
    let (mut persistent, status) = {
        let borrowed = parse_root(chunk_bytes, "chunk")?;
        let view = simdnbt::borrow::NbtCompound::from(&borrowed);
        nbt_to_chunk(&view, pos, min_y, height, source)?
    };

    if let Some(entities) = entity_bytes {
        let borrowed = parse_root(entities, "entity")?;
        let view = simdnbt::borrow::NbtCompound::from(&borrowed);
        ensure_current(
            version_of_borrowed(&view),
            DataFixType::Entity,
            &format!("entity record of {source}"),
        )?;
        for entity in view.list("Entities").map_or_else(Vec::new, compound_list) {
            match entity_from_nbt(&entity) {
                Ok(entity) => persistent.entities.push(entity),
                Err(error) => {
                    tracing::warn!(?pos, "Dropping unparsable Anvil entity: {error}");
                }
            }
        }
    }

    if let Some(poi) = poi_bytes {
        let borrowed = parse_root(poi, "POI")?;
        let view = simdnbt::borrow::NbtCompound::from(&borrowed);
        ensure_current(
            version_of_borrowed(&view),
            DataFixType::Poi,
            &format!("POI record of {source}"),
        )?;
        for record in poi_from_nbt(&view)? {
            let x = record.pos[0] - pos.0.x * 16;
            let z = record.pos[2] - pos.0.y * 16;
            if !(0..16).contains(&x) || !(0..16).contains(&z) {
                tracing::warn!(
                    "Anvil POI record at {:?} lies outside chunk {pos:?}; dropping it",
                    record.pos
                );
                continue;
            }
            let Ok(y) = i16::try_from(record.pos[1]) else {
                tracing::warn!("Anvil POI record has a Y outside i16; dropping it");
                continue;
            };
            let mut entry = PersistentPoi {
                x: x as u8,
                y,
                z: z as u8,
                free_tickets: record.free_tickets,
            };
            let Some(derived) = poi_type_at(&persistent, &entry, min_y)
                .and_then(|type_id| REGISTRY.poi_types.by_id(type_id))
            else {
                tracing::warn!(
                    "Anvil POI at {:?} has no POI type for its block state; its ticket state is ignored",
                    record.pos
                );
                continue;
            };
            if let Some(recorded) = &record.poi_type
                && *recorded != derived.key
            {
                tracing::warn!(
                    "Anvil POI at {:?} records type {recorded} but its block state yields {}; using the block state",
                    record.pos,
                    derived.key
                );
            }
            if entry.free_tickets > derived.ticket_count {
                tracing::warn!(
                    "Anvil POI at {:?} has {} free tickets, above the {} allowed by {}; clamping",
                    record.pos,
                    entry.free_tickets,
                    derived.ticket_count,
                    derived.key
                );
                entry.free_tickets = derived.ticket_count;
            }
            persistent.pois.push(entry);
        }
    }

    Ok((persistent, status))
}

fn parse_root<'a>(bytes: &'a [u8], what: &str) -> io::Result<BaseNbtCompound<'a>> {
    read_borrowed_compound(&mut io::Cursor::new(bytes)).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed to parse Anvil {what} NBT: {error:?}"),
        )
    })
}

/// Reads `generation.min_y`/`generation.height` from a world's `level.toml`.
fn read_level_height(world_dir: &Path) -> Option<(i32, i32)> {
    let contents = fs::read_to_string(world_dir.join("level.toml")).ok()?;
    let parsed: toml::Value = toml::from_str(&contents).ok()?;
    let generation = parsed.get("generation")?;
    let min_y = i32::try_from(generation.get("min_y")?.as_integer()?).ok()?;
    let height = i32::try_from(generation.get("height")?.as_integer()?).ok()?;
    Some((min_y, height))
}
