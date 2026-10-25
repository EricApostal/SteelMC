//! Vanilla Anvil region container (`r.<x>.<z>.mca`).
//!
//! Layout (vanilla `RegionFile`):
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────┐
//! │ 0..4096    : 1024 × big-endian u32 "location"            │
//! │              (sector_number << 8) | sector_count         │
//! │ 4096..8192 : 1024 × big-endian u32 timestamp (epoch s)   │
//! ├──────────────────────────────────────────────────────────┤
//! │ sector 2.. : records, each 4096-byte aligned             │
//! │   big-endian i32 length (payload + 1, includes codec id) │
//! │   u8 compression id (bit 0x80 = payload lives in .mcc)   │
//! │   payload                                                │
//! └──────────────────────────────────────────────────────────┘
//! ```
//!
//! The 1024 location slots are indexed `local_z * 32 + local_x`, matching vanilla's
//! `ChunkPos.getRegionLocalZ() * 32 + getRegionLocalX()`.

use std::io::{self, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::fs::{self, File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use steel_utils::ChunkPos;

use super::compression::Compression;
use crate::chunk_saver::format::{REGION_SIZE, SECTOR_SIZE};

/// Number of location slots (and timestamps) in an Anvil region header.
pub const SECTOR_INTS: usize = 1024;
/// Size of the Anvil region header in bytes (1024 locations + 1024 timestamps).
pub const HEADER_BYTES: usize = 8192;
/// First sector usable for records; sectors 0 and 1 hold the header.
pub const FIRST_DATA_SECTOR: u32 = 2;
/// Size of the per-record prefix: `i32` length plus the compression id byte.
const RECORD_PREFIX_BYTES: usize = 5;
/// High bit of the compression id marking an external `.mcc` payload.
const EXTERNAL_STREAM_FLAG: u8 = 0x80;
/// Vanilla spills records of at least this many sectors into a `.mcc` file.
const EXTERNAL_CHUNK_THRESHOLD: u32 = 256;
/// Upper bound on a single record payload, mirroring the native format's cap.
const MAX_RECORD_BYTES: usize = 16 * 1024 * 1024;

/// Returns the in-region slot index for a world chunk position.
#[must_use]
pub const fn chunk_slot(chunk_x: i32, chunk_z: i32) -> usize {
    let local_x = chunk_x.rem_euclid(REGION_SIZE as i32) as usize;
    let local_z = chunk_z.rem_euclid(REGION_SIZE as i32) as usize;
    local_z * REGION_SIZE + local_x
}

/// Returns the vanilla region container file name (`r.<x>.<z>.mca`).
#[must_use]
pub fn region_file_name(region_x: i32, region_z: i32) -> String {
    format!("r.{region_x}.{region_z}.mca")
}

/// A parsed Anvil region header.
struct AnvilHeader {
    locations: Box<[u32; SECTOR_INTS]>,
    timestamps: Box<[u32; SECTOR_INTS]>,
}

impl AnvilHeader {
    fn new() -> Self {
        Self {
            locations: Box::new([0; SECTOR_INTS]),
            timestamps: Box::new([0; SECTOR_INTS]),
        }
    }

    fn from_bytes(bytes: &[u8]) -> Self {
        let mut header = Self::new();
        for (index, chunk) in bytes.as_chunks::<4>().0.iter().enumerate() {
            if index >= SECTOR_INTS {
                break;
            }
            header.locations[index] = u32::from_be_bytes(*chunk);
        }
        for (index, chunk) in bytes[SECTOR_INTS * 4..]
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
        {
            if index >= SECTOR_INTS {
                break;
            }
            header.timestamps[index] = u32::from_be_bytes(*chunk);
        }
        header
    }

    fn to_bytes(&self) -> [u8; HEADER_BYTES] {
        let mut bytes = [0u8; HEADER_BYTES];
        for (index, value) in self.locations.iter().enumerate() {
            bytes[index * 4..index * 4 + 4].copy_from_slice(&value.to_be_bytes());
        }
        for (index, value) in self.timestamps.iter().enumerate() {
            let offset = SECTOR_INTS * 4 + index * 4;
            bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        }
        bytes
    }

    const fn sector_start(location: u32) -> u32 {
        (location >> 8) & 0x00FF_FFFF
    }

    const fn sector_count(location: u32) -> u32 {
        location & 0xFF
    }

    fn contains(&self, slot: usize) -> bool {
        self.locations[slot] != 0
    }

    /// Validates every entry against the real file size, zeroing damaged slots the way
    /// vanilla `RegionFile` does after warning about them.
    fn sanitize(&mut self, file_sectors: u32) {
        for slot in 0..SECTOR_INTS {
            let location = self.locations[slot];
            if location == 0 {
                continue;
            }
            let start = Self::sector_start(location);
            let count = Self::sector_count(location);
            let invalid = if start < FIRST_DATA_SECTOR {
                Some(format!("sector {start} overlaps the region header"))
            } else if count == 0 {
                Some("sector count is zero".to_owned())
            } else if start.saturating_add(count) > file_sectors {
                Some(format!(
                    "sector range {start}..{} is past the end of the file ({file_sectors} sectors)",
                    start + count
                ))
            } else {
                None
            };
            if let Some(reason) = invalid {
                tracing::warn!("Anvil region slot {slot} is invalid: {reason}; discarding it");
                self.locations[slot] = 0;
            }
        }
    }

    /// Finds a free sector run of `needed` sectors using first-fit allocation.
    ///
    /// `exclude` names the slot being rewritten, so its previous allocation counts as free —
    /// mirroring vanilla's `RegionBitmap.free` of the old run.
    fn allocate(&self, needed: u32, file_sectors: u32, exclude: Option<usize>) -> u32 {
        if needed == 0 {
            return FIRST_DATA_SECTOR;
        }
        let mut used: Vec<(u32, u32)> = self
            .locations
            .iter()
            .enumerate()
            .filter(|(slot, location)| **location != 0 && Some(*slot) != exclude)
            .map(|(_, location)| {
                let start = Self::sector_start(*location);
                (start, start + Self::sector_count(*location))
            })
            .collect();
        used.sort_unstable();

        let mut candidate = FIRST_DATA_SECTOR;
        for (start, end) in used {
            if start >= candidate + needed {
                return candidate;
            }
            candidate = candidate.max(end);
        }
        candidate.max(file_sectors)
    }
}

/// One open Anvil region container.
pub struct AnvilRegionFile {
    external_dir: PathBuf,
    chunk_x: i32,
    chunk_z: i32,
    file: File,
    header: AnvilHeader,
    header_dirty: bool,
    file_sectors: u32,
}

impl AnvilRegionFile {
    /// Opens (and optionally creates) the region container for one region coordinate.
    ///
    /// Returns `Ok(None)` when the file does not exist and `create` is `false`.
    ///
    /// # Errors
    /// Propagates filesystem errors and reports structurally invalid headers.
    pub async fn open(
        directory: &Path,
        region_x: i32,
        region_z: i32,
        create: bool,
    ) -> io::Result<Option<Self>> {
        let path = directory.join(region_file_name(region_x, region_z));
        let exists = fs::try_exists(&path).await?;
        if !exists && !create {
            return Ok(None);
        }
        if create {
            fs::create_dir_all(directory).await?;
        }

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(create)
            .truncate(false)
            .open(&path)
            .await?;

        let file_len = file.metadata().await?.len();
        let mut header = AnvilHeader::new();
        if file_len > 0 {
            if file_len < HEADER_BYTES as u64 {
                tracing::warn!(
                    "Anvil region {} has a truncated header ({file_len} bytes); treating it as empty",
                    path.display()
                );
            }
            let mut bytes = vec![0u8; HEADER_BYTES.min(file_len as usize)];
            file.seek(SeekFrom::Start(0)).await?;
            file.read_exact(&mut bytes).await?;
            bytes.resize(HEADER_BYTES, 0);
            header = AnvilHeader::from_bytes(&bytes);
        }

        let file_sectors =
            ((file_len as usize).div_ceil(SECTOR_SIZE)).max(FIRST_DATA_SECTOR as usize) as u32;
        header.sanitize(file_sectors);

        Ok(Some(Self {
            external_dir: directory.to_path_buf(),
            chunk_x: region_x * REGION_SIZE as i32,
            chunk_z: region_z * REGION_SIZE as i32,
            file,
            header,
            header_dirty: false,
            file_sectors,
        }))
    }

    /// Returns whether this region currently holds a record for `slot`.
    #[must_use]
    pub fn contains(&self, slot: usize) -> bool {
        self.header.contains(slot)
    }

    /// Returns every occupied slot index in ascending order.
    #[must_use]
    pub fn occupied_slots(&self) -> Vec<usize> {
        (0..SECTOR_INTS)
            .filter(|slot| self.header.contains(*slot))
            .collect()
    }

    fn external_path(&self, slot: usize) -> PathBuf {
        let local_x = slot % REGION_SIZE;
        let local_z = slot / REGION_SIZE;
        self.external_dir.join(format!(
            "c.{}.{}.mcc",
            self.chunk_x + local_x as i32,
            self.chunk_z + local_z as i32
        ))
    }

    /// Reads and decompresses one record, returning `Ok(None)` when the slot is empty.
    ///
    /// # Errors
    /// Propagates filesystem errors; corrupt framings are reported as `InvalidData`.
    pub async fn read(&mut self, slot: usize) -> io::Result<Option<Vec<u8>>> {
        if slot >= SECTOR_INTS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("chunk slot {slot} is outside the region"),
            ));
        }
        let location = self.header.locations[slot];
        if location == 0 {
            return Ok(None);
        }
        let sector = AnvilHeader::sector_start(location);
        let sector_count = AnvilHeader::sector_count(location);
        let byte_offset = u64::from(sector) * SECTOR_SIZE as u64;

        let mut prefix = [0u8; RECORD_PREFIX_BYTES];
        self.file.seek(SeekFrom::Start(byte_offset)).await?;
        self.file.read_exact(&mut prefix).await?;
        let length = i32::from_be_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]);
        let compression_id = prefix[4];
        if length == 0 {
            tracing::warn!("Anvil region slot {slot} is allocated but has no stream");
            return Ok(None);
        }

        let external = compression_id & EXTERNAL_STREAM_FLAG != 0;
        let codec = Compression::from_id(compression_id & !EXTERNAL_STREAM_FLAG)?;

        if external {
            let path = self.external_path(slot);
            if !fs::try_exists(&path).await? {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "region slot {slot} points at external chunk file {} which does not exist",
                        path.display()
                    ),
                ));
            }
            let payload = fs::read(&path).await?;
            return Ok(Some(codec.decompress(&payload)?));
        }

        let payload_len = length
            .checked_sub(1)
            .filter(|len| *len >= 0)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("region slot {slot} declares a negative payload length"),
                )
            })? as usize;
        if payload_len > MAX_RECORD_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("region slot {slot} declares an implausible {payload_len}-byte payload"),
            ));
        }
        let capacity = sector_count as usize * SECTOR_SIZE - RECORD_PREFIX_BYTES;
        if payload_len > capacity {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "region slot {slot} declares a {payload_len}-byte payload but only {capacity} bytes are allocated"
                ),
            ));
        }

        let mut payload = vec![0u8; payload_len];
        self.file.read_exact(&mut payload).await?;
        Ok(Some(codec.decompress(&payload)?))
    }

    /// Compresses and stores one record, updating the location and timestamp tables.
    ///
    /// # Errors
    /// Propagates filesystem errors.
    pub async fn write(&mut self, slot: usize, payload: &[u8]) -> io::Result<()> {
        if slot >= SECTOR_INTS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("chunk slot {slot} is outside the region"),
            ));
        }
        let codec = Compression::DEFAULT;
        let compressed = codec.compress(payload)?;

        let mut record = Vec::with_capacity(RECORD_PREFIX_BYTES + compressed.len());
        record.extend_from_slice(&((compressed.len() + 1) as i32).to_be_bytes());
        record.push(codec.id());
        record.extend_from_slice(&compressed);

        let needed = (record.len() as u32).div_ceil(SECTOR_SIZE as u32);
        let previous = self.header.locations[slot];

        let (sector, sectors, record) = if needed >= EXTERNAL_CHUNK_THRESHOLD {
            // Vanilla stores oversized chunks beside the region file and leaves a stub record.
            let external_path = self.external_path(slot);
            tracing::warn!(
                "Anvil record {slot} needs {needed} sectors; storing it in {}",
                external_path.display()
            );
            write_atomic(&external_path, &compressed).await?;
            let stub = [
                1i32.to_be_bytes().as_slice(),
                &[codec.id() | EXTERNAL_STREAM_FLAG],
            ]
            .concat();
            (
                self.header.allocate(1, self.file_sectors, Some(slot)),
                1,
                stub,
            )
        } else {
            let sector = if previous != 0 && AnvilHeader::sector_count(previous) >= needed {
                AnvilHeader::sector_start(previous)
            } else {
                self.header.allocate(needed, self.file_sectors, Some(slot))
            };
            (sector, needed, record)
        };

        let byte_offset = u64::from(sector) * SECTOR_SIZE as u64;
        self.file.seek(SeekFrom::Start(byte_offset)).await?;
        self.file.write_all(&record).await?;
        let padding = (SECTOR_SIZE - (record.len() % SECTOR_SIZE)) % SECTOR_SIZE;
        if padding > 0 {
            self.file.write_all(&vec![0u8; padding]).await?;
        }
        self.file.flush().await?;

        let end = sector + sectors;
        if end > self.file_sectors {
            self.file_sectors = end;
        }
        self.header.locations[slot] = (sector << 8) | sectors;
        self.header.timestamps[slot] = unix_seconds();
        self.header_dirty = true;
        Ok(())
    }

    /// Removes one record from the location table.
    ///
    /// # Errors
    /// Propagates filesystem errors.
    pub async fn clear(&mut self, slot: usize) -> io::Result<()> {
        if slot >= SECTOR_INTS || self.header.locations[slot] == 0 {
            return Ok(());
        }
        self.header.locations[slot] = 0;
        self.header.timestamps[slot] = unix_seconds();
        self.header_dirty = true;
        let external = self.external_path(slot);
        if fs::try_exists(&external).await? {
            fs::remove_file(&external).await?;
        }
        Ok(())
    }

    /// Writes the header table back to disk when it changed.
    ///
    /// # Errors
    /// Propagates filesystem errors.
    pub async fn flush(&mut self) -> io::Result<()> {
        if !self.header_dirty {
            return Ok(());
        }
        self.file.seek(SeekFrom::Start(0)).await?;
        self.file.write_all(&self.header.to_bytes()).await?;
        self.file.flush().await?;
        self.header_dirty = false;
        Ok(())
    }

    /// Flushes and closes the container.
    ///
    /// # Errors
    /// Propagates filesystem errors.
    pub async fn close(mut self) -> io::Result<()> {
        self.flush().await
    }
}

/// Returns the chunk position owning a region slot.
#[must_use]
pub const fn slot_chunk_pos(region_x: i32, region_z: i32, slot: usize) -> ChunkPos {
    let local_x = (slot % REGION_SIZE) as i32;
    let local_z = (slot / REGION_SIZE) as i32;
    ChunkPos::new(
        region_x * REGION_SIZE as i32 + local_x,
        region_z * REGION_SIZE as i32 + local_z,
    )
}

async fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension("mcc.tmp");
    fs::write(&temporary, data).await?;
    fs::rename(&temporary, path).await
}

fn unix_seconds() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() as u32)
}
