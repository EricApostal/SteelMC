//! NPCs (vanilla `net.minecraft.world.entity.npc`).

pub mod gossip;

pub use gossip::{
    GossipContainer, GossipEntry, GossipType, ReputationEventHandler, ReputationEventType,
};

pub mod inventory_carrier;
#[expect(
    clippy::module_inception,
    reason = "vanilla mirrors one class per directory (npc/npc); renaming would churn every consumer"
)]
pub mod npc;
pub mod trading;

pub use inventory_carrier::InventoryCarrier;
pub use npc::Npc;
