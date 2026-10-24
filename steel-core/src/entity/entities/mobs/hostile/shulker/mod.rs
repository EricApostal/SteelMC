//! Shulker implementation (vanilla `net.minecraft.world.entity.monster.Shulker`).

#[expect(
    clippy::module_inception,
    reason = "vanilla mirrors one class per directory (shulker); renaming would churn every consumer"
)]
pub mod shulker;

pub use shulker::ShulkerEntity;
