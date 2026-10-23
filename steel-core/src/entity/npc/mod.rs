//! NPCs (vanilla `net.minecraft.world.entity.npc`).

pub mod gossip;

pub use gossip::{
    GossipContainer, GossipEntry, GossipType, ReputationEventHandler, ReputationEventType,
};

pub mod inventory_carrier;
pub mod npc;
pub mod trading;

pub use inventory_carrier::InventoryCarrier;
pub use npc::Npc;
