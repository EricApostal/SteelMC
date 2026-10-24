//! Vanilla Anvil (`.mca` + NBT) storage support.
//!
//! This module is the whole vanilla-format layer:
//!
//! * [`region`] — the `r.X.Z.mca` container: 8 KiB offsets+timestamps header, 4 KiB sectors,
//!   per-record `i32` length plus compression id (gzip/deflate/none, plus lz4/custom rejection),
//!   and oversized-record spill into `c.X.Z.mcc`.
//! * [`chunk`] — `PersistentChunk` ⇄ vanilla chunk NBT (sections, palettes, light, heightmaps,
//!   ticks, block entities, structure references).
//! * [`entities`] / [`poi`] — the `entities/` and `poi/` side region directories.
//! * [`saved_data`] — the `data/<id>.dat` wrapper plus raids/weather/wandering-trader/game-rule
//!   payloads.
//! * [`storage`] — [`AnvilStorage`], the `ChunkStorage` backend selected by the `steel:anvil`
//!   world-storage factory.
//! * [`convert`] — whole-world import/export between Steel's `.srg` format and Anvil.
//!
//! `level.dat` lives in [`crate::level_data::anvil`] because it belongs to the level-data owner.

mod chunk;
mod compression;
mod convert;
mod entities;
mod nbt;
mod poi;
mod region;
pub(crate) mod saved_data;
mod storage;

#[cfg(test)]
mod tests;

pub use compression::Compression;
pub use convert::{
    AnvilConversionReport, VanillaRegion, export_world, for_each_steel_region_chunk, import_world,
    list_vanilla_regions,
};
pub use storage::AnvilStorage;

pub(crate) use nbt::{read_compressed_nbt, write_gzip_nbt};
