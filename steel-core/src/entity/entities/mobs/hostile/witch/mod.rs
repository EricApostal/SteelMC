//! Witch implementation (vanilla `net.minecraft.world.entity.monster.Witch`).

#[expect(
    clippy::module_inception,
    reason = "vanilla mirrors one class per directory (witch); renaming would churn every consumer"
)]
pub mod witch;

pub use witch::WitchEntity;
