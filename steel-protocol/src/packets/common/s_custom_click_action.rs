//! Serverbound custom click action packet.

use std::io::{Cursor, Error, ErrorKind, Read, Result};

use simdnbt::owned::NbtTag;
use steel_macros::ServerPacket;
#[expect(unused_imports)]
use steel_registry::packets::config::S_CUSTOM_CLICK_ACTION;
#[expect(unused_imports)]
use steel_registry::packets::play::S_CUSTOM_CLICK_ACTION as PLAY_S_CUSTOM_CLICK_ACTION;
use steel_utils::Identifier;
use steel_utils::codec::VarInt;
use steel_utils::serial::ReadFrom;

/// Maximum size of the length-prefixed payload document in bytes.
///
/// Mirrors the `ByteBufCodecs.lengthPrefixed(65536)` wrapper around vanilla's
/// `UNTRUSTED_TAG_CODEC`.
pub const MAX_PAYLOAD_SIZE: usize = 65536;

/// Sent when a client clicks a custom UI element such as a dialog action.
///
/// Equivalent to vanilla's `ServerboundCustomClickActionPacket`: an action
/// [`Identifier`] followed by a length-prefixed optional NBT tag.
#[derive(ServerPacket, Clone, Debug)]
#[packet_id(Config = S_CUSTOM_CLICK_ACTION, Play = PLAY_S_CUSTOM_CLICK_ACTION)]
pub struct SCustomClickAction {
    /// The id of the clicked action.
    pub id: Identifier,
    /// The tag the action carries, or `None` when the document holds an END tag.
    pub payload: Option<NbtTag>,
}

impl ReadFrom for SCustomClickAction {
    fn read(data: &mut Cursor<&[u8]>) -> Result<Self> {
        let id = Identifier::read(data)?;
        let payload = read_length_prefixed_optional_tag(data, MAX_PAYLOAD_SIZE)?;

        Ok(Self { id, payload })
    }
}

/// Reads vanilla's `UNTRUSTED_TAG_CODEC`: a `VarInt` byte length followed by the
/// optional tag, which is either an END tag (`0x00`, meaning absent) or a tag
/// type id plus that tag's payload.
///
/// Mirrors `ByteBufCodecs.optionalTagCodec(..).apply(ByteBufCodecs.lengthPrefixed(65536))`,
/// so the length covers the tag type id and payload but not the length itself.
///
/// # Errors
///
/// Returns an error when the advertised length is negative, exceeds `max_size`,
/// or is not fully available, or when the tag itself is malformed.
fn read_length_prefixed_optional_tag(
    data: &mut Cursor<&[u8]>,
    max_size: usize,
) -> Result<Option<NbtTag>> {
    let size = VarInt::read(data)?.0;
    if size < 0 || size as usize > max_size {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("Payload size {size} is larger than allowed limit of {max_size}"),
        ));
    }

    let mut document = vec![0u8; size as usize];
    data.read_exact(&mut document)?;

    simdnbt::owned::read_optional_tag(&mut Cursor::new(document.as_slice()))
        .map_err(|error| Error::new(ErrorKind::InvalidData, format!("{error:?}")))
}
