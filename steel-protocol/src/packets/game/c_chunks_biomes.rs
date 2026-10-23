//! Clientbound chunks biomes packet - replaces the biome data of loaded chunks.

use std::io::{Error, Result, Write};

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_CHUNKS_BIOMES;
use steel_utils::{PackedChunkPos, codec::VarInt, serial::WriteTo};

/// Vanilla's `TWO_MEGABYTES` bound for a single chunk's serialized biome buffer.
pub const TWO_MEGABYTES: usize = 2_097_152;

/// Serialized biome data for one chunk.
///
/// Mirrors vanilla's nested `ClientboundChunksBiomesPacket.ChunkBiomeData` record:
/// a packed chunk position followed by a VarInt-prefixed byte buffer
/// (`ByteBufCodecs.byteArray(2097152)`).
#[derive(Clone, Debug)]
pub struct ChunkBiomeData {
    /// Position of the chunk whose biomes are replaced, in vanilla's packed `long` layout.
    pub pos: PackedChunkPos,
    /// Raw biome data: a VarInt byte length followed by the bytes themselves.
    pub buffer: Vec<u8>,
}

impl WriteTo for ChunkBiomeData {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        if self.buffer.len() > TWO_MEGABYTES {
            return Err(Error::other(format!(
                "ByteArray with size {} is bigger than allowed {TWO_MEGABYTES}",
                self.buffer.len()
            )));
        }

        self.pos.write(writer)?;
        VarInt(self.buffer.len() as i32).write(writer)?;
        writer.write_all(&self.buffer)
    }
}

/// Replaces the biome data of the listed chunks on the client.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_CHUNKS_BIOMES)]
pub struct CChunksBiomes {
    /// One entry per chunk whose biome data is replaced.
    pub chunk_biome_data: Vec<ChunkBiomeData>,
}

impl CChunksBiomes {
    /// Creates a new chunks-biomes packet.
    #[must_use]
    pub const fn new(chunk_biome_data: Vec<ChunkBiomeData>) -> Self {
        Self { chunk_biome_data }
    }
}
