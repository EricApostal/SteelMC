//! Serverbound packet for saving a command block's settings.

use std::io::Cursor;

use steel_macros::{ReadFrom, ServerPacket};
#[expect(unused_imports)]
use steel_registry::packets::play::S_SET_COMMAND_BLOCK;
use steel_utils::{BlockPos, serial::prefixed_read::read_utf};

/// Maximum UTF-16 code units for the command string (vanilla `readUtf()`).
pub const MAX_COMMAND_BLOCK_COMMAND_LENGTH: usize = 32_767;

/// The execution mode of a command block.
///
/// Mirrors vanilla's `CommandBlockEntity.Mode`, which is written with
/// `FriendlyByteBuf.writeEnum` as the enum ordinal (`VarInt`).
#[derive(ReadFrom, Clone, Copy, Debug, PartialEq, Eq)]
#[read(as = VarInt)]
pub enum CommandBlockMode {
    /// Runs after the command block pointing into it.
    Sequence = 0,
    /// Runs every tick while it has been triggered.
    Auto = 1,
    /// Runs when powered by redstone.
    Redstone = 2,
}

/// Sent when a player saves a command block from its edit screen.
///
/// Matches vanilla's `ServerboundSetCommandBlockPacket`: the block position,
/// the command string, the mode as a `VarInt` enum ordinal, then one flags
/// byte packing `trackOutput` (bit 0), `conditional` (bit 1) and `automatic`
/// (bit 2).
#[derive(ServerPacket, Clone, Debug)]
#[packet_id(Play = S_SET_COMMAND_BLOCK)]
pub struct SSetCommandBlock {
    /// Position of the edited command block.
    pub pos: BlockPos,
    /// The command to store in the block.
    pub command: String,
    /// The command block's execution mode.
    pub mode: CommandBlockMode,
    /// Whether the block should broadcast its output to nearby players.
    pub track_output: bool,
    /// Whether the block only runs when its condition is met.
    pub conditional: bool,
    /// Whether the block keeps running after a redstone pulse.
    pub automatic: bool,
}

impl SSetCommandBlock {
    /// Flags bit for `track_output` (vanilla `FLAG_TRACK_OUTPUT`).
    pub const FLAG_TRACK_OUTPUT: i8 = 1;
    /// Flags bit for `conditional` (vanilla `FLAG_CONDITIONAL`).
    pub const FLAG_CONDITIONAL: i8 = 2;
    /// Flags bit for `automatic` (vanilla `FLAG_AUTOMATIC`).
    pub const FLAG_AUTOMATIC: i8 = 4;
}

impl steel_utils::serial::ReadFrom for SSetCommandBlock {
    fn read(data: &mut Cursor<&[u8]>) -> std::io::Result<Self> {
        let pos = BlockPos::read(data)?;
        let command = read_utf(data, MAX_COMMAND_BLOCK_COMMAND_LENGTH)?;
        let mode = CommandBlockMode::read(data)?;
        let flags = i8::read(data)?;
        Ok(Self {
            pos,
            command,
            mode,
            track_output: flags & Self::FLAG_TRACK_OUTPUT != 0,
            conditional: flags & Self::FLAG_CONDITIONAL != 0,
            automatic: flags & Self::FLAG_AUTOMATIC != 0,
        })
    }
}
