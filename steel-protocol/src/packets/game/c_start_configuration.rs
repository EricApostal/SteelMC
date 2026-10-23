//! Clientbound start configuration packet.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_START_CONFIGURATION;

/// Tells the client to leave the play state and re-enter configuration.
///
/// Unit packet with no payload. Vanilla's `ClientboundStartConfigurationPacket`
/// marks itself terminal.
#[derive(ClientPacket, WriteTo, Clone, Debug, Default)]
#[packet_id(Play = C_START_CONFIGURATION)]
pub struct CStartConfiguration;

impl CStartConfiguration {
    /// Creates a new start configuration packet.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}
