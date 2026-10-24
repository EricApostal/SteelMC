//! Silverfish implementation (vanilla `net.minecraft.world.entity.monster.Silverfish`).

#[expect(
    clippy::module_inception,
    reason = "vanilla mirrors one class per directory (silverfish); renaming would churn every consumer"
)]
pub mod silverfish;

pub use silverfish::SilverfishEntity;
