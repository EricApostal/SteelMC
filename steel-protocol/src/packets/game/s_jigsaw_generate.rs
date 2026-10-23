//! Serverbound packet asking a jigsaw block to generate its structure.

use steel_macros::{ReadFrom, ServerPacket};
#[expect(unused_imports)]
use steel_registry::packets::play::S_JIGSAW_GENERATE;
use steel_utils::BlockPos;

/// Sent when a player presses "Generate" in a jigsaw block's edit screen.
///
/// Matches vanilla's `ServerboundJigsawGeneratePacket`: the block position, the
/// requested recursion depth as a `VarInt`, then the `keepJigsaws` boolean.
#[derive(ReadFrom, ServerPacket, Clone, Debug)]
#[packet_id(Play = S_JIGSAW_GENERATE)]
pub struct SJigsawGenerate {
    /// Position of the jigsaw block that should generate.
    pub pos: BlockPos,
    /// How many recursion levels to generate.
    #[read(as = VarInt)]
    pub levels: i32,
    /// Whether jigsaw blocks are kept (rather than replaced) after generation.
    pub keep_jigsaws: bool,
}
