//! Vanilla region-file compression ids.
//!
//! Mirrors `RegionFileVersion` (vanilla `net.minecraft.world.level.chunk.storage`):
//! id 1 = gzip, 2 = deflate (zlib, vanilla's default writer), 3 = none,
//! 4 = lz4 block, 127 = custom resource-location tagged stream.
//!
//! Steel intentionally supports 1/2/3 only: lz4 (4) and custom (127) are rejected
//! with an explicit error instead of being silently mangled, because Steel does not
//! depend on an lz4 implementation.

use std::io::{self, Read, Write};

use flate2::Compression as FlateLevel;
use flate2::read::{GzDecoder, ZlibDecoder};
use flate2::write::{GzEncoder, ZlibEncoder};

/// Vanilla region-file compression id for gzip streams (`VERSION_GZIP`).
pub const ID_GZIP: u8 = 1;
/// Vanilla region-file compression id for zlib/deflate streams (`VERSION_DEFLATE`).
pub const ID_DEFLATE: u8 = 2;
/// Vanilla region-file compression id for uncompressed payloads (`VERSION_NONE`).
pub const ID_NONE: u8 = 3;
/// Vanilla region-file compression id for lz4 block streams (`VERSION_LZ4`), unsupported.
pub const ID_LZ4: u8 = 4;
/// Vanilla region-file compression id for resource-location tagged streams (`VERSION_CUSTOM`).
pub const ID_CUSTOM: u8 = 127;

/// The compression used for one Anvil region record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    /// gzip stream (id 1, legacy default).
    Gzip,
    /// zlib/deflate stream (id 2, current vanilla default).
    Deflate,
    /// Uncompressed bytes (id 3, `VERSION_NONE`).
    Uncompressed,
}

impl Compression {
    /// Vanilla's default writer compression (`RegionFileVersion.DEFAULT`).
    pub const DEFAULT: Self = Self::Deflate;

    /// Returns the vanilla compression id for this codec.
    #[must_use]
    pub const fn id(self) -> u8 {
        match self {
            Self::Gzip => ID_GZIP,
            Self::Deflate => ID_DEFLATE,
            Self::Uncompressed => ID_NONE,
        }
    }

    /// Resolves a vanilla compression id, rejecting unsupported codecs.
    ///
    /// # Errors
    /// Returns `InvalidData` for ids Steel cannot decode (lz4, custom) or ids that
    /// are not part of `RegionFileVersion`.
    pub fn from_id(id: u8) -> io::Result<Self> {
        match id {
            ID_GZIP => Ok(Self::Gzip),
            ID_DEFLATE => Ok(Self::Deflate),
            ID_NONE => Ok(Self::Uncompressed),
            ID_LZ4 => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "region record uses lz4 compression (id 4), which this build cannot decode",
            )),
            ID_CUSTOM => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "region record uses a custom resource-location compression (id 127), which this build cannot decode",
            )),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("region record uses unknown compression id {other}"),
            )),
        }
    }

    /// Compresses one record payload.
    ///
    /// # Errors
    /// Propagates the encoder's I/O errors.
    pub fn compress(self, data: &[u8]) -> io::Result<Vec<u8>> {
        match self {
            Self::Uncompressed => Ok(data.to_vec()),
            Self::Deflate => {
                let mut encoder = ZlibEncoder::new(Vec::new(), FlateLevel::default());
                encoder.write_all(data)?;
                encoder.finish()
            }
            Self::Gzip => {
                let mut encoder = GzEncoder::new(Vec::new(), FlateLevel::default());
                encoder.write_all(data)?;
                encoder.finish()
            }
        }
    }

    /// Decompresses one record payload.
    ///
    /// # Errors
    /// Propagates decoder I/O errors (truncated or corrupt streams).
    pub fn decompress(self, data: &[u8]) -> io::Result<Vec<u8>> {
        match self {
            Self::Uncompressed => Ok(data.to_vec()),
            Self::Deflate => read_all(ZlibDecoder::new(data)),
            Self::Gzip => read_all(GzDecoder::new(data)),
        }
    }
}

fn read_all(mut reader: impl Read) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    reader.read_to_end(&mut out)?;
    Ok(out)
}
