//! Clientbound show dialog packet.

use std::io::{Result, Write};

use steel_macros::ClientPacket;
use steel_registry::RegistryEntry;
use steel_registry::dialog::DialogRef;
use steel_registry::packets::config::C_SHOW_DIALOG;
use steel_registry::packets::play::C_SHOW_DIALOG as PLAY_C_SHOW_DIALOG;
use steel_utils::codec::VarInt;
use steel_utils::serial::WriteTo;

/// Tells the client to open a dialog.
///
/// Vanilla's play-state codec is `Dialog.STREAM_CODEC`, i.e.
/// `ByteBufCodecs.holder(Registries.DIALOG, directCodec)`: a VarInt holder id
/// (`registry id + 1` for a reference, `0` followed by the inline dialog for a
/// direct holder). Steel's [`DialogRef`] is always a registered dialog, so this
/// packet writes the reference branch.
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(Config = C_SHOW_DIALOG, Play = PLAY_C_SHOW_DIALOG)]
pub struct CShowDialog {
    /// Dialog to show; must be a registered (`steel-registry`) dialog.
    pub dialog: DialogRef,
}

impl WriteTo for CShowDialog {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        // Registry holder reference: `VarInt(id + 1)`, per `ByteBufCodecs.holder`.
        VarInt(self.dialog.id() as i32 + 1).write(writer)
    }
}

impl CShowDialog {
    /// Creates a new show dialog packet.
    #[must_use]
    pub const fn new(dialog: DialogRef) -> Self {
        Self { dialog }
    }
}
