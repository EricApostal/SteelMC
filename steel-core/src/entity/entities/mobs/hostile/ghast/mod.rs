//! Ghast implementation (vanilla `net.minecraft.world.entity.monster.Ghast`).
// TODO(integrator): entity modules are declared by their owner once the entity files exist.

#[expect(
    clippy::module_inception,
    reason = "vanilla mirrors one class per directory (ghast); renaming would churn every consumer"
)]
pub mod ghast;
pub use ghast::GhastEntity;
