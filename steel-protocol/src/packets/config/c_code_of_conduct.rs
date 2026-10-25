//! Clientbound code of conduct packet.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::config::C_CODE_OF_CONDUCT;

/// Sends the server's code of conduct, which the client shows before joining.
///
/// Mirrors vanilla's `ClientboundCodeOfConductPacket`: a single `STRING_UTF8`
/// value, i.e. a VarInt-prefixed UTF-8 string of at most 32767 bytes.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Config = C_CODE_OF_CONDUCT)]
pub struct CCodeOfConduct {
    /// Code of conduct text.
    #[write(as = Prefixed(VarInt), bound = 32767)]
    pub code_of_conduct: String,
}

impl CCodeOfConduct {
    /// Creates a new code of conduct packet.
    #[must_use]
    pub const fn new(code_of_conduct: String) -> Self {
        Self { code_of_conduct }
    }
}
