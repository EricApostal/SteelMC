//! Clientbound delete chat packet - tells the client to remove a received chat message.

use std::io::{Error, ErrorKind, Result, Write};

use steel_macros::ClientPacket;
use steel_registry::packets::play::C_DELETE_CHAT;
use steel_utils::{codec::VarInt, serial::WriteTo};

use crate::packets::game::PreviousMessage;

/// Sent when a chat message the client already received must be deleted from its chat log.
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(Play = C_DELETE_CHAT)]
pub struct CDeleteChat {
    /// Packed message signature identifying the message to delete.
    ///
    /// This reuses Steel's existing packed signature representation (vanilla's
    /// `MessageSignature.Packed`): `id` is written as-is and holds the raw `VarInt` value, where
    /// `0` means the 256-byte signature follows inline and any other value is the
    /// `MessageSignatureCache` index plus one. Use `id: 0` together with `signature: Some(..)` for
    /// a full signature, or `signature: None` for a cache reference, exactly as `CPlayerChat`
    /// encodes its `PreviousMessage` entries.
    pub message_signature: PreviousMessage,
}

impl CDeleteChat {
    /// Creates a new delete chat packet.
    #[must_use]
    pub const fn new(message_signature: PreviousMessage) -> Self {
        Self { message_signature }
    }
}

impl WriteTo for CDeleteChat {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        VarInt(self.message_signature.id).write(writer)?;
        // Vanilla writes the full signature only when one is present (id 0 is the full-signature
        // marker on the wire).
        if self.message_signature.id == 0 {
            let signature = self.message_signature.signature.as_ref().ok_or_else(|| {
                Error::new(
                    ErrorKind::InvalidData,
                    "MessageSignature with id=0 must carry a full signature",
                )
            })?;
            writer.write_all(signature)?;
        }
        Ok(())
    }
}
