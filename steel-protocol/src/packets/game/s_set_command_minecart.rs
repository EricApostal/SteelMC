//! Serverbound packet for saving a command block minecart's settings.

use std::io::Cursor;

use steel_macros::ServerPacket;
#[expect(unused_imports)]
use steel_registry::packets::play::S_SET_COMMAND_MINECART;
use steel_utils::{codec::VarInt, serial::prefixed_read::read_utf};

/// Maximum UTF-16 code units for the command string (vanilla `readUtf()`).
pub const MAX_COMMAND_MINECART_COMMAND_LENGTH: usize = 32_767;

/// Sent when a player saves a command block minecart from its edit screen.
///
/// Matches vanilla's `ServerboundSetCommandMinecartPacket`: the entity id as a
/// `VarInt`, the command string, then the `trackOutput` boolean.
#[derive(ServerPacket, Clone, Debug)]
#[packet_id(Play = S_SET_COMMAND_MINECART)]
pub struct SSetCommandMinecart {
    /// Entity id of the command block minecart.
    pub entity: i32,
    /// The command to store in the minecart's command block.
    pub command: String,
    /// Whether the minecart should broadcast its output to nearby players.
    pub track_output: bool,
}

impl steel_utils::serial::ReadFrom for SSetCommandMinecart {
    fn read(data: &mut Cursor<&[u8]>) -> std::io::Result<Self> {
        let entity = VarInt::read(data)?.0;
        let command = read_utf(data, MAX_COMMAND_MINECART_COMMAND_LENGTH)?;
        let track_output = bool::read(data)?;
        Ok(Self {
            entity,
            command,
            track_output,
        })
    }
}
