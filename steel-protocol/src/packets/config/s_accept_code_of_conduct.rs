//! Serverbound accept code of conduct packet.

use steel_macros::{ReadFrom, ServerPacket};
#[expect(unused_imports)]
use steel_registry::packets::config::S_ACCEPT_CODE_OF_CONDUCT;

/// Sent by the client when it accepts the server's code of conduct.
///
/// Equivalent to vanilla's `ServerboundAcceptCodeOfConductPacket`, which is
/// encoded with `StreamCodec.unit` and therefore carries no fields.
#[derive(ReadFrom, ServerPacket, Clone, Debug)]
#[packet_id(Config = S_ACCEPT_CODE_OF_CONDUCT)]
pub struct SAcceptCodeOfConduct {}
