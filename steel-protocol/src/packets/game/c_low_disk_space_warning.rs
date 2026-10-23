//! Clientbound low disk space warning packet.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_LOW_DISK_SPACE_WARNING;

/// Warns the client that the server is low on disk space.
///
/// Unit packet with no payload.
#[derive(ClientPacket, WriteTo, Clone, Debug, Default)]
#[packet_id(Play = C_LOW_DISK_SPACE_WARNING)]
pub struct CLowDiskSpaceWarning;

impl CLowDiskSpaceWarning {
    /// Creates a new low disk space warning packet.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}
