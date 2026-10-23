//! Clientbound custom query packet (login state).

use std::io::{Result, Write};

use steel_macros::ClientPacket;
use steel_registry::packets::login::C_CUSTOM_QUERY;
use steel_utils::{Identifier, codec::VarInt, serial::WriteTo};

/// Maximum payload size accepted by vanilla's reader.
///
/// Mirrors `ClientboundCustomQueryPacket.MAX_PAYLOAD_SIZE`.
pub const MAX_CUSTOM_QUERY_PAYLOAD_SIZE: usize = 1_048_576;

/// Asks the client a server-defined login question.
///
/// Mirrors vanilla's `ClientboundCustomQueryPacket`: a `VarInt` transaction id,
/// the payload's identifier, then the payload bytes written by the payload
/// itself (`CustomQueryPayload::write`). The payload is NOT length-prefixed:
/// vanilla's reader consumes every remaining byte of the packet
/// (`readUnknownPayload`), so the bytes after the identifier are the payload.
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(Login = C_CUSTOM_QUERY)]
pub struct CCustomQuery {
    /// Transaction id echoed back by the client in its answer.
    pub transaction_id: i32,
    /// Identifier naming the query payload type.
    pub id: Identifier,
    /// Raw payload bytes written verbatim after the identifier.
    pub payload: Vec<u8>,
}

impl CCustomQuery {
    /// Creates a new custom query packet.
    #[must_use]
    pub const fn new(transaction_id: i32, id: Identifier, payload: Vec<u8>) -> Self {
        Self {
            transaction_id,
            id,
            payload,
        }
    }
}

impl WriteTo for CCustomQuery {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        VarInt(self.transaction_id).write(writer)?;
        self.id.write(writer)?;
        writer.write_all(&self.payload)
    }
}
