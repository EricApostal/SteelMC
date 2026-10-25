//! Compressed NBT helpers shared by `level.dat` and `data/*.dat`.
//!
//! Vanilla writes both files as gzip-compressed NBT (`NbtIo.writeCompressed`) and reads
//! them with compression auto-detection (`SavedDataStorage`), so the reader here accepts
//! raw NBT as well.

use std::io::{self, Cursor, Read, Write};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use simdnbt::borrow::{
    NbtCompound as BorrowedNbtCompound, NbtList as BorrowedNbtList, read_compound,
};
use simdnbt::owned::NbtCompound;

/// Collects the compounds of a borrowed list tag into an indexable `Vec`.
///
/// `simdnbt`'s borrowed compound list is not an `Iterator`, so every reader in this module
/// goes through this helper.
#[must_use]
pub(crate) fn compound_list<'a, 'tape>(
    list: BorrowedNbtList<'a, 'tape>,
) -> Vec<BorrowedNbtCompound<'a, 'tape>> {
    match list.compounds() {
        Some(compounds) => (0..compounds.len())
            .filter_map(|index| compounds.get(index))
            .collect(),
        None => Vec::new(),
    }
}

/// Reads NBT bytes, transparently decompressing a gzip stream.
///
/// # Errors
/// Returns `InvalidData` when the payload is neither gzip nor valid NBT.
pub fn read_compressed_nbt(bytes: &[u8]) -> io::Result<NbtCompound> {
    let raw = if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut decoder = GzDecoder::new(bytes);
        let mut out = Vec::new();
        decoder
            .read_to_end(&mut out)
            .map_err(|error| invalid(format!("failed to decompress gzip NBT: {error}")))?;
        out
    } else {
        bytes.to_vec()
    };
    read_raw_nbt(&raw)
}

/// Reads uncompressed NBT bytes into an owned compound.
///
/// # Errors
/// Returns `InvalidData` when the payload is not a valid NBT compound.
pub fn read_raw_nbt(bytes: &[u8]) -> io::Result<NbtCompound> {
    if bytes.is_empty() {
        return Err(invalid("NBT payload is empty"));
    }
    let mut cursor = Cursor::new(bytes);
    let borrowed = read_compound(&mut cursor)
        .map_err(|error| invalid(format!("failed to parse NBT: {error:?}")))?;
    Ok(simdnbt::borrow::NbtCompound::from(&borrowed).to_owned())
}

/// Serializes a compound to uncompressed NBT bytes.
#[must_use]
pub fn write_raw_nbt(compound: &NbtCompound) -> Vec<u8> {
    let mut bytes = Vec::new();
    compound.write(&mut bytes);
    bytes
}

/// Serializes a compound to gzip-compressed NBT bytes, matching `NbtIo.writeCompressed`.
///
/// # Errors
/// Propagates encoder I/O errors.
pub fn write_gzip_nbt(compound: &NbtCompound) -> io::Result<Vec<u8>> {
    let raw = write_raw_nbt(compound);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&raw)?;
    encoder.finish()
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
