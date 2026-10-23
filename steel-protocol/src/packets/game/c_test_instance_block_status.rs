//! Clientbound test instance block status packet.

use std::io::{Result, Write};

use glam::IVec3;
use steel_macros::ClientPacket;
use steel_registry::packets::play::C_TEST_INSTANCE_BLOCK_STATUS;
use steel_utils::{codec::VarInt, serial::WriteTo};
use text_components::TextComponent;

/// Reports the status (and optional size) of a test instance block to the client.
///
/// Wire order mirrors vanilla's `ClientboundTestInstanceBlockStatus` record:
/// the status component, then `ByteBufCodecs.optional(Vec3i.STREAM_CODEC)` - a
/// presence boolean followed by three VarInts.
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(Play = C_TEST_INSTANCE_BLOCK_STATUS)]
pub struct CTestInstanceBlockStatus {
    /// Human-readable status of the test instance block.
    pub status: TextComponent,
    /// Optional test structure size, written as three VarInts when present.
    pub size: Option<IVec3>,
}

impl WriteTo for CTestInstanceBlockStatus {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        self.status.write(writer)?;
        match self.size {
            Some(size) => {
                true.write(writer)?;
                VarInt(size.x).write(writer)?;
                VarInt(size.y).write(writer)?;
                VarInt(size.z).write(writer)?;
            }
            None => false.write(writer)?,
        }
        Ok(())
    }
}

impl CTestInstanceBlockStatus {
    /// Creates a new test instance block status packet.
    #[must_use]
    pub const fn new(status: TextComponent, size: Option<IVec3>) -> Self {
        Self { status, size }
    }
}
