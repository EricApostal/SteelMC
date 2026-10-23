//! Clientbound game test highlight pos packet.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_GAME_TEST_HIGHLIGHT_POS;
use steel_utils::BlockPos;

/// Highlights two positions in the game test debug renderer.
///
/// Both positions use vanilla's `BlockPos.STREAM_CODEC`, a packed `long`.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_GAME_TEST_HIGHLIGHT_POS)]
pub struct CGameTestHighlightPos {
    /// Absolute position to highlight.
    pub absolute_pos: BlockPos,
    /// Position to highlight, relative to the test structure origin.
    pub relative_pos: BlockPos,
}

impl CGameTestHighlightPos {
    /// Creates a new game test highlight pos packet.
    #[must_use]
    pub const fn new(absolute_pos: BlockPos, relative_pos: BlockPos) -> Self {
        Self {
            absolute_pos,
            relative_pos,
        }
    }
}
