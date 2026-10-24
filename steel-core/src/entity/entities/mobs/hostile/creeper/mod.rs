//! Creeper implementation (vanilla `net.minecraft.world.entity.monster.Creeper`).

#[expect(
    clippy::module_inception,
    reason = "vanilla mirrors one class per directory (creeper); renaming would churn every consumer"
)]
pub mod creeper;

pub use creeper::CreeperEntity;
