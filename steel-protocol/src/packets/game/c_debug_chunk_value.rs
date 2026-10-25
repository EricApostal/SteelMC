//! Clientbound debug chunk value packet - a debug-renderer subscription value for a chunk.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_DEBUG_CHUNK_VALUE;
use steel_utils::{PackedChunkPos, serial::WriteTo};

use crate::packets::game::c_debug_block_value::DebugSubscriptionUpdate;

/// Sent to update the client's debug-renderer data for a single chunk.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_DEBUG_CHUNK_VALUE)]
pub struct CDebugChunkValue<V: WriteTo> {
    /// Chunk the value belongs to, in vanilla's packed `i64` form (`x` in the low 32 bits, `z` in
    /// the high 32 bits). Steel's `ChunkPos` writes two `i32`s, so the packed type is used here to
    /// match vanilla's `FriendlyByteBuf.writeChunkPos`.
    pub chunk_pos: PackedChunkPos,
    /// The subscription value that changed for this chunk.
    pub update: DebugSubscriptionUpdate<V>,
}

impl<V: WriteTo> CDebugChunkValue<V> {
    /// Creates a new debug chunk value packet.
    #[must_use]
    pub const fn new(chunk_pos: PackedChunkPos, update: DebugSubscriptionUpdate<V>) -> Self {
        Self { chunk_pos, update }
    }
}
