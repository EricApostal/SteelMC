//! Serverbound packet for saving a test block's settings.

use std::io::Cursor;

use steel_macros::ServerPacket;
use steel_registry::blocks::properties::TestBlockMode;
#[expect(unused_imports)]
use steel_registry::packets::play::S_SET_TEST_BLOCK;
use steel_utils::{BlockPos, codec::VarInt, serial::prefixed_read::read_utf};

/// Maximum UTF-16 code units for the message (vanilla `ByteBufCodecs.STRING_UTF8`).
pub const MAX_TEST_MESSAGE_LENGTH: usize = 32_767;

/// Sent when a player saves a test block from its edit screen.
///
/// Matches vanilla's `ServerboundSetTestBlockPacket`: the block position, the
/// mode as a `VarInt` id (vanilla `TestBlockMode.STREAM_CODEC`, which falls
/// back to the first variant for out-of-bounds ids), then the message string.
#[derive(ServerPacket, Clone, Debug)]
#[packet_id(Play = S_SET_TEST_BLOCK)]
pub struct SSetTestBlock {
    /// Position of the edited test block.
    pub pos: BlockPos,
    /// The test block's mode.
    pub mode: TestBlockMode,
    /// The test block's message.
    pub message: String,
}

impl steel_utils::serial::ReadFrom for SSetTestBlock {
    fn read(data: &mut Cursor<&[u8]>) -> std::io::Result<Self> {
        let pos = BlockPos::read(data)?;
        // Vanilla `ByIdMap.continuous(..., OutOfBoundsStrategy.ZERO)`: any
        // unknown id falls back to the first variant (`Start`).
        let mode = match VarInt::read(data)?.0 {
            1 => TestBlockMode::Log,
            2 => TestBlockMode::Fail,
            3 => TestBlockMode::Accept,
            _ => TestBlockMode::Start,
        };
        let message = read_utf(data, MAX_TEST_MESSAGE_LENGTH)?;

        Ok(Self { pos, mode, message })
    }
}
