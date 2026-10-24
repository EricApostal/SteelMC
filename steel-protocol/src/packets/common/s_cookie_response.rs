//! Serverbound cookie response packet - the client's answer to a cookie request.

use std::io::{Cursor, Result};

use steel_macros::ServerPacket;
#[expect(unused_imports)]
use steel_registry::packets::config::S_COOKIE_RESPONSE;
#[expect(unused_imports)]
use steel_registry::packets::login::S_COOKIE_RESPONSE as LOGIN_S_COOKIE_RESPONSE;
#[expect(unused_imports)]
use steel_registry::packets::play::S_COOKIE_RESPONSE as PLAY_S_COOKIE_RESPONSE;
use steel_utils::Identifier;
use steel_utils::codec::VarInt;
use steel_utils::serial::{PrefixedRead, ReadFrom};

/// Maximum size of a cookie payload in bytes.
///
/// Mirrors vanilla's `ClientboundStoreCookiePacket.MAX_PAYLOAD_SIZE`, whose
/// `ByteBufCodecs.byteArray(5120)` codec is reused for the response payload.
pub const MAX_COOKIE_PAYLOAD_SIZE: usize = 5120;

/// The client's answer to a `ClientboundCookieRequestPacket`.
///
/// Vanilla's `ServerboundCookieResponsePacket` is registered in the login,
/// configuration and play states, so a single definition carries one
/// `#[packet_id]` per state.
#[derive(ServerPacket, Clone, Debug)]
#[packet_id(
    Login = LOGIN_S_COOKIE_RESPONSE,
    Config = S_COOKIE_RESPONSE,
    Play = PLAY_S_COOKIE_RESPONSE
)]
pub struct SCookieResponse {
    /// The cookie key the request was sent under.
    pub key: Identifier,
    /// The cookie value, or `None` when the client has no value for the key.
    pub payload: Option<Vec<u8>>,
}

impl ReadFrom for SCookieResponse {
    fn read(data: &mut Cursor<&[u8]>) -> Result<Self> {
        let key = Identifier::read(data)?;
        // Vanilla: `readNullable(PAYLOAD_STREAM_CODEC)` - a presence byte followed
        // by a `VarInt`-length-prefixed byte array.
        let payload = if bool::read(data)? {
            Some(Vec::<u8>::read_prefixed_bound::<VarInt>(
                data,
                MAX_COOKIE_PAYLOAD_SIZE,
            )?)
        } else {
            None
        };

        Ok(Self { key, payload })
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use steel_utils::serial::{ReadFrom as _, WriteTo as _};

    use super::*;

    #[test]
    fn cookie_response_reads_present_payload() {
        let mut bytes = Vec::new();
        Identifier::vanilla_static("brand")
            .write(&mut bytes)
            .expect("key should encode");
        bytes.push(1);
        bytes.push(3);
        bytes.extend_from_slice(b"abc");

        let packet = SCookieResponse::read(&mut Cursor::new(&bytes[..])).expect("should decode");

        assert_eq!(packet.key, Identifier::vanilla_static("brand"));
        assert_eq!(packet.payload.as_deref(), Some(&b"abc"[..]));
    }

    #[test]
    fn cookie_response_reads_absent_payload() {
        let mut bytes = Vec::new();
        Identifier::vanilla_static("brand")
            .write(&mut bytes)
            .expect("key should encode");
        bytes.push(0);

        let packet = SCookieResponse::read(&mut Cursor::new(&bytes[..])).expect("should decode");

        assert!(packet.payload.is_none());
    }

    #[test]
    fn cookie_response_rejects_payload_over_the_vanilla_limit() {
        let mut bytes = Vec::new();
        Identifier::vanilla_static("brand")
            .write(&mut bytes)
            .expect("key should encode");
        bytes.push(1);
        VarInt((MAX_COOKIE_PAYLOAD_SIZE + 1) as i32)
            .write(&mut bytes)
            .expect("length should encode");

        assert!(SCookieResponse::read(&mut Cursor::new(&bytes[..])).is_err());
    }
}
