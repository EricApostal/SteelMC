//! Clientbound mount screen open packet.

use std::io::{Result, Write};

use steel_macros::ClientPacket;
use steel_registry::packets::play::C_MOUNT_SCREEN_OPEN;
use steel_utils::{codec::VarInt, serial::WriteTo};

/// Opens the inventory screen of the entity the player is riding.
///
/// Wire order mirrors vanilla's `ClientboundMountScreenOpenPacket`:
/// `writeContainerId` (VarInt), `writeVarInt` for the column count and a plain
/// `writeInt` for the entity id.
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(Play = C_MOUNT_SCREEN_OPEN)]
pub struct CMountScreenOpen {
    /// Container id the client should use for slot interactions.
    pub container_id: i32,
    /// Number of inventory columns to render.
    pub inventory_columns: i32,
    /// Entity id of the mount whose inventory is opened.
    pub entity_id: i32,
}

impl WriteTo for CMountScreenOpen {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        VarInt(self.container_id).write(writer)?;
        VarInt(self.inventory_columns).write(writer)?;
        self.entity_id.write(writer)?;
        Ok(())
    }
}

impl CMountScreenOpen {
    /// Creates a new mount screen open packet.
    #[must_use]
    pub const fn new(container_id: i32, inventory_columns: i32, entity_id: i32) -> Self {
        Self {
            container_id,
            inventory_columns,
            entity_id,
        }
    }
}
