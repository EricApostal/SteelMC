//! Clientbound ping packet - the `i32` play/configuration ping.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::{config, play};

/// Sent to the client to measure the connection's round-trip time.
///
/// Equivalent to vanilla's `ClientboundPingPacket`: a single big-endian `i32` that the
/// client echoes back in `ServerboundPongPacket`. Vanilla registers this packet in the
/// configuration and play states but never sends it from the server itself, so Steel only
/// needs the codec for completeness.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Config = config::C_PING, Play = play::C_PING)]
pub struct CPing {
    /// The challenge id the client echoes back.
    pub id: i32,
}

impl CPing {
    /// Creates a new ping packet.
    #[must_use]
    pub const fn new(id: i32) -> Self {
        Self { id }
    }
}

#[cfg(test)]
mod tests {
    use steel_utils::serial::WriteTo as _;

    use super::*;

    #[test]
    fn ping_writes_the_challenge_as_a_big_endian_int() {
        let mut bytes = Vec::new();
        CPing::new(0x0102_0304)
            .write(&mut bytes)
            .expect("ping should encode");

        assert_eq!(bytes, 0x0102_0304_i32.to_be_bytes());
    }
}
