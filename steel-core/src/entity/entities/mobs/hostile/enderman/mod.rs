//! Enderman implementation (vanilla `net.minecraft.world.entity.monster.Enderman`).
// TODO(integrator): entity modules are declared by their owner once the entity files exist.

#[expect(
    clippy::module_inception,
    reason = "vanilla mirrors one class per directory (enderman); renaming would churn every consumer"
)]
pub mod enderman;
pub use enderman::EndermanEntity;
