//! Serverbound custom query answer packet (login state).

use std::io::{Cursor, Error, ErrorKind, Result};

use steel_macros::ServerPacket;
#[expect(unused_imports)]
use steel_registry::packets::login::S_CUSTOM_QUERY_ANSWER;
use steel_utils::codec::VarInt;
use steel_utils::serial::ReadFrom;

/// Maximum size of a custom query answer payload in bytes.
///
/// Mirrors vanilla's `ServerboundCustomQueryAnswerPacket.MAX_PAYLOAD_SIZE`.
pub const MAX_PAYLOAD_SIZE: usize = 1_048_576;

/// The payload of a custom query answer.
///
/// Mirrors vanilla's `CustomQueryAnswerPayload` interface. The vanilla reader
/// cannot know the shape a custom query answer uses, so it consumes the whole
/// remaining payload as an unknown blob and keeps only this marker:
/// `DiscardedQueryAnswerPayload`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CustomQueryAnswerPayload {
    /// The answer's bytes were not interpreted and have been skipped.
    Discarded,
}

/// Sent by the client as its answer to a `ClientboundCustomQueryPacket`.
///
/// Equivalent to vanilla's `ServerboundCustomQueryAnswerPacket`: a `VarInt`
/// transaction id followed by an optional, uninterpreted payload.
#[derive(ServerPacket, Clone, Debug)]
#[packet_id(Login = S_CUSTOM_QUERY_ANSWER)]
pub struct SCustomQueryAnswer {
    /// The transaction id of the query this answers.
    pub transaction_id: i32,
    /// The answer payload.
    ///
    /// Vanilla's reader never produces `None`: an empty payload still yields
    /// `DiscardedQueryAnswerPayload`. The field stays optional to mirror the
    /// nullable `payload` component of the vanilla record (the client writes a
    /// presence byte followed by the payload bytes).
    pub payload: Option<CustomQueryAnswerPayload>,
}

impl ReadFrom for SCustomQueryAnswer {
    fn read(data: &mut Cursor<&[u8]>) -> Result<Self> {
        let transaction_id = VarInt::read(data)?.0;
        let payload = read_unknown_payload(data)?;

        Ok(Self {
            transaction_id,
            payload,
        })
    }
}

/// Reads vanilla's `readUnknownPayload`: the payload is whatever bytes remain in
/// the packet, and it is discarded without being parsed.
///
/// # Errors
///
/// Returns an error when more than [`MAX_PAYLOAD_SIZE`] bytes remain.
fn read_unknown_payload(data: &mut Cursor<&[u8]>) -> Result<Option<CustomQueryAnswerPayload>> {
    let remaining = data
        .get_ref()
        .len()
        .saturating_sub(data.position() as usize);
    if remaining > MAX_PAYLOAD_SIZE {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("Payload may not be larger than {MAX_PAYLOAD_SIZE} bytes"),
        ));
    }

    data.set_position(data.position() + remaining as u64);

    Ok(Some(CustomQueryAnswerPayload::Discarded))
}
