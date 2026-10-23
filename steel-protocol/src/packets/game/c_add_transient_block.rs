//! Clientbound add transient block packet - shows a client-side-only block.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_ADD_TRANSIENT_BLOCK;
use steel_utils::{BlockPos, BlockStateId};

/// Places a transient (client-side only) block at a position.
///
/// Vanilla uses a hand-written codec (`Packet.codec`) that writes the position and
/// then the block state through `Block.BLOCK_STATE_REGISTRY`'s id mapper, which is
/// a plain `VarInt` global block-state id.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_ADD_TRANSIENT_BLOCK)]
pub struct CAddTransientBlock {
    /// Position of the transient block.
    pub pos: BlockPos,
    /// Global block state id shown at `pos`.
    pub block_state: BlockStateId,
}
