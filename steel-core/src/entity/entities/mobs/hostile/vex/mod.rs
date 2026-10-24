//! Vex implementation (vanilla `net.minecraft.world.entity.monster.Vex`).

#[expect(
    clippy::module_inception,
    reason = "vanilla mirrors one class per directory (vex); renaming would churn every consumer"
)]
pub mod vex;

pub use vex::VexEntity;
