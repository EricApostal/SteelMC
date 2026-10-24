//! Blaze implementation (vanilla `net.minecraft.world.entity.monster.Blaze`).

#[expect(
    clippy::module_inception,
    reason = "vanilla mirrors one class per directory (blaze); renaming would churn every consumer"
)]
pub mod blaze;

pub use blaze::BlazeEntity;
